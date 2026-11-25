//! Passive elements: resistor, inductor, capacitor, coupled RL branches and
//! the capacitance matrix.

use super::{
    branch_voltages, inject, instant, matrix_currents, phasor, stamp_conductance,
    stamp_conductance_matrix, volt, Element, Node, Stage, SwitchAction,
};
use crate::linalg::{Complex64, Scalar, Stamp};
use crate::sim::Stages;

pub(crate) struct Resistor {
    pub name: String,
    pub a: Node,
    pub b: Node,
    pub g: f64,
    pub changes: Vec<(f64, f64)>,
    pub next_change: usize,
}

impl Element for Resistor {
    fn name(&self) -> &str {
        &self.name
    }
    fn pre_step_changed(
        &mut self,
        t: f64,
        tol: f64,
        _actions: &mut Vec<(String, SwitchAction)>,
    ) -> bool {
        let mut changed = false;
        while self.next_change < self.changes.len() && self.changes[self.next_change].0 <= t + tol {
            self.g = self.changes[self.next_change].1;
            self.next_change += 1;
            changed = true;
        }
        changed
    }
    fn action_times(&self) -> Vec<f64> {
        self.changes.iter().map(|c| c.0).collect()
    }
    fn stamp(&self, _alpha: f64, m: &mut dyn Stamp) {
        stamp_conductance(m, self.a, self.b, self.g);
    }
    fn current(&self, _k: usize, _t: f64, _stage: &Stage, x: &[f64]) -> f64 {
        self.g * (volt(x, self.a) - volt(x, self.b))
    }
    fn current_phasor(&self, _k: usize, _omega: f64, x: &[Complex64]) -> Complex64 {
        self.g * (phasor(x, self.a) - phasor(x, self.b))
    }
}

/// v = L di/dt. State: current. Derivative: di/dt = v / L.
pub(crate) struct Inductor {
    pub name: String,
    pub a: Node,
    pub b: Node,
    pub l: f64,
    /// Current at the start of the step.
    pub i_start: f64,
    /// di/dt of the previous stage.
    pub f_prev: f64,
    /// (i_start, f_prev) stored by `save`.
    pub saved: (f64, f64),
    /// Scheduled (time_s, new_l, current_scale).
    pub changes: Vec<(f64, f64, f64)>,
    pub next_change: usize,
}

impl Inductor {
    fn history(&self, stage: &Stage) -> f64 {
        self.i_start + stage.c / stage.alpha * self.f_prev
    }
}

impl Element for Inductor {
    fn name(&self) -> &str {
        &self.name
    }
    fn pre_step_changed(
        &mut self,
        t: f64,
        tol: f64,
        _actions: &mut Vec<(String, SwitchAction)>,
    ) -> bool {
        let mut changed = false;
        while self.next_change < self.changes.len() && self.changes[self.next_change].0 <= t + tol {
            let (_, new_l, scale) = self.changes[self.next_change];
            self.l = new_l;
            self.i_start *= scale;
            self.f_prev *= scale;
            self.next_change += 1;
            changed = true;
        }
        changed
    }
    fn action_times(&self) -> Vec<f64> {
        self.changes.iter().map(|c| c.0).collect()
    }
    fn stamp(&self, alpha: f64, m: &mut dyn Stamp) {
        stamp_conductance(m, self.a, self.b, 1.0 / (alpha * self.l));
    }
    fn rhs(&self, _t: f64, stage: &Stage, b: &mut [f64]) {
        inject(b, self.a, self.b, self.history(stage));
    }
    fn current(&self, _k: usize, _t: f64, stage: &Stage, x: &[f64]) -> f64 {
        (volt(x, self.a) - volt(x, self.b)) / (stage.alpha * self.l) + self.history(stage)
    }
    fn advance(&mut self, stage: &Stage, x: &[f64]) {
        let i = self.current(0, 0.0, stage, x);
        self.f_prev = (volt(x, self.a) - volt(x, self.b)) / self.l;
        if stage.commit {
            self.i_start = i;
        }
    }
    fn save(&mut self) {
        self.saved = (self.i_start, self.f_prev);
    }
    fn restore(&mut self) {
        (self.i_start, self.f_prev) = self.saved;
    }
    fn stamp_phasor(&self, omega: f64, m: &mut dyn Stamp<Complex64>) {
        stamp_conductance(m, self.a, self.b, 1.0 / Complex64::new(0.0, omega * self.l));
    }
    fn current_phasor(&self, _k: usize, omega: f64, x: &[Complex64]) -> Complex64 {
        (phasor(x, self.a) - phasor(x, self.b)) / Complex64::new(0.0, omega * self.l)
    }
    fn init_from_phasor(&mut self, omega: f64, x: &[Complex64]) {
        self.i_start = instant(self.current_phasor(0, omega, x));
    }
}

