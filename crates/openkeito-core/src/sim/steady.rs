//! The steady state a run starts from, and the comparison of the run with
//! it.

use crate::clock::Stopwatch;

use openkeito_model::Diagnostic;

use super::{PhasorValue, Probe, RunStats, SimError, Simulator, SteadyStateReport};
use crate::linalg::{Complex64, Solver, Stamp};
use crate::netlist::Term;

/// A relative departure from the steady state above this means the time
/// step is too large for the frequency (or the steady state is wrong).
const DEVIATION_WARNING: f64 = 1e-3;

/// Steady state of the run and the comparison with it.
pub(super) struct Steady {
    omega: f64,
    /// Phasor of every unknown.
    x: Vec<Complex64>,
    /// Phasor of every output column; `None` for signals.
    probes: Vec<Option<Complex64>>,
    /// Output columns that are currents.
    current_columns: Vec<usize>,
    voltage_amplitude: f64,
    current_amplitude: f64,
    /// False from the first switching action on: the circuit has left the
    /// steady state.
    pub active: bool,
    checked_until: f64,
    max_dv: f64,
    max_di: f64,
}

impl Steady {
    /// Compares the solution at time t with the steady state.
    pub(super) fn check(&mut self, t: f64, x: &[f64], row: &[f64], voltage_unknowns: &[usize]) {
        if !self.active {
            return;
        }
        let (c, s) = ((self.omega * t).cos(), (self.omega * t).sin());
        let at = |p: Complex64| std::f64::consts::SQRT_2 * (p.re * c - p.im * s);
        for &i in voltage_unknowns {
            self.max_dv = self.max_dv.max((x[i] - at(self.x[i])).abs());
        }
        for &k in &self.current_columns {
            if let Some(p) = self.probes[k] {
                self.max_di = self.max_di.max((row[k] - at(p)).abs());
            }
        }
        self.checked_until = t;
    }
}

