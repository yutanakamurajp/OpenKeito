//! Lines: the pi line, the travelling-wave (Bergeron) line and the line
//! parameters per km.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::three_phase::{check_series_buses, phase_pairs, sequence_matrix, BusRef, PhaseMatrix};
use super::{ComponentInitialization, ComponentSpec};
use crate::catalog::{self, p, ComponentInfo};
use crate::check::{check_matrix, positive, range_error, sequence_values};
use crate::diagnostic::Diagnostic;
use crate::model::{Simulation, GROUND};

/// Parameters of a line per km of length.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LineParameters {
    /// Positive- and zero-sequence values of a transposed line: resistances
    /// at least 0, inductances and capacitances above 0. The capacitances are
    /// those of the sequence networks (phase to ground for zero sequence).
    Sequence {
        r1_ohm_per_km: f64,
        l1_h_per_km: f64,
        c1_f_per_km: f64,
        r0_ohm_per_km: f64,
        l0_h_per_km: f64,
        c0_f_per_km: f64,
    },
    /// Phase matrices, rows and columns in the order a, b, c, all symmetric.
    /// The capacitance matrix is the nodal (Maxwell) matrix: i = C dv/dt with
    /// v the phase-to-ground voltages, so its diagonal is positive and its
    /// off-diagonal entries are negative or zero. L and C positive definite,
    /// R positive semidefinite.
    Matrix {
        resistance_ohm_per_km: [[f64; 3]; 3],
        inductance_h_per_km: [[f64; 3]; 3],
        capacitance_f_per_km: [[f64; 3]; 3],
    },
}

impl LineParameters {
    /// (R, L, C) phase matrices per km.
    pub fn matrices(&self) -> (PhaseMatrix, PhaseMatrix, PhaseMatrix) {
        match self {
            LineParameters::Sequence {
                r1_ohm_per_km,
                l1_h_per_km,
                c1_f_per_km,
                r0_ohm_per_km,
                l0_h_per_km,
                c0_f_per_km,
            } => (
                sequence_matrix(*r1_ohm_per_km, *r0_ohm_per_km),
                sequence_matrix(*l1_h_per_km, *l0_h_per_km),
                sequence_matrix(*c1_f_per_km, *c0_f_per_km),
            ),
            LineParameters::Matrix {
                resistance_ohm_per_km,
                inductance_h_per_km,
                capacitance_f_per_km,
            } => (
                *resistance_ohm_per_km,
                *inductance_h_per_km,
                *capacitance_f_per_km,
            ),
        }
    }
}

pub(crate) fn check_line_parameters(
    l: &LineParameters,
    path: &str,
    name: &str,
    d: &mut Vec<Diagnostic>,
) {
    match l {
        LineParameters::Sequence {
            r1_ohm_per_km,
            l1_h_per_km,
            c1_f_per_km,
            r0_ohm_per_km,
            l0_h_per_km,
            c0_f_per_km,
        } => {
            sequence_values(
                path,
                name,
                &[
                    ("r1_ohm_per_km", *r1_ohm_per_km),
                    ("r0_ohm_per_km", *r0_ohm_per_km),
                ],
                false,
                d,
            );
            sequence_values(
                path,
                name,
                &[
                    ("l1_h_per_km", *l1_h_per_km),
                    ("l0_h_per_km", *l0_h_per_km),
                    ("c1_f_per_km", *c1_f_per_km),
                    ("c0_f_per_km", *c0_f_per_km),
                ],
                true,
                d,
            );
        }
        LineParameters::Matrix {
            resistance_ohm_per_km,
            inductance_h_per_km,
            capacitance_f_per_km,
        } => {
            check_matrix(
                path,
                name,
                "resistance_ohm_per_km",
                resistance_ohm_per_km,
                false,
                d,
            );
            check_matrix(
                path,
                name,
                "inductance_h_per_km",
                inductance_h_per_km,
                true,
                d,
            );
            check_matrix(
                path,
                name,
                "capacitance_f_per_km",
                capacitance_f_per_km,
                true,
                d,
            );
        }
    }
}

fn one_section() -> u32 {
    1
}

/// Three-phase line as `sections` cascaded pi sections. Each section has the
/// series coupled R and L of length_km/sections and half of its shunt
/// capacitance to ground at each end; the half capacitances of adjacent
/// sections add up.
///
/// The terminal currents I(<name>.0.a) and I(<name>.1.a) flow from the bus
/// into the line at buses[0] and buses[1].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PiLine {
    pub name: String,
    pub buses: [BusRef; 2],
    /// Must be > 0.
    pub length_km: f64,
    /// Number of pi sections, >= 1. Default 1.
    #[serde(default = "one_section")]
    pub sections: u32,
    pub per_km: LineParameters,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

/// Largest number of pi sections of one line.
const MAX_SECTIONS: u32 = 10_000;