/// i = C dv/dt. State: voltage. Derivative: dv/dt = i / C.
pub(crate) struct Capacitor {
    pub name: String,
    pub a: Node,
    pub b: Node,
    pub c: f64,
    /// Voltage at the start of the step.
    pub v_start: f64,
    /// dv/dt of the previous stage.
    pub f_prev: f64,
    /// (v_start, f_prev) stored by `save`.
    pub saved: (f64, f64),
}

impl Capacitor {
    /// The branch current is alpha*C*v - equivalent(stage).
    fn equivalent(&self, stage: &Stage) -> f64 {
        self.c * (stage.alpha * self.v_start + stage.c * self.f_prev)
    }
}

impl Element for Capacitor {
    fn name(&self) -> &str {
        &self.name
    }
    fn stamp(&self, alpha: f64, m: &mut dyn Stamp) {
        stamp_conductance(m, self.a, self.b, alpha * self.c);
    }
    fn rhs(&self, _t: f64, stage: &Stage, b: &mut [f64]) {
        inject(b, self.a, self.b, -self.equivalent(stage));
    }
    fn current(&self, _k: usize, _t: f64, stage: &Stage, x: &[f64]) -> f64 {
        stage.alpha * self.c * (volt(x, self.a) - volt(x, self.b)) - self.equivalent(stage)
    }
    fn advance(&mut self, stage: &Stage, x: &[f64]) {
        let i = self.current(0, 0.0, stage, x);
        self.f_prev = i / self.c;
        if stage.commit {
            self.v_start = volt(x, self.a) - volt(x, self.b);
        }
    }
    fn save(&mut self) {
        self.saved = (self.v_start, self.f_prev);
    }
    fn restore(&mut self) {
        (self.v_start, self.f_prev) = self.saved;
    }
    fn stamp_phasor(&self, omega: f64, m: &mut dyn Stamp<Complex64>) {
        stamp_conductance(m, self.a, self.b, Complex64::new(0.0, omega * self.c));
    }
    fn current_phasor(&self, _k: usize, omega: f64, x: &[Complex64]) -> Complex64 {
        Complex64::new(0.0, omega * self.c) * (phasor(x, self.a) - phasor(x, self.b))
    }
    fn init_from_phasor(&mut self, _omega: f64, x: &[Complex64]) {
        self.v_start = instant(phasor(x, self.a) - phasor(x, self.b));
    }
}

/// A 3x3 matrix of which the leading n x n block is used.
pub(crate) type Mat3 = [[f64; 3]; 3];

/// Inverse of the leading n x n block of `m` (Gauss-Jordan with partial
/// pivoting); `None` when it is singular.
pub(crate) fn invert<T: Scalar>(m: &[[T; 3]; 3], n: usize) -> Option<[[T; 3]; 3]> {
    let mut a = *m;
    let mut inv = [[T::ZERO; 3]; 3];
    for (k, row) in inv.iter_mut().enumerate().take(n) {
        row[k] = T::ONE;
    }
    let scale = a
        .iter()
        .take(n)
        .flat_map(|r| r.iter().take(n))
        .fold(0.0_f64, |s, v| s.max(v.magnitude()));
    for col in 0..n {
        let p =
            (col..n).max_by(|&i, &j| a[i][col].magnitude().total_cmp(&a[j][col].magnitude()))?;
        // Also rejects NaN.
        if a[p][col].magnitude().partial_cmp(&(1e-14 * scale)) != Some(std::cmp::Ordering::Greater)
        {
            return None;
        }
        a.swap(col, p);
        inv.swap(col, p);
        let pivot = a[col][col];
        for k in 0..n {
            a[col][k] = a[col][k] / pivot;
            inv[col][k] = inv[col][k] / pivot;
        }
        for r in 0..n {
            if r != col {
                let f = a[r][col];
                if f != T::ZERO {
                    for k in 0..n {
                        a[r][k] = a[r][k] - f * a[col][k];
                        inv[r][k] = inv[r][k] - f * inv[col][k];
                    }
                }
            }
        }
    }
    Some(inv)
}