impl Simulator {
    /// Solves the phasor equations at `frequency_hz`, sets every state to
    /// its value at t = 0 and returns the steady state to compare with.
    pub(super) fn steady_state(
        &mut self,
        frequency_hz: f64,
        stats: &mut RunStats,
    ) -> Result<Steady, SimError> {
        let started = Stopwatch::start();
        let omega = 2.0 * std::f64::consts::PI * frequency_hz;
        let n = self.labels.len();
        let singular = |k: usize| SimError::PhasorSingular {
            frequency_hz,
            unknown: self.labels[k].clone(),
        };
        // The steady state joins the parts through the lines: one matrix
        // for the whole circuit.
        let elements = &self.elements;
        let excluded = &self.steady_state_excluded;
        let layout = &self.layout;
        if let Some(k) = elements
            .iter()
            .enumerate()
            .filter(|(i, _)| !excluded[*i])
            .find_map(|(i, e)| e.phasor_singular(omega).map(|k| layout.unknown(i, k)))
        {
            return Err(singular(k));
        }
        struct TrackStamp<'a> {
            inner: Option<&'a mut dyn Stamp<f64>>,
            touched: &'a mut [bool],
        }
        impl Stamp<f64> for TrackStamp<'_> {
            fn add(&mut self, row: usize, col: usize, value: f64) {
                self.touched[row] = true;
                self.touched[col] = true;
                if let Some(inner) = self.inner.as_mut() {
                    inner.add(row, col, value);
                }
            }
        }
        let mut touched = vec![false; n];
        let is_sparse = self.linear_solver.is_sparse(n);
        let solver = if is_sparse {
            Solver::new(true, n, |s| {
                let mut track = TrackStamp {
                    inner: Some(s),
                    touched: &mut touched,
                };
                layout.assemble_indexed(elements, &mut track, |idx, e, m| {
                    if !excluded[idx] {
                        e.structure(m);
                    }
                });
                for (u, &seen) in touched.iter().enumerate() {
                    if !seen {
                        s.add(u, u, 1.0);
                    }
                }
            })
        } else {
            let mut track = TrackStamp {
                inner: None,
                touched: &mut touched,
            };
            layout.assemble_indexed(elements, &mut track, |idx, e, m| {
                if !excluded[idx] {
                    e.structure(m);
                }
            });
            Solver::new(false, n, |_| {})
        };
        let lu = solver
            .factor(|m| {
                layout.assemble_indexed(elements, m, |idx, e, m| {
                    if !excluded[idx] {
                        e.stamp_phasor(omega, m);
                    }
                });
                for (u, &seen) in touched.iter().enumerate() {
                    if !seen {
                        m.add(u, u, Complex64::new(1.0, 0.0));
                    }
                }
            })
            .map_err(singular)?;
        stats.max_condition_estimate = stats.max_condition_estimate.max(lu.condition_estimate());
        let mut b = vec![Complex64::ZERO; n];
        for part in &layout.parts {
            let b = &mut b[part.unknowns.clone()];
            for idx in part.elements.clone() {
                if !excluded[idx] {
                    elements[idx].rhs_phasor(omega, b);
                }
            }
        }
        for link in &layout.links {
            if !excluded[link.element] {
                let mut at_ports = vec![Complex64::ZERO; link.ports.len()];
                elements[link.element].rhs_phasor(omega, &mut at_ports);
                for (v, &unknown) in at_ports.iter().zip(&link.ports) {
                    b[unknown] += *v;
                }
            }
        }
        let mut x = vec![Complex64::ZERO; n];
        lu.solve(&b, &mut x);
        if let Some(k) = x.iter().position(|v| !v.is_finite()) {
            return Err(singular(k));
        }
        for i in 0..self.elements.len() {
            if !excluded[i] {
                let e = &mut self.elements[i];
                self.layout.with(i, &x, |x| e.init_from_phasor(omega, x));
            }
        }
        stats.warnings.extend(
            self.elements
                .iter()
                .enumerate()
                .filter(|(i, _)| !excluded[*i])
                .filter_map(|(_, e)| e.steady_state_warning()),
        );
        let probe_phasor = |p: &Probe| match p {
            Probe::Terms(terms) => Some(terms.iter().fold(Complex64::ZERO, |acc, term| {
                acc + match *term {
                    Term::Unknown(i, c) => c * x[i],
                    Term::Current(e, k, c) => {
                        if excluded[e] {
                            Complex64::ZERO
                        } else {
                            c * self
                                .layout
                                .with(e, &x, |x| self.elements[e].current_phasor(k, omega, x))
                        }
                    }
                }
            })),
            Probe::Signal(_) => None,
        };
        let probes: Vec<Option<Complex64>> = self.probes.iter().map(probe_phasor).collect();
        let current_columns: Vec<usize> = (0..self.columns.len())
            .filter(|&k| self.columns[k].kind == "branch_current")
            .collect();
        let largest = |v: &mut dyn Iterator<Item = Complex64>| {
            std::f64::consts::SQRT_2 * v.fold(0.0_f64, |a, p| a.max(p.norm()))
        };
        let voltage_amplitude = largest(&mut self.voltage_unknowns.iter().map(|&i| x[i]));
        let current_amplitude = largest(&mut current_columns.iter().filter_map(|&k| probes[k]));
        stats.factorizations += 1;
        stats.solves += 1;
        stats.timing.factorize_s += started.elapsed_s();
        Ok(Steady {
            omega,
            x,
            probes,
            current_columns,
            voltage_amplitude,
            current_amplitude,
            active: true,
            checked_until: 0.0,
            max_dv: 0.0,
            max_di: 0.0,
        })
    }

    /// The report of the steady state: phasors and the departure from them.
    pub(super) fn steady_report(&self, steady: &Steady, frequency_hz: f64) -> SteadyStateReport {
        let mut phasors: Vec<PhasorValue> = self
            .named_nodes
            .iter()
            .map(|(n, i)| PhasorValue {
                name: format!("V({n})"),
                unit: "V",
                value: steady.x[*i],
            })
            .collect();
        for (c, p) in self.columns.iter().zip(&steady.probes) {
            if let Some(p) = p {
                if !phasors.iter().any(|q| q.name == c.name) {
                    phasors.push(PhasorValue {
                        name: c.name.clone(),
                        unit: c.unit,
                        value: *p,
                    });
                }
            }
        }
        let relative = |d: f64, amplitude: f64| if amplitude > 0.0 { d / amplitude } else { 0.0 };
        let has_currents = !steady.current_columns.is_empty();
        SteadyStateReport {
            frequency_hz,
            checked_until_s: steady.checked_until,
            max_voltage_deviation_v: steady.max_dv,
            relative_voltage_deviation: relative(steady.max_dv, steady.voltage_amplitude),
            max_current_deviation_a: has_currents.then_some(steady.max_di),
            relative_current_deviation: has_currents
                .then(|| relative(steady.max_di, steady.current_amplitude)),
            phasors,
        }
    }

    /// Adds the report of the steady state to the statistics, with a warning
    /// when the waveforms departed from it.
    pub(super) fn report_steady(&self, steady: &Steady, frequency_hz: f64, stats: &mut RunStats) {
        let (s, f) = (steady, frequency_hz);
        let report = self.steady_report(s, f);
        let worst = report
            .relative_voltage_deviation
            .max(report.relative_current_deviation.unwrap_or(0.0));
        if worst > DEVIATION_WARNING {
            stats.warnings.push(Diagnostic::warning(
                "W220",
                "simulation.initialization",
                format!(
                    "the waveforms departed from the steady state by {worst:.2e} of their \
                     amplitude before t = {:e} s",
                    report.checked_until_s
                ),
                "Reduce time_step_s (the departure of a second-order method grows with \
                 (2 pi f time_step_s)^2), and check that the sources and initial switch states \
                 are the intended ones. With 2S-DIRK, the current of a capacitor connected \
                 directly across a voltage source (or the voltage of an inductor in series with \
                 a current source) is of first order, about 0.35 (2 pi f time_step_s): add a \
                 source impedance or use the trapezoidal rule.",
            ));
        }
        stats.initialization = Some(report);
    }
}