impl ComponentSpec for PiLine {
    fn current_suffixes(&self) -> Option<Vec<String>> {
        Some(super::phase_suffixes(&[".0", ".1"]))
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "pi_line"
    }
    fn since(&self) -> &'static str {
        "0.3"
    }
    fn node_names(&self) -> Vec<String> {
        self.buses.iter().flat_map(BusRef::nodes).collect()
    }
    fn terminals(&self) -> Vec<super::Terminal> {
        super::series_terminals(&self.buses)
    }
    fn check_terminals(&self, p: &str, d: &mut Vec<Diagnostic>) {
        check_series_buses(&self.name, &self.buses, p, d);
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        let (l, name) = (self, self.name.as_str());
        if !positive(l.length_km) {
            range_error(
                d,
                p,
                name,
                "length_km",
                l.length_km,
                "Length must be > 0 km.",
            );
        }
        if l.sections == 0 || l.sections > MAX_SECTIONS {
            range_error(
                d,
                p,
                name,
                "sections",
                l.sections as f64,
                &format!(
                "Use 1 to {MAX_SECTIONS} sections. More sections approach the distributed line; \
                 for long lines a section per 10 to 30 km is common."
            ),
            );
        }
        check_line_parameters(&l.per_km, &format!("{p}.per_km"), name, d);
    }
    fn connections(&self) -> Vec<(String, String)> {
        // Series branches and the shunt capacitances to ground.
        let mut v = phase_pairs(&self.buses);
        v.extend(
            self.buses[0]
                .nodes()
                .into_iter()
                .map(|n| (n, GROUND.to_string())),
        );
        v
    }
    // A pi line is connected to gnd through its capacitances without naming
    // it.
    fn grounds_implicitly(&self) -> bool {
        true
    }
    fn series_impedance(&self) -> Option<(PhaseMatrix, PhaseMatrix, &'static str)> {
        let (r, l, _) = self.per_km.matrices();
        Some((r, l, "per_km"))
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "pi_line",
            category: "circuit",
            summary: "Three-phase line as cascaded pi sections: each section has the coupled series R and L of length_km/sections and half its shunt capacitance to ground at each end.",
            sign_convention: "Terminal currents I(<name>.0.a) etc. flow from the bus into the line at buses[0], I(<name>.1.a) etc. at buses[1]. Branch voltages U(<name>.a) are V(buses[0].p) - V(buses[1].p).",
            parameters: vec![
                catalog::name(),
                p("buses", "", true, "[from, to]; each a bus or three node names. See bus."),
                p("length_km", "km", true, "Length, > 0."),
                p("sections", "", false, "Number of pi sections, 1 to 10000. Default 1."),
                p("per_km", "ohm/km, H/km, F/km", true, "{\"type\":\"sequence\",\"r1_ohm_per_km\",\"l1_h_per_km\",\"c1_f_per_km\",\"r0_ohm_per_km\",\"l0_h_per_km\",\"c0_f_per_km\"} (sequence capacitances; zero sequence phase to ground) | {\"type\":\"matrix\",\"resistance_ohm_per_km\":[[3x3]],\"inductance_h_per_km\":[[3x3]],\"capacitance_f_per_km\":[[3x3]]} (nodal capacitance matrix: positive diagonal, negative off-diagonal)."),
            ],
            example: json!({"type": "pi_line", "name": "LINE1", "buses": ["src", "b2"], "length_km": 50.0, "sections": 2,
                "per_km": {"type": "sequence", "r1_ohm_per_km": 0.03, "l1_h_per_km": 0.0009, "c1_f_per_km": 1.25e-8, "r0_ohm_per_km": 0.2, "l0_h_per_km": 0.003, "c0_f_per_km": 6.0e-9}}),
        }
    }
}

/// One travelling-wave mode of a line of a given length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineMode {
    /// Characteristic impedance sqrt(L/C) of the mode in ohm.
    pub surge_impedance_ohm: f64,
    /// Travel time length * sqrt(L C) of the mode in seconds.
    pub travel_time_s: f64,
    /// Series resistance of the mode over the whole length in ohm.
    pub resistance_ohm: f64,
}

/// The decomposition of a three-phase line into three modes that travel
/// independently of each other.
#[derive(Debug, Clone, PartialEq)]
pub struct LineModes {
    pub modes: [LineMode; 3],
    /// Row k gives the voltage of mode k from the phase voltages:
    /// v_mode = W v_phase. The phase currents follow from the mode currents
    /// with the transpose, i_phase = W^T i_mode, so power is the same in both
    /// descriptions. The rows have length 1.
    pub transform: PhaseMatrix,
    /// What the modes leave out: the largest |R_jk| / sqrt(R_jj R_kk) of the
    /// resistance matrix in modal quantities, 0 for a balanced line.
    pub resistance_coupling: f64,
}

/// Modes with travel times closer than this (relative) count as one
/// eigenvalue: within their subspace the modes are chosen so that the
/// resistance matrix is diagonal too.
const SAME_VELOCITY: f64 = 1e-9;

fn mat_mul(a: &PhaseMatrix, b: &PhaseMatrix) -> PhaseMatrix {
    let mut c = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            c[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    c
}

fn transposed(a: &PhaseMatrix) -> PhaseMatrix {
    let mut t = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            t[i][j] = a[j][i];
        }
    }
    t
}

