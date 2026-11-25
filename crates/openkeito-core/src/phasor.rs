//! Phasor solutions: the complex nodal equations at one angular frequency,
//! for the steady-state initialization of a run and for frequency scans.
//!
//! Phasors are rms values with a cosine reference:
//! x(t) = sqrt(2) |X| cos(wt + arg X).

use crate::clock::Stopwatch;
use std::f64::consts::PI;

use openkeito_model::{self as model, BusRef, Diagnostic, Model, Severity};
use serde::Serialize;

use crate::element::Element;
use crate::linalg::{Complex64, Factors, Solver};
use crate::netlist;
use crate::sim::SimError;

/// Factorizes the complex nodal matrix at `omega`. On a singular matrix,
/// returns the index of the unknown that cannot be determined.
pub(crate) fn factor(
    solver: &Solver,
    elements: &[Box<dyn Element>],
    omega: f64,
) -> Result<Factors<Complex64>, usize> {
    if let Some(k) = elements.iter().find_map(|e| e.phasor_singular(omega)) {
        return Err(k);
    }
    solver.factor(|m| {
        for e in elements {
            e.stamp_phasor(omega, m);
        }
    })
}

/// e^{j 120 deg}.
fn a_operator() -> Complex64 {
    Complex64::from_polar(1.0, 2.0 * PI / 3.0)
}

/// Diagonal [Z0, Z1, Z2] of the symmetrical components of a 3x3 phase
/// matrix: Z012 = A^-1 Zabc A with A = [[1, 1, 1], [1, a^2, a], [1, a, a^2]].
pub(crate) fn sequence_diagonal(z: &[[Complex64; 3]; 3]) -> [Complex64; 3] {
    let a = a_operator();
    let a2 = a * a;
    let one = Complex64::ONE;
    // Columns of A: the phase values of unit zero-, positive- and
    // negative-sequence quantities. Rows of A^-1 (without 1/3): the
    // conjugates of those columns.
    let col = [[one, one, one], [one, a2, a], [one, a, a2]];
    let mut d = [Complex64::ZERO; 3];
    for (s, c) in col.iter().enumerate() {
        let mut acc = Complex64::ZERO;
        for i in 0..3 {
            for j in 0..3 {
                acc += c[i].conj() * z[i][j] * c[j];
            }
        }
        d[s] = acc / 3.0;
    }
    d
}

/// One column of `scan.csv` after `frequency_hz`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanColumn {
    /// `Zmag(<node>)`, `Zdeg(<node>)`, `Zmag(<node>;<node>)`, `Z1mag(<bus>)` ...
    /// Names contain no comma, so that the CSV header splits like its rows.
    pub name: String,
    /// "self", "mutual", "positive_sequence", "negative_sequence" or
    /// "zero_sequence".
    pub kind: &'static str,
    /// "ohm" or "deg".
    pub unit: &'static str,
    /// Node, "node;node" or bus.
    pub target: String,
}

/// Counts and times of a scan.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ScanStats {
    pub unknowns: usize,
    /// "dense" or "sparse".
    pub linear_solver: &'static str,
    /// Largest condition estimate of the factorizations (see RunStats).
    pub max_condition_estimate: f64,
    pub frequencies: usize,
    pub factorizations: u64,
    pub solves: u64,
    /// Frequencies at which the network equations are singular (the row has
    /// no values).
    pub singular_frequencies_hz: Vec<f64>,
    pub factorize_s: f64,
    pub solve_s: f64,
    pub output_s: f64,
    pub total_s: f64,
    #[serde(skip)]
    pub warnings: Vec<Diagnostic>,
}

/// Which complex quantity a column pair (magnitude, angle) shows.
enum Quantity {
    /// Z_bus entry between two injection nodes (indices into `injections`).
    Entry(usize, usize),
    /// Sequence impedance s (0, 1, 2) of a bus whose phase nodes are
    /// injections [a, b, c].
    Sequence([usize; 3], usize),
}

/// A frequency scan of a model, ready to run once.
pub struct FrequencyScanner {
    solver: Solver,
    elements: Vec<Box<dyn Element>>,
    labels: Vec<String>,
    frequencies: Vec<f64>,
    /// Unknowns of the nodes into which unit currents are injected.
    injections: Vec<usize>,
    quantities: Vec<Quantity>,
    columns: Vec<ScanColumn>,
    warnings: Vec<Diagnostic>,
}