fn mat_mul(a: &Mat3, b: &Mat3, n: usize) -> Mat3 {
    let mut c = [[0.0; 3]; 3];
    for i in 0..n {
        for j in 0..n {
            c[i][j] = (0..n).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    c
}

/// G = (R + alpha L)^-1 and H = G L of a `CoupledRl` for one `alpha`.
#[derive(Clone, Copy)]
struct RlConductance {
    alpha: f64,
    g: Mat3,
    h: Mat3,
}

/// n coupled series branches (n <= 3) with resistance and inductance
/// matrices: v = R i + L di/dt, where v[k] = V(a[k]) - V(b[k]) and i[k]
/// flows from a[k] to b[k]. State: the currents.
///
/// With the stage formula di/dt = alpha (i - i_start) - c f_prev the branch
/// equation becomes (R + alpha L) i = v + L (alpha i_start + c f_prev), so
/// i = G v + h with G = (R + alpha L)^-1 and h = G L (alpha i_start + c f_prev).
pub(crate) struct CoupledRl {
    pub name: String,
    pub n: usize,
    pub a: [Node; 3],
    pub b: [Node; 3],
    pub r: Mat3,
    pub l: Mat3,
    pub i_start: [f64; 3],
    pub f_prev: [f64; 3],
    saved: ([f64; 3], [f64; 3]),
    /// Initial (r, l) followed by scheduled changes.
    states: Vec<(Mat3, Mat3)>,
    change_times: Vec<f64>,
    active: usize,
    /// Cached conductance sets per state: the stage coefficients of `prepare`,
    /// followed by at most one for the part of a split step (`prepare_alpha`).
    caches: Vec<Vec<RlConductance>>,
    /// Number of entries of `caches[k]` that `prepare` computed.
    prepared: usize,
    event_name: Option<String>,
}

impl CoupledRl {
    pub fn new(name: String, n: usize, a: [Node; 3], b: [Node; 3], r: Mat3, l: Mat3) -> Self {
        Self::with_changes(name, n, a, b, r, l, Vec::new(), None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_changes(
        name: String,
        n: usize,
        a: [Node; 3],
        b: [Node; 3],
        r: Mat3,
        l: Mat3,
        changes: Vec<(f64, Mat3, Mat3)>,
        event_name: Option<String>,
    ) -> Self {
        let mut states = Vec::with_capacity(1 + changes.len());
        states.push((r, l));
        let mut change_times = Vec::with_capacity(changes.len());
        for (t, cr, cl) in changes {
            change_times.push(t);
            states.push((cr, cl));
        }
        let caches = vec![Vec::new(); states.len()];
        CoupledRl {
            name,
            n,
            a,
            b,
            r,
            l,
            i_start: [0.0; 3],
            f_prev: [0.0; 3],
            saved: ([0.0; 3], [0.0; 3]),
            states,
            change_times,
            active: 0,
            caches,
            prepared: 0,
            event_name,
        }
    }

    /// `None` when R + alpha L is singular to rounding: L is positive
    /// definite (validated), but a zero-sequence value many orders of
    /// magnitude above the positive-sequence one leaves no digits for the
    /// difference between the phases.
    fn compute_for(n: usize, r: &Mat3, l: &Mat3, alpha: f64) -> Option<RlConductance> {
        let mut m = [[0.0; 3]; 3];
        for i in 0..n {
            for j in 0..n {
                m[i][j] = r[i][j] + alpha * l[i][j];
            }
        }
        let g = invert(&m, n)?;
        // H = G L = (I - G R) / alpha. Where the sequences differ by a factor
        // k, the entries of G times those of L are k times the entries of H,
        // and the product loses that many digits. The second form loses them
        // of G R only, which is the small part of I - G R where alpha L is
        // far above R: in the first part of the first step (alpha of 1e6
        // steps), where what is lost in h is a voltage error 1 / G larger.
        // It is used where the product loses more than two digits.
        const LOST: f64 = 100.0;
        let largest = |a: &Mat3| a.iter().flatten().fold(0.0_f64, |m, x| m.max(x.abs()));
        let mut h = mat_mul(&g, l, n);
        let mut terms = 0.0_f64;
        for gi in g.iter().take(n) {
            // L is symmetric: its row j is its column j.
            for lj in l.iter().take(n) {
                terms = terms.max((0..n).map(|k| (gi[k] * lj[k]).abs()).sum());
            }
        }
        if terms > LOST * largest(&h) && alpha * largest(l) > largest(r) {
            h = mat_mul(&g, r, n);
            for (i, row) in h.iter_mut().enumerate().take(n) {
                for (j, x) in row.iter_mut().enumerate().take(n) {
                    *x = (f64::from(u8::from(i == j)) - *x) / alpha;
                }
            }
        }
        Some(RlConductance { alpha, g, h })
    }

    /// (R + j omega L)^-1, or `None` when it is singular to rounding.
    fn try_admittance(&self, omega: f64) -> Option<[[Complex64; 3]; 3]> {
        let mut z = [[Complex64::ZERO; 3]; 3];
        for i in 0..self.n {
            for j in 0..self.n {
                z[i][j] = Complex64::new(self.r[i][j], omega * self.l[i][j]);
            }
        }
        invert(&z, self.n)
    }

    /// (R + j omega L)^-1. A phasor solution is only computed after
    /// `phasor_singular` returned `None` for every element.
    fn admittance(&self, omega: f64) -> [[Complex64; 3]; 3] {
        self.try_admittance(omega)
            .expect("phasor_singular was checked before the phasor solution")
    }

    /// The values for a stage coefficient given to `prepare` or
    /// `prepare_alpha`.
    fn conductance(&self, alpha: f64) -> RlConductance {
        self.caches[self.active]
            .iter()
            .find(|c| c.alpha == alpha)
            .copied()
            .expect("prepare or prepare_alpha computed the stage coefficient")
    }

    fn history(&self, cd: &RlConductance, stage: &Stage) -> [f64; 3] {
        let n = self.n;
        let mut y = [0.0; 3];
        for (j, yj) in y.iter_mut().enumerate().take(n) {
            *yj = stage.alpha * self.i_start[j] + stage.c * self.f_prev[j];
        }
        let mut h = [0.0; 3];
        for (k, hk) in h.iter_mut().enumerate().take(n) {
            *hk = (0..n).map(|j| cd.h[k][j] * y[j]).sum();
        }
        h
    }

    fn currents(&self, stage: &Stage, x: &[f64]) -> [f64; 3] {
        let cd = self.conductance(stage.alpha);
        let h = self.history(&cd, stage);
        let v = branch_voltages(x, &self.a, &self.b, self.n);
        let mut i = [0.0; 3];
        for k in 0..self.n {
            i[k] = (0..self.n).map(|j| cd.g[k][j] * v[j]).sum::<f64>() + h[k];
        }
        i
    }
}

impl Element for CoupledRl {
    fn name(&self) -> &str {
        &self.name
    }
    fn pre_step_changed(
        &mut self,
        t: f64,
        tol: f64,
        actions: &mut Vec<(String, SwitchAction)>,
    ) -> bool {
        let mut changed = false;
        while self.active < self.change_times.len() && self.change_times[self.active] <= t + tol {
            self.active += 1;
            let (r, l) = self.states[self.active];
            self.r = r;
            self.l = l;
            changed = true;
        }
        if changed {
            if let Some(name) = &self.event_name {
                actions.push((name.clone(), SwitchAction::Step));
            }
        }
        changed
    }
    fn action_times(&self) -> Vec<f64> {
        self.change_times.clone()
    }
    fn prepare(&mut self, stages: &Stages) -> Result<(), String> {
        let alphas = stages.alphas();
        for (cache, (r, l)) in self.caches.iter_mut().zip(&self.states) {
            cache.clear();
            for &alpha in &alphas {
                let cd = Self::compute_for(self.n, r, l, alpha).ok_or_else(|| {
                    "its resistance and inductance matrices differ by too many orders of magnitude \
                     between the phases to be inverted in floating point"
                        .to_string()
                })?;
                cache.push(cd);
            }
        }
        self.prepared = alphas.len();
        Ok(())
    }
    fn prepare_alpha(&mut self, alpha: f64) -> Result<(), String> {
        let cache = &mut self.caches[self.active];
        if cache.iter().any(|c| c.alpha == alpha) {
            return Ok(());
        }
        let (r, l) = &self.states[self.active];
        let cd = Self::compute_for(self.n, r, l, alpha).ok_or_else(|| {
            "its resistance and inductance matrices differ by too many orders of magnitude \
             between the phases to be inverted in floating point for a part of a time step"
                .to_string()
        })?;
        cache.truncate(self.prepared);
        cache.push(cd);
        Ok(())
    }
    fn structure(&self, m: &mut dyn Stamp) {
        // Every entry of the conductance matrix; asked for before `prepare`.
        stamp_conductance_matrix(m, &self.a, &self.b, &[[1.0; 3]; 3], self.n);
    }
    fn phasor_singular(&self, omega: f64) -> Option<usize> {
        if self.try_admittance(omega).is_some() {
            return None;
        }
        // Any of its nodes names the place; gnd has no unknown.
        Some(
            self.a
                .iter()
                .chain(&self.b)
                .take(2 * self.n)
                .find_map(|n| *n)
                .unwrap_or(0),
        )
    }
    fn stamp(&self, alpha: f64, m: &mut dyn Stamp) {
        let cd = self.conductance(alpha);
        stamp_conductance_matrix(m, &self.a, &self.b, &cd.g, self.n);
    }
    fn rhs(&self, _t: f64, stage: &Stage, b: &mut [f64]) {
        let cd = self.conductance(stage.alpha);
        let h = self.history(&cd, stage);
        for (k, hk) in h.iter().enumerate().take(self.n) {
            inject(b, self.a[k], self.b[k], *hk);
        }
    }
    fn current(&self, k: usize, _t: f64, stage: &Stage, x: &[f64]) -> f64 {
        self.currents(stage, x)[k]
    }
    fn advance(&mut self, stage: &Stage, x: &[f64]) {
        let i = self.currents(stage, x);
        for k in 0..self.n {
            self.f_prev[k] = stage.alpha * (i[k] - self.i_start[k]) - stage.c * self.f_prev[k];
        }
        if stage.commit {
            self.i_start = i;
        }
    }
    fn save(&mut self) {
        self.saved = (self.i_start, self.f_prev);
    }
    fn restore(&mut self) {
        (self.i_start, self.f_prev) = self.saved;
    }
    fn stamp_phasor(&self, omega: f64, m: &mut dyn Stamp<Complex64>) {
        stamp_conductance_matrix(m, &self.a, &self.b, &self.admittance(omega), self.n);
    }
    fn current_phasor(&self, k: usize, omega: f64, x: &[Complex64]) -> Complex64 {
        matrix_currents(&self.admittance(omega), &self.a, &self.b, self.n, x)[k]
    }
    fn init_from_phasor(&mut self, omega: f64, x: &[Complex64]) {
        let i = matrix_currents(&self.admittance(omega), &self.a, &self.b, self.n, x);
        for k in 0..self.n {
            self.i_start[k] = instant(i[k]);
        }
    }
}

/// n coupled capacitances (n <= 3): i = C dv/dt with a nodal capacitance
/// matrix C, v[k] = V(a[k]) - V(b[k]). State: the voltages.
///
/// With dv/dt = alpha (v - v_start) - c f_prev, i = alpha C v - q with
/// q = C (alpha v_start + c f_prev).
pub(crate) struct CapacitanceMatrix {
    pub name: String,
    pub n: usize,
    pub a: [Node; 3],
    pub b: [Node; 3],
    pub c: Mat3,
    pub v_start: [f64; 3],
    pub f_prev: [f64; 3],
    saved: ([f64; 3], [f64; 3]),
}

impl CapacitanceMatrix {
    pub fn new(name: String, n: usize, a: [Node; 3], b: [Node; 3], c: Mat3) -> Self {
        CapacitanceMatrix {
            name,
            n,
            a,
            b,
            c,
            v_start: [0.0; 3],
            f_prev: [0.0; 3],
            saved: ([0.0; 3], [0.0; 3]),
        }
    }

    /// j omega C.
    fn admittance(&self, omega: f64) -> [[Complex64; 3]; 3] {
        self.c
            .map(|row| row.map(|c| Complex64::new(0.0, omega * c)))
    }

    fn equivalent(&self, stage: &Stage) -> [f64; 3] {
        let n = self.n;
        let mut q = [0.0; 3];
        for (k, qk) in q.iter_mut().enumerate().take(n) {
            *qk = (0..n)
                .map(|j| self.c[k][j] * (stage.alpha * self.v_start[j] + stage.c * self.f_prev[j]))
                .sum();
        }
        q
    }

    fn currents(&self, stage: &Stage, x: &[f64]) -> [f64; 3] {
        let v = branch_voltages(x, &self.a, &self.b, self.n);
        let q = self.equivalent(stage);
        let mut i = [0.0; 3];
        for k in 0..self.n {
            i[k] = stage.alpha * (0..self.n).map(|j| self.c[k][j] * v[j]).sum::<f64>() - q[k];
        }
        i
    }
}

impl Element for CapacitanceMatrix {
    fn name(&self) -> &str {
        &self.name
    }
    fn stamp(&self, alpha: f64, m: &mut dyn Stamp) {
        let mut g = [[0.0; 3]; 3];
        for (gj, cj) in g.iter_mut().zip(&self.c) {
            for (gjk, cjk) in gj.iter_mut().zip(cj) {
                *gjk = alpha * cjk;
            }
        }
        stamp_conductance_matrix(m, &self.a, &self.b, &g, self.n);
    }
    fn rhs(&self, _t: f64, stage: &Stage, b: &mut [f64]) {
        let q = self.equivalent(stage);
        for (k, qk) in q.iter().enumerate().take(self.n) {
            inject(b, self.a[k], self.b[k], -qk);
        }
    }
    fn current(&self, k: usize, _t: f64, stage: &Stage, x: &[f64]) -> f64 {
        self.currents(stage, x)[k]
    }
    fn advance(&mut self, stage: &Stage, x: &[f64]) {
        let v = branch_voltages(x, &self.a, &self.b, self.n);
        for k in 0..self.n {
            self.f_prev[k] = stage.alpha * (v[k] - self.v_start[k]) - stage.c * self.f_prev[k];
        }
        if stage.commit {
            self.v_start = v;
        }
    }
    fn save(&mut self) {
        self.saved = (self.v_start, self.f_prev);
    }
    fn restore(&mut self) {
        (self.v_start, self.f_prev) = self.saved;
    }
    fn stamp_phasor(&self, omega: f64, m: &mut dyn Stamp<Complex64>) {
        stamp_conductance_matrix(m, &self.a, &self.b, &self.admittance(omega), self.n);
    }
    fn current_phasor(&self, k: usize, omega: f64, x: &[Complex64]) -> Complex64 {
        matrix_currents(&self.admittance(omega), &self.a, &self.b, self.n, x)[k]
    }
    fn init_from_phasor(&mut self, _omega: f64, x: &[Complex64]) {
        for k in 0..self.n {
            self.v_start[k] = instant(phasor(x, self.a[k]) - phasor(x, self.b[k]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_of_a_coupled_matrix() {
        let m = [[2.0, 0.5, 0.1], [0.5, 3.0, 0.2], [0.1, 0.2, 4.0]];
        let inv = invert(&m, 3).unwrap();
        let p = mat_mul(&m, &inv, 3);
        for (i, row) in p.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                let e = if i == j { 1.0 } else { 0.0 };
                assert!((v - e).abs() < 1e-14, "{p:?}");
            }
        }
        assert!(invert(&[[1.0, 2.0, 0.0], [2.0, 4.0, 0.0], [0.0, 0.0, 1.0]], 3).is_none());
        assert_eq!(
            invert(&[[4.0, 0.0, 0.0], [0.0; 3], [0.0; 3]], 1).unwrap()[0][0],
            0.25
        );
    }
}