/// Lower triangular S with m = S S^T; `None` when m is not positive
/// definite.
fn cholesky(m: &PhaseMatrix) -> Option<PhaseMatrix> {
    let mut s = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..=i {
            let sum: f64 = (0..j).map(|k| s[i][k] * s[j][k]).sum();
            if i == j {
                let d = m[i][i] - sum;
                if !(d.is_finite() && d > 0.0) {
                    return None;
                }
                s[i][i] = d.sqrt();
            } else {
                s[i][j] = (m[i][j] - sum) / s[j][j];
            }
        }
    }
    Some(s)
}

/// Inverse of a lower triangular matrix.
fn lower_inverse(s: &PhaseMatrix) -> PhaseMatrix {
    let mut inv = [[0.0; 3]; 3];
    for col in 0..3 {
        for i in col..3 {
            let unit = if i == col { 1.0 } else { 0.0 };
            let sum: f64 = (col..i).map(|k| s[i][k] * inv[k][col]).sum();
            inv[i][col] = (unit - sum) / s[i][i];
        }
    }
    inv
}

/// Eigenvalues and eigenvectors (the columns of the second matrix) of a
/// symmetric matrix by cyclic Jacobi rotations [Golub, Van Loan]. Entries
/// that are exactly zero are left alone, so a block structure is kept.
fn jacobi(m: &PhaseMatrix) -> ([f64; 3], PhaseMatrix) {
    let mut a = *m;
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _sweep in 0..60 {
        let off = a[0][1] * a[0][1] + a[0][2] * a[0][2] + a[1][2] * a[1][2];
        let diagonal = a[0][0] * a[0][0] + a[1][1] * a[1][1] + a[2][2] * a[2][2];
        if off <= 1e-32 * diagonal {
            break;
        }
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            if a[p][q] == 0.0 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let sign = if theta < 0.0 { -1.0 } else { 1.0 };
            let t = sign / (theta.abs() + (theta * theta + 1.0).sqrt());
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            for row in a.iter_mut() {
                let (x, y) = (row[p], row[q]);
                row[p] = c * x - s * y;
                row[q] = s * x + c * y;
            }
            for k in 0..3 {
                let (x, y) = (a[p][k], a[q][k]);
                a[p][k] = c * x - s * y;
                a[q][k] = s * x + c * y;
            }
            // The rotation makes this pair zero; write it so.
            a[p][q] = 0.0;
            a[q][p] = 0.0;
            for row in v.iter_mut() {
                let (x, y) = (row[p], row[q]);
                row[p] = c * x - s * y;
                row[q] = s * x + c * y;
            }
        }
    }
    ([a[0][0], a[1][1], a[2][2]], v)
}

/// The Clarke transformation with rows of length 1: the zero-sequence mode
/// and two line modes [Clarke].
fn clarke() -> PhaseMatrix {
    let (r3, r6, r2) = (3.0_f64.sqrt(), 6.0_f64.sqrt(), 2.0_f64.sqrt());
    [
        [1.0 / r3, 1.0 / r3, 1.0 / r3],
        [2.0 / r6, -1.0 / r6, -1.0 / r6],
        [0.0, 1.0 / r2, -1.0 / r2],
    ]
}