impl std::fmt::Debug for FrequencyScanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrequencyScanner")
            .field("unknowns", &self.labels.len())
            .field("frequencies", &self.frequencies.len())
            .field("columns", &self.columns)
            .finish()
    }
}

impl FrequencyScanner {
    /// Builds the network of a model with a `frequency_scan`. The model is
    /// validated first; errors are returned as diagnostics.
    pub fn from_model(m: &Model) -> Result<FrequencyScanner, Vec<Diagnostic>> {
        let mut diagnostics = model::validate(m);
        let Some(scan) = &m.frequency_scan else {
            diagnostics.push(Diagnostic::error(
                "E190",
                "frequency_scan",
                "the model has no frequency_scan",
                "Add \"frequency_scan\": {\"start_hz\": 1, \"stop_hz\": 2500, \"points\": 2500, \
                 \"nodes\": [...]} with the nodes or buses to scan.",
            ));
            return Err(diagnostics);
        };
        if diagnostics.iter().any(|d| d.severity == Severity::Error) {
            return Err(diagnostics);
        }
        let net = netlist::build(m);
        // The validation has said which elements the scan takes as off
        // (W192), so that `validate` gives the warning before a scan does.
        debug_assert_eq!(
            diagnostics.iter().any(|d| d.code == "W192"),
            !net.two_state.is_empty(),
            "W192 of the validation names the elements with two states"
        );
        let warnings: Vec<Diagnostic> = diagnostics;

        let mut injections: Vec<usize> = Vec::new();
        let mut slot = |unknown: usize| -> usize {
            injections
                .iter()
                .position(|&u| u == unknown)
                .unwrap_or_else(|| {
                    injections.push(unknown);
                    injections.len() - 1
                })
        };
        let mut quantities = Vec::new();
        let mut columns = Vec::new();
        let pair = |quantities: &mut Vec<Quantity>,
                    columns: &mut Vec<ScanColumn>,
                    q: Quantity,
                    name: String,
                    kind: &'static str,
                    target: String| {
            quantities.push(q);
            columns.push(ScanColumn {
                name: name.replacen('Z', "Zmag", 1),
                kind,
                unit: "ohm",
                target: target.clone(),
            });
            columns.push(ScanColumn {
                name: name.replacen('Z', "Zdeg", 1),
                kind,
                unit: "deg",
                target,
            });
        };
        let node_slots: Vec<usize> = scan
            .nodes
            .iter()
            .map(|n| slot(net.node_index[n.as_str()]))
            .collect();
        for (i, a) in scan.nodes.iter().enumerate() {
            pair(
                &mut quantities,
                &mut columns,
                Quantity::Entry(node_slots[i], node_slots[i]),
                format!("Z({a})"),
                "self",
                a.clone(),
            );
        }
        for i in 0..scan.nodes.len() {
            for j in i + 1..scan.nodes.len() {
                let (a, b) = (&scan.nodes[i], &scan.nodes[j]);
                pair(
                    &mut quantities,
                    &mut columns,
                    Quantity::Entry(node_slots[i], node_slots[j]),
                    format!("Z({a};{b})"),
                    "mutual",
                    format!("{a};{b}"),
                );
            }
        }
        for bus in &scan.buses {
            let phases = BusRef::Bus(bus.clone()).nodes();
            let s = [
                slot(net.node_index[phases[0].as_str()]),
                slot(net.node_index[phases[1].as_str()]),
                slot(net.node_index[phases[2].as_str()]),
            ];
            for (seq, (prefix, kind)) in [
                ("Z1", "positive_sequence"),
                ("Z2", "negative_sequence"),
                ("Z0", "zero_sequence"),
            ]
            .into_iter()
            .enumerate()
            {
                // Order of the columns: 1, 2, 0; index into [Z0, Z1, Z2].
                let index = [1, 2, 0][seq];
                quantities.push(Quantity::Sequence(s, index));
                for (suffix, unit) in [("mag", "ohm"), ("deg", "deg")] {
                    columns.push(ScanColumn {
                        name: format!("{prefix}{suffix}({bus})"),
                        kind,
                        unit,
                        target: bus.clone(),
                    });
                }
            }
        }
        let solver = net.solver(m.simulation.linear_solver);
        Ok(FrequencyScanner {
            solver,
            elements: net.elements,
            labels: net.labels,
            frequencies: scan.frequencies(),
            injections,
            quantities,
            columns,
            warnings,
        })
    }