/// (L, C, R) per km of the three modes of phase matrices, the rows of the
/// transformation and the coupling that the modes leave out.
///
/// With L = S S^T (Cholesky) and S^T C S = Q diag(lambda) Q^T, the rows of
/// W = Q^T S^-1 are left eigenvectors of L C: W L W^T = 1 and
/// W^-T C W^-1 = diag(lambda). L and C are symmetric and positive definite,
/// so the eigenvalues are real and positive whatever the line. The modes are
/// then scaled to rows of length 1 and ordered by travel time, the slowest
/// first.
#[allow(clippy::type_complexity)]
fn matrix_modes(
    r: &PhaseMatrix,
    l: &PhaseMatrix,
    c: &PhaseMatrix,
) -> Option<([(f64, f64, f64); 3], PhaseMatrix, f64)> {
    let s = cholesky(l)?;
    let s_inv = lower_inverse(&s);
    let m = mat_mul(&mat_mul(&transposed(&s), c), &s);
    // Rounding leaves m symmetric to a few ulp only; make it exact.
    let mut symmetric = m;
    for i in 0..3 {
        for j in 0..i {
            symmetric[i][j] = 0.5 * (m[i][j] + m[j][i]);
            symmetric[j][i] = symmetric[i][j];
        }
    }
    let (lambda, mut q) = jacobi(&symmetric);
    if !lambda.iter().all(|v| v.is_finite() && *v > 0.0) {
        return None;
    }
    let largest = lambda.iter().fold(0.0_f64, |a, v| a.max(*v));
    // R in the quantities of S: B = S^-1 R S^-T.
    let b = mat_mul(&mat_mul(&s_inv, r), &transposed(&s_inv));
    let modal = |q: &PhaseMatrix| mat_mul(&mat_mul(&transposed(q), &b), q);
    // Within modes of one velocity any basis diagonalizes L C: take the one
    // that diagonalizes R there as well.
    let present = modal(&q);
    let mut block = [[0.0; 3]; 3];
    let mut degenerate = false;
    for i in 0..3 {
        block[i][i] = present[i][i];
        for j in 0..i {
            if (lambda[i] - lambda[j]).abs() <= SAME_VELOCITY * largest {
                degenerate = true;
                block[i][j] = 0.5 * (present[i][j] + present[j][i]);
                block[j][i] = block[i][j];
            }
        }
    }
    if degenerate {
        let (_, u) = jacobi(&block);
        q = mat_mul(&q, &u);
    }
    let r_modal = modal(&q);
    let r_scale = r_modal[0][0].max(r_modal[1][1]).max(r_modal[2][2]);
    let mut coupling = 0.0_f64;
    for i in 0..3 {
        for j in 0..i {
            let own = (r_modal[i][i] * r_modal[j][j]).max(0.0).sqrt();
            let off = r_modal[i][j].abs();
            if own > 1e-12 * r_scale {
                coupling = coupling.max(off / own);
            } else if off > 1e-12 * r_scale {
                coupling = 1.0;
            }
        }
    }
    // v_mode = Q^T S^-1 v_phase, each row scaled to length 1: a row divided
    // by its length n has L = 1/n^2, C = lambda n^2 and R = r/n^2.
    let w = mat_mul(&transposed(&q), &s_inv);
    let mut order = [0, 1, 2];
    order.sort_by(|&i, &j| lambda[j].total_cmp(&lambda[i]));
    let mut values = [(0.0, 0.0, 0.0); 3];
    let mut transform = [[0.0; 3]; 3];
    for (k, &i) in order.iter().enumerate() {
        let n2: f64 = w[i].iter().map(|v| v * v).sum();
        let n = n2.sqrt();
        values[k] = (1.0 / n2, lambda[i] * n2, r_modal[i][i].max(0.0) / n2);
        transform[k] = w[i].map(|v| v / n);
    }
    Some((values, transform, coupling))
}

/// The modes of a line of `length_km`. `None` when the parameters are not
/// those of a line (they are reported by the checks of the parameters).
///
/// Sequence values: the Clarke transformation; the zero-sequence mode comes
/// first, then two line modes with the positive-sequence values. Matrices:
/// the modes of L C (`matrix_modes`). The resistance in modal quantities,
/// W R W^T, is diagonal only for some lines; `resistance_coupling` says how
/// far it is from that.
pub fn line_modes(per_km: &LineParameters, length_km: f64) -> Option<LineModes> {
    let ok = |v: f64| v.is_finite() && v > 0.0;
    if !ok(length_km) {
        return None;
    }
    let (values, transform, resistance_coupling) = match per_km {
        LineParameters::Sequence {
            r1_ohm_per_km,
            l1_h_per_km,
            c1_f_per_km,
            r0_ohm_per_km,
            l0_h_per_km,
            c0_f_per_km,
        } => {
            let zero = (*l0_h_per_km, *c0_f_per_km, *r0_ohm_per_km);
            let line = (*l1_h_per_km, *c1_f_per_km, *r1_ohm_per_km);
            ([zero, line, line], clarke(), 0.0)
        }
        LineParameters::Matrix {
            resistance_ohm_per_km,
            inductance_h_per_km,
            capacitance_f_per_km,
        } => matrix_modes(
            resistance_ohm_per_km,
            inductance_h_per_km,
            capacitance_f_per_km,
        )?,
    };
    let mut modes = [LineMode {
        surge_impedance_ohm: 0.0,
        travel_time_s: 0.0,
        resistance_ohm: 0.0,
    }; 3];
    for (mode, (l, c, r)) in modes.iter_mut().zip(values) {
        if !(ok(l) && ok(c) && r.is_finite() && r >= 0.0) {
            return None;
        }
        *mode = LineMode {
            surge_impedance_ohm: (l / c).sqrt(),
            travel_time_s: length_km * (l * c).sqrt(),
            resistance_ohm: r * length_km,
        };
        if !(ok(mode.surge_impedance_ohm) && ok(mode.travel_time_s)) {
            return None;
        }
    }
    Some(LineModes {
        modes,
        transform,
        resistance_coupling,
    })
}

/// How far the travel time of a mode may be below the time step and still
/// count as one step (the two are decimal numbers, their quotient is
/// rounded). The solver takes a quotient this close to a whole number as
/// that number.
pub const TRAVEL_TIME_TOLERANCE: f64 = 1e-9;

/// Above this coupling between two modes of the resistance matrix a line
/// cannot be a `bergeron_line` (E231), and above `COUPLING_WARNING` the
/// model gets a warning (W231).
const COUPLING_LIMIT: f64 = 0.1;
const COUPLING_WARNING: f64 = 0.01;

/// Three-phase line with distributed, constant parameters: travelling waves
/// (Bergeron's method). The line is split into three modes; each is a
/// lossless line with the characteristic impedance sqrt(L/C) and the travel
/// time length * sqrt(L C), with its series resistance lumped as R/4 at each
/// end and R/2 in the middle. Sequence values give the zero-sequence mode
/// and two line modes; matrices give the modes of L C.
///
/// What happens at one end reaches the other end one travel time later, so
/// every mode needs a travel time of at least one time step; a shorter line
/// is a `pi_line`.
///
/// The terminal currents I(<name>.0.a) and I(<name>.1.a) flow from the bus
/// into the line at buses[0] and buses[1].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BergeronLine {
    pub name: String,
    pub buses: [BusRef; 2],
    /// Must be > 0.
    pub length_km: f64,
    pub per_km: LineParameters,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl BergeronLine {
    /// The modes of the line; `None` when its parameters are not valid.
    pub fn modes(&self) -> Option<LineModes> {
        line_modes(&self.per_km, self.length_km)
    }

    /// What the modes are called in messages.
    fn mode_names(&self) -> [&'static str; 3] {
        match self.per_km {
            LineParameters::Sequence { .. } => ["zero-sequence mode", "line modes", "line modes"],
            LineParameters::Matrix { .. } => ["mode 1", "mode 2", "mode 3"],
        }
    }

    /// The modes to name in a message: the two line modes of sequence values
    /// are one.
    fn distinct_modes(&self) -> &'static [usize] {
        match self.per_km {
            LineParameters::Sequence { .. } => &[0, 1],
            LineParameters::Matrix { .. } => &[0, 1, 2],
        }
    }
}