    pub fn columns(&self) -> &[ScanColumn] {
        &self.columns
    }

    pub fn frequencies(&self) -> &[f64] {
        &self.frequencies
    }

    /// Scans every frequency. `sink` receives the frequency and the values
    /// of every column (`None` where the equations are singular).
    pub fn run<F>(self, mut sink: F) -> Result<ScanStats, SimError>
    where
        F: FnMut(f64, &[Option<f64>]) -> std::io::Result<()>,
    {
        let started = Stopwatch::start();
        let n = self.labels.len();
        let k = self.injections.len();
        let mut stats = ScanStats {
            unknowns: n,
            linear_solver: self.solver.name(),
            frequencies: self.frequencies.len(),
            warnings: self.warnings,
            ..ScanStats::default()
        };
        let mut b = vec![Complex64::ZERO; n];
        let mut x = vec![Complex64::ZERO; n];
        // z[i][j]: voltage at injection node i per ampere into node j.
        let mut z = vec![vec![Complex64::ZERO; k]; k];
        let mut row = vec![None; self.columns.len()];
        for &f in &self.frequencies {
            let omega = 2.0 * PI * f;
            let t = Stopwatch::start();
            let lu = factor(&self.solver, &self.elements, omega);
            stats.factorizations += 1;
            stats.factorize_s += t.elapsed_s();
            let t = Stopwatch::start();
            let solved = match lu {
                Ok(lu) => {
                    stats.max_condition_estimate =
                        stats.max_condition_estimate.max(lu.condition_estimate());
                    let mut finite = true;
                    for j in 0..k {
                        b.fill(Complex64::ZERO);
                        b[self.injections[j]] = Complex64::ONE;
                        lu.solve(&b, &mut x);
                        stats.solves += 1;
                        for i in 0..k {
                            z[i][j] = x[self.injections[i]];
                        }
                        finite &= x.iter().all(|v| v.is_finite());
                    }
                    finite
                }
                Err(_) => false,
            };
            if solved {
                for (q, pair) in self.quantities.iter().zip(row.chunks_mut(2)) {
                    let v = match *q {
                        Quantity::Entry(i, j) => z[i][j],
                        Quantity::Sequence(s, index) => {
                            let mut m = [[Complex64::ZERO; 3]; 3];
                            for (r, &si) in s.iter().enumerate() {
                                for (c, &sj) in s.iter().enumerate() {
                                    m[r][c] = z[si][sj];
                                }
                            }
                            sequence_diagonal(&m)[index]
                        }
                    };
                    pair[0] = Some(v.norm());
                    pair[1] = Some(v.arg().to_degrees());
                }
            } else {
                row.fill(None);
                stats.singular_frequencies_hz.push(f);
            }
            stats.solve_s += t.elapsed_s();
            let t = Stopwatch::start();
            sink(f, &row).map_err(SimError::Output)?;
            stats.output_s += t.elapsed_s();
        }
        if !stats.singular_frequencies_hz.is_empty() {
            let list: Vec<String> = stats
                .singular_frequencies_hz
                .iter()
                .take(10)
                .map(|f| format!("{f}"))
                .collect();
            stats.warnings.push(Diagnostic::warning(
                "W193",
                "frequency_scan",
                format!(
                    "the network equations are singular at {} frequencies ({} Hz{}); their rows are empty",
                    stats.singular_frequencies_hz.len(),
                    list.join(", "),
                    if stats.singular_frequencies_hz.len() > 10 { ", ..." } else { "" }
                ),
                "A lossless resonance at exactly a scanned frequency has infinite impedance. Add a \
                 small resistance, or move the frequencies.",
            ));
        }
        stats.total_s = started.elapsed_s();
        Ok(stats)
    }
}