impl ComponentSpec for BergeronLine {
    fn current_suffixes(&self) -> Option<Vec<String>> {
        Some(super::phase_suffixes(&[".0", ".1"]))
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "bergeron_line"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn node_names(&self) -> Vec<String> {
        self.buses.iter().flat_map(BusRef::nodes).collect()
    }
    fn terminals(&self) -> Vec<super::Terminal> {
        super::series_terminals(&self.buses)
    }
    fn check_terminals(&self, p: &str, d: &mut Vec<Diagnostic>) {
        check_series_buses(&self.name, &self.buses, p, d);
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        let name = self.name.as_str();
        let before = d.len();
        if !positive(self.length_km) {
            range_error(
                d,
                p,
                name,
                "length_km",
                self.length_km,
                "Length must be > 0 km.",
            );
        }
        check_line_parameters(&self.per_km, &format!("{p}.per_km"), name, d);
        if d.len() > before {
            return;
        }
        let Some(modes) = self.modes() else {
            return;
        };
        let coupling = modes.resistance_coupling;
        if coupling > COUPLING_WARNING {
            let message = format!(
                "the resistance matrix of \"{name}\" couples the modes of its inductance and \
                 capacitance matrices: the largest mutual resistance between two modes is \
                 {coupling:.3} of their own resistances (geometric mean)"
            );
            if coupling > COUPLING_LIMIT {
                d.push(Diagnostic::error(
                    "E231",
                    format!("{p}.per_km.resistance_ohm_per_km"),
                    message,
                    format!(
                        "A bergeron_line lets every mode travel on its own and would drop this \
                         coupling; it accepts up to {COUPLING_LIMIT}. Give the line by its \
                         positive- and zero-sequence values ({{\"type\": \"sequence\", ...}}, a \
                         transposed line), or use a pi_line, which keeps the full matrices."
                    ),
                ));
                return;
            }
            d.push(Diagnostic::warning(
                "W231",
                format!("{p}.per_km.resistance_ohm_per_km"),
                message,
                "The bergeron_line drops this coupling: the damping of the waves is off by about \
                 that fraction. Give the line by its positive- and zero-sequence values, or use \
                 a pi_line, if that matters.",
            ));
        }
        let names = self.mode_names();
        let lossy: Vec<String> = self
            .distinct_modes()
            .iter()
            .filter(|&&k| modes.modes[k].resistance_ohm > modes.modes[k].surge_impedance_ohm)
            .map(|&k| {
                format!(
                    "{} ({:.4e} ohm against {:.4e} ohm)",
                    names[k], modes.modes[k].resistance_ohm, modes.modes[k].surge_impedance_ohm
                )
            })
            .collect();
        if !lossy.is_empty() {
            d.push(Diagnostic::warning(
                "W230",
                format!("{p}.length_km"),
                format!(
                    "the series resistance of \"{name}\" over its length is above the \
                     characteristic impedance: {}",
                    lossy.join(", ")
                ),
                "The bergeron_line lumps the resistance at the ends and in the middle, which is \
                 accurate to about (R/Zc)^2/8. Split the line into several bergeron_line \
                 components in series (the error falls with their number), or use a pi_line \
                 with sections.",
            ));
        }
    }
    fn check_simulation(&self, sim: &Simulation, p: &str, d: &mut Vec<Diagnostic>) {
        // Parameters that are wrong are reported on their own.
        let own = format!("{p}.");
        if d.iter().any(|e| e.path.starts_with(&own)) {
            return;
        }
        let Some(modes) = self.modes() else {
            return;
        };
        let h = sim.time_step_s;
        let names = self.mode_names();
        let short: Vec<String> = self
            .distinct_modes()
            .iter()
            .filter(|&&k| modes.modes[k].travel_time_s < h * (1.0 - TRAVEL_TIME_TOLERANCE))
            .map(|&k| format!("{} {:e} s", names[k], modes.modes[k].travel_time_s))
            .collect();
        if short.is_empty() {
            return;
        }
        let shortest = modes
            .modes
            .iter()
            .fold(f64::INFINITY, |a, m| a.min(m.travel_time_s));
        d.push(Diagnostic::error(
            "E230",
            format!("{p}.length_km"),
            format!(
                "\"{}\" is too short for time_step_s = {h:e} s: travel time of the {}",
                self.name,
                short.join(", of the ")
            ),
            format!(
                "A bergeron_line needs a travel time of at least one time step in every mode. \
                 Model this line as a pi_line (same buses, length_km and per_km), or set \
                 time_step_s to {shortest:e} or less."
            ),
        ));
    }
    fn connections(&self) -> Vec<(String, String)> {
        // In the equations of a time step every end is a conductance matrix
        // to gnd (the characteristic impedances) and a current source; the
        // two ends are not connected to each other. With phasors the line
        // has an admittance to gnd at every frequency above 0 as well.
        self.node_names()
            .into_iter()
            .map(|n| (n, GROUND.to_string()))
            .collect()
    }
    fn grounds_implicitly(&self) -> bool {
        true
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "bergeron_line",
            category: "circuit",
            summary: "Three-phase line with distributed constant parameters (travelling waves, Bergeron's method): three modes, each a lossless line with Zc = sqrt(L/C) and travel time length*sqrt(L*C), its resistance lumped as R/4 at each end and R/2 in the middle. Every mode needs a travel time of at least one time step (E230); a shorter line is a pi_line. A travel time that is not a whole number of time steps is interpolated linearly, which smooths steep fronts.",
            sign_convention: "Terminal currents I(<name>.0.a) etc. flow from the bus into the line at buses[0], I(<name>.1.a) etc. at buses[1]. Branch voltages U(<name>.a) are V(buses[0].p) - V(buses[1].p).",
            parameters: vec![
                catalog::name(),
                p("buses", "", true, "[from, to]; each a bus or three node names. See bus."),
                p("length_km", "km", true, "Length, > 0."),
                p("per_km", "ohm/km, H/km, F/km", true, "As for pi_line: {\"type\":\"sequence\",\"r1_ohm_per_km\",\"l1_h_per_km\",\"c1_f_per_km\",\"r0_ohm_per_km\",\"l0_h_per_km\",\"c0_f_per_km\"} (zero-sequence mode and two line modes by the Clarke transformation) | {\"type\":\"matrix\",\"resistance_ohm_per_km\":[[3x3]],\"inductance_h_per_km\":[[3x3]],\"capacitance_f_per_km\":[[3x3]]} (modes of L*C; the resistance matrix must be nearly diagonal in them, E231)."),
            ],
            example: json!({"type": "bergeron_line", "name": "LINE1", "buses": ["b1", "b2"], "length_km": 100.0,
                "per_km": {"type": "sequence", "r1_ohm_per_km": 0.03, "l1_h_per_km": 0.0009, "c1_f_per_km": 1.27e-8, "r0_ohm_per_km": 0.2, "l0_h_per_km": 0.003, "c0_f_per_km": 7.0e-9}}),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{load_str, Severity};

    fn balanced(x1: f64, x0: f64) -> PhaseMatrix {
        sequence_matrix(x1, x0)
    }

    fn sequence() -> LineParameters {
        LineParameters::Sequence {
            r1_ohm_per_km: 0.03,
            l1_h_per_km: 0.9e-3,
            c1_f_per_km: 12.7e-9,
            r0_ohm_per_km: 0.2,
            l0_h_per_km: 3e-3,
            c0_f_per_km: 7e-9,
        }
    }

    /// W^T diag(d) W.
    fn in_phases(w: &PhaseMatrix, d: [f64; 3]) -> PhaseMatrix {
        let mut m = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] = (0..3).map(|k| w[k][i] * d[k] * w[k][j]).sum();
            }
        }
        m
    }

    #[test]
    fn jacobi_finds_the_eigenvalues_of_a_symmetric_matrix() {
        // Eigenvalues of [[2, 1, 0], [1, 2, 1], [0, 1, 2]]: 2 and 2 +- sqrt 2.
        let m = [[2.0, 1.0, 0.0], [1.0, 2.0, 1.0], [0.0, 1.0, 2.0]];
        let (mut lambda, v) = jacobi(&m);
        // m v = v diag(lambda).
        let mv = mat_mul(&m, &v);
        for i in 0..3 {
            for j in 0..3 {
                assert!((mv[i][j] - v[i][j] * lambda[j]).abs() < 1e-14);
            }
        }
        lambda.sort_by(f64::total_cmp);
        let r2 = 2.0_f64.sqrt();
        for (got, expected) in lambda.iter().zip([2.0 - r2, 2.0, 2.0 + r2]) {
            assert!((got - expected).abs() < 1e-14, "{got} {expected}");
        }
    }

    #[test]
    fn balanced_matrices_have_the_modes_of_the_clarke_transformation() {
        // The same line written both ways. The modes of the matrices are any
        // basis of the line-mode plane, so the transformations may differ,
        // but the values of the modes and everything in phase quantities
        // (W^T diag W) agree to rounding.
        let by_sequence = line_modes(&sequence(), 80.0).unwrap();
        let by_matrix = line_modes(
            &LineParameters::Matrix {
                resistance_ohm_per_km: balanced(0.03, 0.2),
                inductance_h_per_km: balanced(0.9e-3, 3e-3),
                capacitance_f_per_km: balanced(12.7e-9, 7e-9),
            },
            80.0,
        )
        .unwrap();
        assert!(by_matrix.resistance_coupling < 1e-12);
        for (a, b) in by_sequence.modes.iter().zip(&by_matrix.modes) {
            let close = |x: f64, y: f64| (x - y).abs() < 1e-12 * y.abs();
            assert!(
                close(a.surge_impedance_ohm, b.surge_impedance_ohm),
                "{a:?} {b:?}"
            );
            assert!(close(a.travel_time_s, b.travel_time_s), "{a:?} {b:?}");
            assert!(close(a.resistance_ohm, b.resistance_ohm), "{a:?} {b:?}");
        }
        // The zero-sequence mode is the slowest and comes first in both.
        for v in by_matrix.transform[0] {
            assert!((v.abs() - 1.0 / 3.0_f64.sqrt()).abs() < 1e-12);
        }
        let z = |m: &LineModes| m.modes.map(|k| 1.0 / k.surge_impedance_ohm);
        let (ga, gb) = (
            in_phases(&by_sequence.transform, z(&by_sequence)),
            in_phases(&by_matrix.transform, z(&by_matrix)),
        );
        for i in 0..3 {
            for j in 0..3 {
                assert!(
                    (ga[i][j] - gb[i][j]).abs() < 1e-12 * ga[0][0],
                    "{ga:?} {gb:?}"
                );
            }
        }
    }

    /// An untransposed line: flat formation, the outer phases alike.
    fn untransposed(r: PhaseMatrix) -> LineParameters {
        LineParameters::Matrix {
            resistance_ohm_per_km: r,
            inductance_h_per_km: [
                [1.6e-3, 0.6e-3, 0.4e-3],
                [0.6e-3, 1.6e-3, 0.6e-3],
                [0.4e-3, 0.6e-3, 1.6e-3],
            ],
            capacitance_f_per_km: [
                [10.0e-9, -2.0e-9, -0.8e-9],
                [-2.0e-9, 10.5e-9, -2.0e-9],
                [-0.8e-9, -2.0e-9, 10.0e-9],
            ],
        }
    }

    #[test]
    fn the_modes_of_an_untransposed_line_diagonalize_its_matrices() {
        let per_km = untransposed([[0.0; 3]; 3]);
        let m = line_modes(&per_km, 1.0).unwrap();
        let (_, l, c) = per_km.matrices();
        let w = m.transform;
        // W L W^T and W^-T C W^-1 are diagonal, with Zc^2 = L/C and
        // tau^2 = L C on the diagonal: check L = W^-1 diag(L_k) W^-T through
        // W L W^T, and C through its inverse relation C = W^T diag(C_k) W.
        let wl = mat_mul(&mat_mul(&w, &l), &transposed(&w));
        let c_modes = m.modes.map(|k| k.travel_time_s / k.surge_impedance_ohm);
        let c_back = in_phases(&w, c_modes);
        for i in 0..3 {
            let l_mode = m.modes[i].travel_time_s * m.modes[i].surge_impedance_ohm;
            for j in 0..3 {
                let expected = if i == j { l_mode } else { 0.0 };
                assert!((wl[i][j] - expected).abs() < 1e-12 * l[0][0], "{wl:?}");
                assert!(
                    (c_back[i][j] - c[i][j]).abs() < 1e-12 * c[0][0],
                    "{c_back:?}"
                );
            }
        }
        // Three different velocities, the slowest first.
        assert!(m.modes[0].travel_time_s > m.modes[1].travel_time_s);
        assert!(m.modes[1].travel_time_s > m.modes[2].travel_time_s);
        assert_eq!(m.resistance_coupling, 0.0);
    }

    fn model(line: &str, h: f64) -> String {
        format!(
            r#"{{"format": "openkeito-model", "version": "0.5",
            "simulation": {{"time_step_s": {h:e}, "end_time_s": 1e-2}},
            "components": [
              {{"type": "three_phase_source", "name": "G", "bus": "b1", "line_voltage_rms_v": 66e3, "frequency_hz": 50.0}},
              {line}
            ]}}"#
        )
    }

    const LINE: &str = r#"{"type": "bergeron_line", "name": "L1", "buses": ["b1", "b2"], "length_km": 10.0,
        "per_km": {"type": "sequence", "r1_ohm_per_km": 0.03, "l1_h_per_km": 0.9e-3, "c1_f_per_km": 12.7e-9,
                   "r0_ohm_per_km": 0.2, "l0_h_per_km": 3e-3, "c0_f_per_km": 7e-9}}"#;

    #[test]
    fn a_line_with_an_open_end_is_a_valid_model() {
        // Every node of the line reaches gnd through the line itself.
        let loaded = load_str(&model(LINE, 10e-6)).unwrap();
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    }

    #[test]
    fn a_travel_time_below_the_time_step_is_e230() {
        // 10 km: 33.8 us in the line modes, 45.8 us in the zero sequence.
        let d = load_str(&model(LINE, 40e-6)).unwrap_err();
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].code, "E230");
        assert_eq!(d[0].path, "components[1].length_km");
        assert!(d[0].message.contains("line modes 3.38"), "{}", d[0].message);
        assert!(!d[0].message.contains("zero"), "{}", d[0].message);
        assert!(d[0].hint.contains("pi_line"), "{}", d[0].hint);
        assert!(d[0].hint.contains("time_step_s to 3.38"), "{}", d[0].hint);
        let d = load_str(&model(LINE, 50e-6)).unwrap_err();
        assert!(
            d[0].message.contains("zero-sequence mode 4.58"),
            "{}",
            d[0].message
        );
    }

    #[test]
    fn a_travel_time_of_one_step_to_rounding_is_accepted() {
        // Zc = 400 ohm and 4 us/km in every mode: 2.5 km is one step of
        // 10 us, whatever the rounding of the square root.
        let line = r#"{"type": "bergeron_line", "name": "L1", "buses": ["b1", "b2"], "length_km": 2.5,
            "per_km": {"type": "sequence", "r1_ohm_per_km": 0.0, "l1_h_per_km": 1.6e-3, "c1_f_per_km": 1e-8,
                       "r0_ohm_per_km": 0.0, "l0_h_per_km": 1.6e-3, "c0_f_per_km": 1e-8}}"#;
        assert!(load_str(&model(line, 10e-6)).is_ok());
    }

    #[test]
    fn a_resistance_that_couples_the_modes_is_w231_or_e231() {
        let line = |r: PhaseMatrix| {
            serde_json::json!({"type": "bergeron_line", "name": "L1", "buses": ["b1", "b2"],
                "length_km": 30.0, "per_km": untransposed(r)})
            .to_string()
        };
        // Equal conductors and an earth return common to all phases: the
        // earth return couples the ground mode a little with a line mode.
        let d = load_str(&model(&line(sequence_matrix(0.05, 0.2)), 10e-6)).unwrap();
        assert_eq!(d.warnings.len(), 1, "{:?}", d.warnings);
        assert_eq!(d.warnings[0].code, "W231");
        // Very different conductors: the modes of L C are far from those of R.
        let unequal = [[0.02, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 0.02]];
        let d = load_str(&model(&line(unequal), 10e-6)).unwrap_err();
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].code, "E231");
        assert_eq!(d[0].severity, Severity::Error);
        assert!(d[0].hint.contains("sequence"), "{}", d[0].hint);
        assert!(d[0].hint.contains("pi_line"), "{}", d[0].hint);
    }

    #[test]
    fn a_resistance_above_the_characteristic_impedance_is_w230() {
        // Z1 = 266 ohm; 3 ohm/km over 100 km is 300 ohm.
        let line = LINE
            .replace("\"length_km\": 10.0", "\"length_km\": 100.0")
            .replace("\"r1_ohm_per_km\": 0.03", "\"r1_ohm_per_km\": 3.0");
        let d = load_str(&model(&line, 10e-6)).unwrap();
        assert_eq!(d.warnings.len(), 1, "{:?}", d.warnings);
        assert_eq!(d.warnings[0].code, "W230");
        assert!(d.warnings[0].message.contains("line modes"));
        assert!(!d.warnings[0].message.contains("zero"));
    }

    #[test]
    fn wrong_parameters_are_reported_once_and_unknown_fields_are_errors() {
        let line = LINE.replace("\"l0_h_per_km\": 3e-3", "\"l0_h_per_km\": 0.0");
        let d = load_str(&model(&line, 10e-6)).unwrap_err();
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].code, "E130");
        let line = LINE.replace(
            "\"length_km\": 10.0",
            "\"length_km\": 10.0, \"sections\": 2",
        );
        let d = load_str(&model(&line, 10e-6)).unwrap_err();
        assert!(d[0].message.contains("sections"), "{d:?}");
        // The format version that has the line.
        let d = load_str(&model(LINE, 10e-6).replace("\"0.5\"", "\"0.4\"")).unwrap_err();
        assert_eq!(d[0].code, "E102");
        assert!(d[0].hint.contains("bergeron_line"), "{}", d[0].hint);
    }
}
