//! Lines with travelling waves: the constant-parameter (Bergeron) line.

use std::cell::Cell;
use std::collections::VecDeque;

use openkeito_model::{LineModes, TRAVEL_TIME_TOLERANCE};

use super::{add, inject, phasor, stamp_conductance_matrix, volt, Element, Mat3, Node, Stage};
use crate::linalg::{Complex64, Stamp};
use crate::sim::Stages;

/// One mode: a lossless line with characteristic impedance Zc and travel
/// time tau, with R/4 at each end and R/2 in the middle.
#[derive(Debug, Clone, Copy)]
struct Mode {
    /// Zc + R/4: what an end looks like to its node.
    z: f64,
    /// (Zc - R/4) / (Zc + R/4).
    rho: f64,
    /// Travel time in seconds.
    tau: f64,
}

/// A travel time in time steps.
#[derive(Debug, Clone, Copy)]
enum Delay {
    /// A whole number of steps: the history is read at the same place in a
    /// step as the stage being solved, without interpolation.
    Whole(i64),
    Fraction(f64),
}

/// The values of every mode at both ends: `[end][mode]`.
type Waves = [[f64; 3]; 2];

/// What both ends sent into the line at one instant: a = v + rho z i of
/// every mode (v the mode voltage at the end, i the mode current into the
/// line), from the solution of a stage.
#[derive(Debug, Clone, Copy)]
struct Sample {
    /// Place in its time step, in (0, 1]; 1 is the grid point at its end.
    fraction: f64,
    a: Waves,
}

/// Where a value of the history is found.
#[derive(Debug, Clone, Copy)]
enum Reading {
    /// Before t = 0, at this time in seconds.
    Before(f64),
    /// The sample of this number.
    At(u64),
    /// Between the samples of these numbers, with this weight of the second.
    Between(u64, u64, f64),
}

/// What is still to do about a switching action.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Action {
    None,
    /// An action took effect at the present time and no stage has been
    /// solved since: the next sample is of the circuit after it.
    Taken,
    /// The sample of the value just after the action has this number and
    /// place (in steps from t = 0), and holds the value of the first
    /// solution after the action, which lies at the second place.
    Sampled(u64, f64, f64),
}

/// The history term of the stage that was solved last, so that the right-hand
/// side, the recorded currents and `advance` compute it once.
#[derive(Debug, Clone, Copy)]
struct Cached {
    alpha: u64,
    fraction: u64,
    /// E of every mode at both ends.
    e: Waves,
    /// The known currents out of the line, `[end][phase]`: W^T (E / z).
    j: Waves,
}

/// Three-phase line with distributed constant parameters [Dommel 1969].
///
/// Each mode k of `w` (v_mode = W v_phase, i_phase = W^T i_mode) is two
/// lossless half lines with R/4, R/2 and R/4 in series at the ends and in the
/// middle. Eliminating the middle gives, for the end `e` with the other end
/// `o` (currents into the line),
///
/// ```text
/// i_e(t) = (v_e(t) - E_e(t)) / z
/// E_e(t) = (1 + rho)/2 a_o(t - tau) + (1 - rho)/2 a_e(t - tau)
/// a(t)   = v(t) + rho z i(t) = (1 + rho) v(t) - rho E(t)
/// ```
///
/// with z = Zc + R/4 and rho = (Zc - R/4)/z. To the circuit an end is the
/// conductance matrix G = W^T diag(1/z) W from its nodes to gnd and the
/// current W^T (E/z) out of the line, which is known: it is at least one time
/// step old. The matrix does not depend on the stage and has no entries
/// between the two ends.
///
/// History: `a` of both ends is kept from the solution of every stage (the
/// grid points and the stages between them), and read by linear
/// interpolation between the two neighbouring samples. A travel time of a
/// whole number of steps reads the sample of the same stage of an earlier
/// step, which exists when that step had the same stages: no interpolation.
/// Before t = 0 the history is zero, or the sine of the steady state.
pub(crate) struct BergeronLine {
    name: String,
    ends: [[Node; 3]; 2],
    /// Rows: the modes.
    w: Mat3,
    modes: [Mode; 3],
    /// W^T diag(1/z) W.
    g: Mat3,
    /// The time step, from `prepare`.
    step_s: f64,
    delay: [Delay; 3],
    /// Length of the negligible step at t = 0+, whose solutions are the
    /// values at t = 0.
    initial_interval: f64,
    /// The samples from t = 0 on, in the order of time. Sample number 0 (of
    /// all, `dropped` counts those removed at the front) is t = 0.
    samples: VecDeque<Sample>,
    dropped: u64,
    /// Number of the first sample of every time step from `first_step` to
    /// the present one; the sample before it is the grid point at the start
    /// of the step.
    first: VecDeque<u64>,
    first_step: u64,
    /// The time step being solved (grid point `step` is its start).
    step: u64,
    /// Time steps to keep behind the present one.
    keep: u64,
    /// The steady state before t = 0: its angular frequency and the phasors
    /// (rms) of `a`.
    steady: Option<(f64, [[Complex64; 3]; 2])>,
    /// The stages of a regular step, and for each of them where every mode
    /// reads, in steps from the start of the present step and a fraction in
    /// [0, 1): these do not change during a run.
    regular: [Option<Stage>; 2],
    places: [[(i64, f64); 3]; 2],
    /// The history terms of the regular stages of the present step, computed
    /// when the step before it ended. Every stage reads at or before the
    /// start of its step, so they are known then; the places they read are
    /// neighbours in the history, which is read once per step this way and
    /// not once per stage with everything else of the circuit in between.
    ahead: [Option<Cached>; 2],
    /// The first mode with the same travel time as mode k (k itself when
    /// there is none before it): they read at the same place.
    same: [usize; 3],
    cache: Cell<Option<Cached>>,
    action: Action,
    /// (number of samples, step, `action`) stored by `save`.
    saved: (u64, u64, Action),
    /// The part of the present time step being solved, when the step is
    /// split at a switching instant (`begin_part`): its start and its length
    /// as fractions of the time step, and whether it ends at the grid point.
    part: Option<(f64, f64, bool)>,
}

/// W^T diag(d) W.
fn in_phases<T>(w: &Mat3, d: [T; 3]) -> [[T; 3]; 3]
where
    T: crate::linalg::Scalar,
{
    let mut m = [[T::ZERO; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            for k in 0..3 {
                m[i][j] = m[i][j] + d[k].scale(w[k][i] * w[k][j]);
            }
        }
    }
    m
}

impl BergeronLine {
    pub fn new(name: String, from: [Node; 3], to: [Node; 3], modes: &LineModes) -> Self {
        let m = modes.modes.map(|m| {
            let z = m.surge_impedance_ohm + m.resistance_ohm / 4.0;
            Mode {
                z,
                rho: (m.surge_impedance_ohm - m.resistance_ohm / 4.0) / z,
                tau: m.travel_time_s,
            }
        });
        BergeronLine {
            name,
            ends: [from, to],
            w: modes.transform,
            modes: m,
            g: in_phases(&modes.transform, m.map(|m| 1.0 / m.z)),
            step_s: 0.0,
            delay: [Delay::Whole(1); 3],
            initial_interval: 0.0,
            samples: VecDeque::new(),
            dropped: 0,
            first: VecDeque::from([1]),
            first_step: 0,
            step: 0,
            keep: 3,
            steady: None,
            regular: [None; 2],
            places: [[(0, 0.0); 3]; 2],
            ahead: [None; 2],
            same: [0, 1, 2],
            cache: Cell::new(None),
            action: Action::None,
            saved: (0, 0, Action::None),
            part: None,
        }
    }

    /// The place of `stage` in the present time step, in (0, 1]: its
    /// `time_fraction`, which is a fraction of the part being solved when
    /// the step is split.
    #[inline]
    fn place(&self, stage: &Stage) -> f64 {
        match self.part {
            None => stage.time_fraction,
            Some((_, _, true)) if stage.time_fraction >= 1.0 => 1.0,
            // Below 1: only the grid point ends the step.
            Some((start, length, _)) => {
                (start + length * stage.time_fraction).min(1.0 - f64::EPSILON)
            }
        }
    }

    /// The voltages of the modes at both ends in the solution `x`.
    #[inline]
    fn mode_voltages(&self, x: &[f64]) -> Waves {
        let mut v = [[0.0; 3]; 2];
        for (end, nodes) in self.ends.iter().enumerate() {
            let phase = [volt(x, nodes[0]), volt(x, nodes[1]), volt(x, nodes[2])];
            for k in 0..3 {
                v[end][k] =
                    self.w[k][0] * phase[0] + self.w[k][1] * phase[1] + self.w[k][2] * phase[2];
            }
        }
        v
    }

    /// `a` of mode `k` at both ends at the time `t` < 0.
    fn before_start(&self, k: usize, t: f64) -> [f64; 2] {
        match &self.steady {
            Some((omega, a)) => {
                let turn = Complex64::from_polar(std::f64::consts::SQRT_2, omega * t);
                [(a[0][k] * turn).re, (a[1][k] * turn).re]
            }
            None => [0.0; 2],
        }
    }

    #[inline]
    fn sample(&self, number: u64) -> &Sample {
        &self.samples[(number - self.dropped) as usize]
    }

    /// Where the history holds the place `fraction` in [0, 1) of the time
    /// step `step` >= 0, which lies at or before the start of the present
    /// step.
    fn find(&self, step: u64, fraction: f64) -> Reading {
        debug_assert!(step < self.step || (step == self.step && fraction == 0.0));
        if self.samples.is_empty() {
            return Reading::Before(0.0);
        }
        if step > self.step || step < self.first_step {
            // Not reached: the travel time is at least one step, and the
            // steps it reaches back are kept.
            return Reading::At(self.dropped + self.samples.len() as u64 - 1);
        }
        let start = self.first[(step - self.first_step) as usize];
        let mut before = start - 1;
        if fraction == 0.0 || step == self.step {
            // The grid point at the start of the step. After an action
            // there, the value after it (the first sample of the step, at
            // 0): what the grid point sent on is what the circuit was from
            // then on.
            let after = start < self.dropped + self.samples.len() as u64
                && self.sample(start).fraction == 0.0;
            return Reading::At(if after { start } else { before });
        }
        let end = self.first[(step + 1 - self.first_step) as usize];
        let mut from = 0.0;
        for number in start..end {
            let at = self.sample(number).fraction;
            if fraction <= at {
                // Weights 0 and 1 at a sample: its value to the last bit.
                return Reading::Between(before, number, (fraction - from) / (at - from));
            }
            from = at;
            before = number;
        }
        // Not reached: the last sample of a step is its grid point.
        Reading::At(before)
    }

    /// The place one travel time `delay` before the place `fraction` of a
    /// step: whole steps from the start of that step and a fraction in
    /// [0, 1).
    #[inline]
    fn back(delay: Delay, fraction: f64) -> (i64, f64) {
        let (mut step, mut fraction) = match delay {
            Delay::Whole(n) => (-n, fraction),
            Delay::Fraction(d) => {
                let place = fraction - d;
                let whole = place.floor();
                (whole as i64, place - whole)
            }
        };
        if fraction >= 1.0 {
            step += 1;
            fraction = 0.0;
        }
        (step, fraction)
    }

    /// Where mode `k` reads for `stage` of the present step, one travel time
    /// earlier. `regular` is the number of the stage in a regular step.
    #[inline]
    fn locate(&self, k: usize, stage: &Stage, regular: Option<usize>) -> Reading {
        if stage.is_initial() {
            let tau = self.modes[k].tau;
            return Reading::Before(stage.time_fraction * self.initial_interval - tau);
        }
        let (back, fraction) = match regular {
            Some(j) => self.places[j][k],
            None => Self::back(self.delay[k], self.place(stage)),
        };
        let step = self.step as i64 + back;
        if step < 0 {
            Reading::Before((step as f64 + fraction) * self.step_s)
        } else {
            self.find(step as u64, fraction)
        }
    }

    /// `a` of mode `k` at both ends at `reading`.
    #[inline]
    fn value(&self, k: usize, reading: Reading) -> [f64; 2] {
        match reading {
            Reading::Before(t) => self.before_start(k, t),
            Reading::At(number) => {
                let s = self.sample(number);
                [s.a[0][k], s.a[1][k]]
            }
            Reading::Between(before, after, weight) => {
                let (p, q) = (self.sample(before), self.sample(after));
                [
                    (1.0 - weight) * p.a[0][k] + weight * q.a[0][k],
                    (1.0 - weight) * p.a[1][k] + weight * q.a[1][k],
                ]
            }
        }
    }

    /// The history term of `stage` of the present step.
    #[inline]
    fn history(&self, stage: &Stage) -> Cached {
        // A regular step that is solved whole: computed when the step
        // before it ended. `begin_part` and `after_action` empty this, so
        // nothing here belongs to a step that is split.
        let key = (stage.alpha.to_bits(), stage.time_fraction.to_bits());
        for c in self.ahead.iter().flatten() {
            if (c.alpha, c.fraction) == key {
                return *c;
            }
        }
        self.history_of_stage(stage)
    }

    /// The history term of a stage that was not computed ahead: read on its
    /// own, and kept until the next solution is taken.
    #[inline(never)]
    fn history_of_stage(&self, stage: &Stage) -> Cached {
        let key = (stage.alpha.to_bits(), self.place(stage).to_bits());
        if let Some(c) = self.cache.get() {
            if (c.alpha, c.fraction) == key {
                return c;
            }
        }
        // The stages of a part of a split step lie elsewhere in the step
        // than those of a regular one.
        let regular = match self.part {
            Some(_) => None,
            None => self.regular.iter().position(|s| {
                s.is_some_and(|s| (s.alpha.to_bits(), s.time_fraction.to_bits()) == key)
            }),
        };
        let cached = self.read(stage, regular);
        self.cache.set(Some(cached));
        cached
    }

    /// The history term of `stage` of the present step, from the history.
    /// `regular` is the number of the stage in a regular step.
    fn read(&self, stage: &Stage, regular: Option<usize>) -> Cached {
        let mut readings = [Reading::Before(0.0); 3];
        let mut e = [[0.0; 3]; 2];
        for (k, mode) in self.modes.iter().enumerate() {
            readings[k] = if self.same[k] < k {
                readings[self.same[k]]
            } else {
                self.locate(k, stage, regular)
            };
            let a = self.value(k, readings[k]);
            let (across, back) = (0.5 * (1.0 + mode.rho), 0.5 * (1.0 - mode.rho));
            e[0][k] = across * a[1] + back * a[0];
            e[1][k] = across * a[0] + back * a[1];
        }
        Cached {
            alpha: stage.alpha.to_bits(),
            fraction: self.place(stage).to_bits(),
            e,
            j: [self.sources(&e[0]), self.sources(&e[1])],
        }
    }

    /// The known currents out of the line at an end, per phase:
    /// W^T (E / z).
    #[inline]
    fn sources(&self, e: &[f64; 3]) -> [f64; 3] {
        let m = [
            e[0] / self.modes[0].z,
            e[1] / self.modes[1].z,
            e[2] / self.modes[2].z,
        ];
        let mut j = [0.0; 3];
        for (p, jp) in j.iter_mut().enumerate() {
            *jp = self.w[0][p] * m[0] + self.w[1][p] * m[1] + self.w[2][p] * m[2];
        }
        j
    }

    /// Starts the history at t = 0 with `a`.
    fn begin(&mut self, a: Waves) {
        self.action = Action::None;
        self.cache.set(None);
        self.ahead = [None; 2];
        self.samples.clear();
        self.samples.push_back(Sample { fraction: 1.0, a });
        self.dropped = 0;
        self.first.clear();
        self.first.push_back(1);
        self.first_step = 0;
        self.step = 0;
    }

    /// Removes the time steps that no travel time reaches back to.
    fn forget(&mut self) {
        if self.step <= self.first_step + self.keep {
            return;
        }
        let first_step = self.step - self.keep;
        // The grid point at the start of the first step kept stays.
        let first_sample = self.first[(first_step - self.first_step) as usize] - 1;
        while self.dropped < first_sample {
            self.samples.pop_front();
            self.dropped += 1;
        }
        while self.first_step < first_step {
            self.first.pop_front();
            self.first_step += 1;
        }
    }

    /// (1 - p)/(1 + p rho) and (1 + p rho)/(1 - p rho^2) with
    /// p = e^{-j omega tau}: z times the admittance of mode `k` to equal
    /// voltages at both ends and to opposite ones. `None` at a resonance of a
    /// line without losses, where one of them is infinite.
    fn mode_response(&self, k: usize, omega: f64) -> Option<(Complex64, Complex64)> {
        let Mode { rho, tau, .. } = self.modes[k];
        let p = Complex64::from_polar(1.0, -omega * tau);
        let common = Complex64::ONE + p * rho;
        let opposite = Complex64::ONE - p * (rho * rho);
        if common.norm() < 1e-12 || opposite.norm() < 1e-12 {
            return None;
        }
        Some(((Complex64::ONE - p) / common, common / opposite))
    }

    /// The admittance matrices of the two-port per phase: the currents into
    /// the line at an end are `own` times its voltages plus `transfer`
    /// times those of the other end. The same circuit as in the time domain:
    /// E(t) becomes e^{-j omega tau} ((1 + rho)/2 A_o + (1 - rho)/2 A_e).
    fn admittance(&self, omega: f64) -> ([[Complex64; 3]; 3], [[Complex64; 3]; 3]) {
        let mut own = [Complex64::ZERO; 3];
        let mut transfer = [Complex64::ZERO; 3];
        for k in 0..3 {
            let (common, opposite) = self
                .mode_response(k, omega)
                .expect("phasor_singular was checked before the phasor solution");
            own[k] = (common + opposite) / (2.0 * self.modes[k].z);
            transfer[k] = (common - opposite) / (2.0 * self.modes[k].z);
        }
        (in_phases(&self.w, own), in_phases(&self.w, transfer))
    }

    /// The phasors of the currents into the line at both ends, per phase.
    fn currents_phasor(&self, omega: f64, x: &[Complex64]) -> [[Complex64; 3]; 2] {
        let (own, transfer) = self.admittance(omega);
        let v = self.ends.map(|nodes| nodes.map(|n| phasor(x, n)));
        let mut i = [[Complex64::ZERO; 3]; 2];
        for end in 0..2 {
            for p in 0..3 {
                for q in 0..3 {
                    i[end][p] += own[p][q] * v[end][q] + transfer[p][q] * v[1 - end][q];
                }
            }
        }
        i
    }
}

impl Element for BergeronLine {
    fn name(&self) -> &str {
        &self.name
    }
    fn prepare(&mut self, stages: &Stages) -> Result<(), String> {
        let h = stages.time_step_s;
        let mut longest = 1.0_f64;
        for (k, mode) in self.modes.iter().enumerate() {
            let steps = mode.tau / h;
            if steps < 1.0 - TRAVEL_TIME_TOLERANCE {
                return Err(format!(
                    "the travel time of mode {} is {:e} s, shorter than the time step {h:e} s",
                    k + 1,
                    mode.tau
                ));
            }
            // Decimal numbers whose quotient is a whole number to rounding.
            let whole = steps.round();
            if (steps - whole).abs() <= TRAVEL_TIME_TOLERANCE * steps {
                self.delay[k] = Delay::Whole(whole as i64);
                longest = longest.max(whole);
            } else {
                self.delay[k] = Delay::Fraction(steps);
                longest = longest.max(steps.ceil());
            }
        }
        self.step_s = h;
        self.initial_interval = stages.initial_interval;
        // One more than the reach, for a step that is solved again after
        // `restore`.
        self.keep = longest as u64 + 1;
        // Room for the steps kept with two samples each and the grid point
        // before them, and no more: the samples go round in it, so all of
        // it is in use.
        let room = 2 * self.keep as usize + 3;
        self.samples = VecDeque::with_capacity(room);
        self.first = VecDeque::with_capacity(self.keep as usize + 2);
        // No sample yet: the solution at t = 0+ brings the first.
        self.begin([[0.0; 3]; 2]);
        self.samples.clear();
        self.steady = None;
        self.saved = (0, 0, Action::None);
        self.regular = [None; 2];
        for (j, stage) in stages.regular.iter().enumerate().take(2) {
            self.regular[j] = Some(*stage);
            for k in 0..3 {
                self.places[j][k] = Self::back(self.delay[k], stage.time_fraction);
            }
        }
        let read_alike = |a: usize, b: usize| {
            self.modes[a].tau.to_bits() == self.modes[b].tau.to_bits()
                && match (self.delay[a], self.delay[b]) {
                    (Delay::Whole(m), Delay::Whole(n)) => m == n,
                    (Delay::Fraction(c), Delay::Fraction(d)) => c.to_bits() == d.to_bits(),
                    _ => false,
                }
        };
        self.same = [0, 1, 2].map(|k| (0..k).find(|&first| read_alike(first, k)).unwrap_or(k));
        Ok(())
    }
    fn stamp(&self, _alpha: f64, m: &mut dyn Stamp) {
        for nodes in &self.ends {
            stamp_conductance_matrix(m, nodes, &[None; 3], &self.g, 3);
        }
    }
    fn rhs(&self, _t: f64, stage: &Stage, b: &mut [f64]) {
        let j = self.history(stage).j;
        for (nodes, j) in self.ends.iter().zip(&j) {
            for (node, j) in nodes.iter().zip(j) {
                inject(b, *node, None, -j);
            }
        }
    }
    /// Currents 0 to 2: into the line at end 0, phases a, b, c; 3 to 5: at
    /// end 1.
    fn current(&self, k: usize, _t: f64, stage: &Stage, x: &[f64]) -> f64 {
        let (end, p) = (k / 3, k % 3);
        let nodes = &self.ends[end];
        let j = self.history(stage).j[end];
        self.g[p][0] * volt(x, nodes[0])
            + self.g[p][1] * volt(x, nodes[1])
            + self.g[p][2] * volt(x, nodes[2])
            - j[p]
    }
    fn advance(&mut self, stage: &Stage, x: &[f64]) {
        let e = self.history(stage).e;
        self.cache.set(None);
        let v = self.mode_voltages(x);
        let mut a = [[0.0; 3]; 2];
        for end in 0..2 {
            for (k, mode) in self.modes.iter().enumerate() {
                a[end][k] = (1.0 + mode.rho) * v[end][k] - mode.rho * e[end][k];
            }
        }
        if stage.is_initial() {
            // The solutions at t = 0+ are the values at t = 0, whether the
            // other states keep them or not; the last one is the first
            // output row. Its sources are those of the end of the
            // negligible step: it is taken back to t = 0 along the steady
            // state (nothing without one), which leaves a run that starts on
            // the steady state on it to rounding.
            let t = stage.time_fraction * self.initial_interval;
            for (k, mode) in [0, 1, 2].into_iter().enumerate() {
                let (late, at_zero) = (self.before_start(mode, t), self.before_start(mode, 0.0));
                a[0][k] -= late[0] - at_zero[0];
                a[1][k] -= late[1] - at_zero[1];
            }
            self.begin(a);
            return;
        }
        if self.samples.is_empty() {
            let at_zero = [0, 1, 2].map(|k| self.before_start(k, 0.0));
            self.begin([0, 1].map(|end| at_zero.map(|a| a[end])));
        }
        let place = self.place(stage);
        let position = self.step as f64 + place;
        match self.action {
            Action::None => {}
            Action::Taken => {
                // The first solution after an action. The circuit changed at
                // the start of this part, where the last sample (of the
                // solution before the action) lies: a second sample there
                // holds the value just after it. Readings up to that place
                // see the old circuit and later ones the new one, so the
                // front of the action is not spread over the stage, and a
                // travel time of whole steps reads it without a step of
                // delay. Until the next solution is known the value is that
                // of this one.
                let total = self.dropped + self.samples.len() as u64;
                let in_step = total > self.first[(self.step - self.first_step) as usize];
                let fraction = if in_step {
                    self.samples.back().map_or(0.0, |s| s.fraction)
                } else {
                    0.0
                };
                self.samples.push_back(Sample { fraction, a });
                self.action = Action::Sampled(total, self.step as f64 + fraction, position);
            }
            Action::Sampled(number, at, first) => {
                // The second solution after the action: the value just
                // after it is on the straight line through the two. A wave
                // that jumps with the action and is then level stays as it
                // is; one that starts to rise from the old value (a source
                // behind an inductance) is taken back to it, to second
                // order.
                self.action = Action::None;
                if number >= self.dropped && position > first {
                    let back = (first - at) / (position - first);
                    let after = &mut self.samples[(number - self.dropped) as usize].a;
                    for end in 0..2 {
                        for k in 0..3 {
                            after[end][k] += (after[end][k] - a[end][k]) * back;
                        }
                    }
                }
            }
        }
        self.samples.push_back(Sample { fraction: place, a });
        if place >= 1.0 {
            // The step is complete; the next one is whole unless
            // `begin_part` says otherwise.
            self.part = None;
            self.step += 1;
            self.forget();
            self.first
                .push_back(self.dropped + self.samples.len() as u64);
            let regular = self.regular;
            for (j, stage) in regular.iter().enumerate() {
                self.ahead[j] = stage.map(|stage| self.read(&stage, Some(j)));
            }
        }
    }
    fn begin_part(&mut self, t_start: f64, t_end: f64, ends_at_grid_point: bool) {
        let h = self.step_s;
        self.part = Some((
            t_start / h - self.step as f64,
            (t_end - t_start) / h,
            ends_at_grid_point,
        ));
        self.cache.set(None);
        // What was computed ahead is for a step that is solved whole.
        self.ahead = [None; 2];
    }
    fn after_action(&mut self) {
        self.action = Action::Taken;
        // What was computed ahead for this step may read the grid point at
        // its start, which gets a second value.
        self.ahead = [None; 2];
        self.cache.set(None);
    }
    fn save(&mut self) {
        self.saved = (
            self.dropped + self.samples.len() as u64,
            self.step,
            self.action,
        );
    }
    fn restore(&mut self) {
        let (samples, step, action) = self.saved;
        self.action = action;
        self.samples
            .truncate(samples.saturating_sub(self.dropped) as usize);
        self.step = step;
        self.first
            .truncate((step.saturating_sub(self.first_step) + 1) as usize);
        self.cache.set(None);
        self.ahead = [None; 2];
    }
    fn phasor_singular(&self, omega: f64) -> Option<usize> {
        if (0..3).all(|k| self.mode_response(k, omega).is_some()) {
            return None;
        }
        // Any of its nodes names the place; gnd has no unknown.
        Some(self.ends.iter().flatten().find_map(|n| *n).unwrap_or(0))
    }
    fn stamp_phasor(&self, omega: f64, m: &mut dyn Stamp<Complex64>) {
        let (own, transfer) = self.admittance(omega);
        for end in 0..2 {
            let (here, there) = (&self.ends[end], &self.ends[1 - end]);
            for p in 0..3 {
                for q in 0..3 {
                    add(m, here[p], here[q], own[p][q]);
                    add(m, here[p], there[q], transfer[p][q]);
                }
            }
        }
    }
    fn current_phasor(&self, k: usize, omega: f64, x: &[Complex64]) -> Complex64 {
        self.currents_phasor(omega, x)[k / 3][k % 3]
    }
    fn init_from_phasor(&mut self, omega: f64, x: &[Complex64]) {
        // A = V + rho z I of every mode, with the mode currents
        // I_mode = W^-T I_phase written through the admittances of the modes.
        let mut a = [[Complex64::ZERO; 3]; 2];
        let v = self.ends.map(|nodes| nodes.map(|n| phasor(x, n)));
        for k in 0..3 {
            let (common, opposite) = self
                .mode_response(k, omega)
                .expect("phasor_singular was checked before the phasor solution");
            let mode =
                |end: usize| (0..3).fold(Complex64::ZERO, |s, p| s + v[end][p] * self.w[k][p]);
            let (v0, v1) = (mode(0), mode(1));
            // z times the mode currents into the line.
            let (sum, difference) = (common * (v0 + v1), opposite * (v0 - v1));
            let rho = self.modes[k].rho;
            a[0][k] = v0 + (sum + difference) * (0.5 * rho);
            a[1][k] = v1 + (sum - difference) * (0.5 * rho);
        }
        self.steady = Some((omega, a));
        self.cache.set(None);
        self.ahead = [None; 2];
    }
    fn decoupled_ends(&self) -> Option<[Vec<usize>; 2]> {
        Some(self.ends.map(|nodes| {
            let mut unknowns: Vec<usize> = nodes.iter().flatten().copied().collect();
            unknowns.sort_unstable();
            unknowns.dedup();
            unknowns
        }))
    }
}

#[cfg(test)]
mod tests {
    use openkeito_model::{LineMode, Method};

    use super::*;

    const GAMMA: f64 = 1.0 - std::f64::consts::FRAC_1_SQRT_2;

    /// A line whose three modes are the phases, each with Zc = 100 ohm and
    /// travel time `tau`, between the unknowns 0..3 and 3..6.
    fn line(tau: f64, resistance: f64) -> BergeronLine {
        let mode = LineMode {
            surge_impedance_ohm: 100.0,
            travel_time_s: tau,
            resistance_ohm: resistance,
        };
        let modes = LineModes {
            modes: [mode; 3],
            transform: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            resistance_coupling: 0.0,
        };
        BergeronLine::new(
            "L".into(),
            [Some(0), Some(1), Some(2)],
            [Some(3), Some(4), Some(5)],
            &modes,
        )
    }

    /// The solution of a stage with `v` on phase a of end 0, and end 1
    /// terminated so that nothing comes back (v = E/2 gives a = 0): what the
    /// near end sends is then a = 2 v, and the far end receives E = a one
    /// travel time later.
    fn solution(l: &BergeronLine, stage: &Stage, v: f64) -> [f64; 6] {
        let mut x = [0.0; 6];
        x[0] = v;
        x[3] = 0.5 * l.history(stage).e[1][0];
        x
    }

    /// Advances `l` over one step of `stages` with the voltage `v(place)`
    /// (place in steps from t = 0).
    fn step(l: &mut BergeronLine, stages: &[Stage], v: impl Fn(f64) -> f64) {
        let start = l.step as f64;
        for stage in stages {
            let x = solution(l, stage, v(start + stage.time_fraction));
            l.advance(stage, &x);
        }
    }

    /// The line at rest with the solution at t = 0+ taken.
    fn started(tau: f64, method: Method, v0: f64) -> (BergeronLine, Stages) {
        let stages = Stages::new(method, 1e-5);
        let mut l = line(tau, 0.0);
        l.prepare(&stages).unwrap();
        l.save();
        for stage in &stages.initial {
            let x = solution(&l, stage, v0);
            l.advance(stage, &x);
        }
        (l, stages)
    }

    /// E at end 1, phase a: what the far end receives.
    fn received(l: &BergeronLine, stage: &Stage) -> f64 {
        l.history(stage).e[1][0]
    }

    #[test]
    fn a_whole_number_of_steps_reads_the_same_stage_without_interpolation() {
        // tau = 3 steps, written so that tau / h is not 3 to the last bit.
        let (mut l, stages) = started(3e-5 * (1.0 + 1e-13), Method::TwoStageDirk, 0.3);
        assert!(matches!(l.delay[0], Delay::Whole(3)));
        // An irregular sequence: any interpolation would show.
        let v = |place: f64| (place * 1.7).sin() + 0.3;
        for s in 0..12_i64 {
            for stage in &stages.regular {
                let got = received(&l, stage);
                let expected = if s == 2 && stage.time_fraction == 1.0 {
                    // The value of t = 0 is the one of the solution at 0+.
                    2.0 * 0.3
                } else if s < 3 {
                    // Nothing arrives before one travel time.
                    0.0
                } else {
                    2.0 * v((s - 3) as f64 + stage.time_fraction)
                };
                assert_eq!(got, expected, "step {s}, at {}", stage.time_fraction);
            }
            step(&mut l, &stages.regular, v);
        }
        // 3 steps of travel time and 1 more are kept: 2 samples a step and
        // the grid point before them.
        assert_eq!(l.samples.len(), 2 * 4 + 1);
    }

    #[test]
    fn a_fraction_of_a_step_is_interpolated_between_the_neighbouring_samples() {
        // tau = 2.4 steps, trapezoidal rule: samples at the grid points only.
        // Linear interpolation is exact for a ramp.
        let (mut l, stages) = started(2.4e-5, Method::Trapezoidal, 1.0);
        let v = |place: f64| 1.0 + place;
        for _ in 0..4 {
            step(&mut l, &stages.regular, v);
        }
        // The end of step 4 reads at 5 - 2.4 = 2.6.
        let got = received(&l, &stages.regular[0]);
        assert!((got - 2.0 * v(2.6)).abs() < 1e-14, "{got}");
        // The first half step after an action reads at 4.5 - 2.4 = 2.1.
        let got = received(&l, &stages.half_steps[0]);
        assert!((got - 2.0 * v(2.1)).abs() < 1e-14, "{got}");
        // Between two samples of a curve the error is that of the chord:
        // for v = place^2 at 2.6, 2 * 0.6 * 0.4.
        let (mut l, stages) = started(2.4e-5, Method::Trapezoidal, 0.0);
        for _ in 0..4 {
            step(&mut l, &stages.regular, |place| place * place);
        }
        let got = received(&l, &stages.regular[0]);
        assert!((got - 2.0 * (2.6 * 2.6 + 0.6 * 0.4)).abs() < 1e-13, "{got}");
    }

    #[test]
    fn stages_between_grid_points_are_samples_too() {
        // tau = 1.5 steps with 2S-DIRK: the end of a step reads half a step
        // into an earlier one, between its stage at gamma and its end.
        let (mut l, stages) = started(1.5e-5, Method::TwoStageDirk, 0.0);
        // 0 V at the grid points and 0.5 V at the first stages.
        let v = |place: f64| if place.fract() == 0.0 { 0.0 } else { 0.5 };
        for _ in 0..3 {
            step(&mut l, &stages.regular, v);
        }
        // 4 - 1.5 = 2.5: between the stage at 2 + gamma (a = 1) and the grid
        // point 3 (a = 0).
        let got = received(&l, &stages.regular[1]);
        let expected = (1.0 - 0.5) / (1.0 - GAMMA);
        assert!((got - expected).abs() < 1e-14, "{got} {expected}");
    }

    #[test]
    fn a_travel_time_of_one_step_reads_only_what_exists() {
        for method in [Method::TwoStageDirk, Method::Trapezoidal] {
            let (mut l, stages) = started(1e-5, method, 1.0);
            let v = |place: f64| 1.0 + place;
            for _ in 0..3 {
                step(&mut l, &stages.regular, v);
            }
            // The last sample is the grid point 3, the start of the present
            // step. The stage at its end reads exactly that one; the stages
            // before it read earlier (a ramp: interpolation is exact).
            let latest = l.samples.back().unwrap().a[0][0];
            assert_eq!(latest, 2.0 * v(3.0));
            let kinds = [&stages.regular, &stages.after_event, &stages.half_steps];
            for stage in kinds.into_iter().flatten().chain([&stages.probe]) {
                let got = received(&l, stage);
                if stage.time_fraction == 1.0 {
                    assert_eq!(got, latest);
                } else {
                    let expected = 2.0 * v(2.0 + stage.time_fraction);
                    assert!((got - expected).abs() < 1e-14, "{method:?} {stage:?}");
                    assert!(got < latest);
                }
            }
            // Reading does not move the history (the trial solution of the
            // diode states is read, never advanced).
            assert_eq!(l.samples.back().unwrap().a[0][0], latest);
            assert_eq!(l.step, 3);
        }
        // A shorter travel time is refused.
        let mut short = line(0.99e-5, 0.0);
        let error = short
            .prepare(&Stages::new(Method::TwoStageDirk, 1e-5))
            .unwrap_err();
        assert!(error.contains("shorter than the time step"), "{error}");
    }

    #[test]
    fn restore_returns_to_the_saved_history() {
        let (mut l, stages) = started(2e-5, Method::TwoStageDirk, 1.0);
        // Solving t = 0+ again replaces the value at t = 0.
        l.restore();
        assert!(l.samples.is_empty());
        for stage in &stages.initial {
            let x = solution(&l, stage, 2.0);
            l.advance(stage, &x);
        }
        assert_eq!(l.samples.len(), 1);
        assert_eq!(l.samples[0].a[0][0], 4.0);
        // A step solved again from its start.
        let v = |place: f64| place;
        for _ in 0..6 {
            step(&mut l, &stages.regular, v);
        }
        l.save();
        let read = |l: &BergeronLine| -> Vec<f64> {
            stages
                .regular
                .iter()
                .map(|stage| received(l, stage))
                .collect()
        };
        let reads = read(&l);
        let latest = l.samples.back().unwrap().a;
        for _ in 0..2 {
            step(&mut l, &stages.half_steps, |_| 100.0);
            assert_eq!(l.step, 7);
            l.restore();
            assert_eq!(l.step, 6);
            assert_eq!(l.samples.back().unwrap().a, latest);
            let again = read(&l);
            assert_eq!(again, reads);
        }
        step(&mut l, &stages.regular, v);
        assert_eq!(l.step, 7);
        // What the step read two travel times later is the ramp, not the
        // values of the steps that were given up.
        step(&mut l, &stages.regular, v);
        assert_eq!(received(&l, &stages.regular[1]), 2.0 * v(7.0));
    }

    #[test]
    fn an_action_leaves_the_values_before_and_after_it_at_its_grid_point() {
        // tau = 2 steps. 1 V up to grid point 3, where an action takes
        // effect; afterwards 3 V at the first stage and at the end of the
        // step (a jump), or 2 V and 4.83 V (a line through 1.17 V at the
        // grid point: no jump but a slope).
        for (first, second, after) in [
            (3.0, 3.0, 3.0),
            (2.0, 4.0, 2.0 - 2.0 * GAMMA / (1.0 - GAMMA)),
        ] {
            let (mut l, stages) = started(2e-5, Method::TwoStageDirk, 1.0);
            for _ in 0..3 {
                step(&mut l, &stages.regular, |_| 1.0);
            }
            l.save();
            for _ in 0..2 {
                l.after_action();
                step(&mut l, &stages.regular, |place| {
                    if place.fract() == 0.0 {
                        second
                    } else {
                        first
                    }
                });
                // The step that ends at grid point 5 reads values before the
                // action at its first stage (at 2 + gamma) and the value
                // after it at its end (grid point 3).
                assert_eq!(received(&l, &stages.regular[0]), 2.0);
                let at_end = received(&l, &stages.regular[1]);
                assert!((at_end - 2.0 * after).abs() < 1e-14, "{at_end} {after}");
                // The next one reads the first solution after the action.
                step(&mut l, &stages.regular, |_| second);
                assert_eq!(received(&l, &stages.regular[0]), 2.0 * first);
                // Solved again from before the action: the same.
                l.restore();
            }
        }
    }

    #[test]
    fn the_steady_state_is_read_before_t_0() {
        let stages = Stages::new(Method::TwoStageDirk, 1e-5);
        let mut l = line(2.5e-5, 8.0);
        l.prepare(&stages).unwrap();
        let omega = 2.0 * std::f64::consts::PI * 50.0;
        let mut x = [Complex64::ZERO; 6];
        x[0] = Complex64::from_polar(100.0, 0.3);
        x[3] = Complex64::from_polar(90.0, -0.2);
        l.init_from_phasor(omega, &x);
        // The equation of the time domain holds with the phasors:
        // z I = V - e^{-j w tau} ((1 + rho)/2 A_other + (1 - rho)/2 A_own).
        let i = l.currents_phasor(omega, &x);
        let (_, a) = l.steady.unwrap();
        let Mode { z, rho, tau } = l.modes[0];
        let p = Complex64::from_polar(1.0, -omega * tau);
        for end in 0..2 {
            let e = p * (a[1 - end][0] * (0.5 * (1.0 + rho)) + a[end][0] * (0.5 * (1.0 - rho)));
            let residual = i[end][0] * z - (x[3 * end] - e);
            assert!(residual.norm() < 1e-12, "{residual}");
            assert!((a[end][0] - (x[3 * end] + i[end][0] * (rho * z))).norm() < 1e-12);
        }
        // The first stage of the first step reads the sine at gamma h - tau.
        let got = l.history(&stages.regular[0]).e[0][0];
        let t = GAMMA * 1e-5 - tau;
        let wave = |a: Complex64| (a * Complex64::from_polar(2.0_f64.sqrt(), omega * t)).re;
        let expected = 0.5 * (1.0 + rho) * wave(a[1][0]) + 0.5 * (1.0 - rho) * wave(a[0][0]);
        assert!((got - expected).abs() < 1e-12, "{got} {expected}");
    }

    #[test]
    fn the_ends_have_no_common_matrix_entry() {
        struct Entries(Vec<(usize, usize)>);
        impl Stamp for Entries {
            fn add(&mut self, row: usize, col: usize, _value: f64) {
                self.0.push((row, col));
            }
        }
        let l = line(2e-5, 8.0);
        let mut m = Entries(Vec::new());
        l.structure(&mut m);
        let [from, to] = l.decoupled_ends().unwrap();
        assert_eq!(
            (from.as_slice(), to.as_slice()),
            (&[0, 1, 2][..], &[3, 4, 5][..])
        );
        assert!(!m.0.is_empty());
        for (row, col) in m.0 {
            assert_eq!(from.contains(&row), from.contains(&col), "({row}, {col})");
        }
    }

    #[test]
    fn place_stays_at_the_stage_fraction_when_a_grid_time_rounds_down() {
        // (27.0 * 1e-5) / 1e-5 < 27.0 in IEEE 754: a part spanning [t0, t1]
        // must place its stages at their fractions, not collapse to zero.
        let h = 1e-5;
        let step_num = 27_u64;
        assert!((step_num as f64 * h) / h < step_num as f64);
        let (mut l, stages) = started(h, Method::TwoStageDirk, 1.0);
        for _ in 0..step_num {
            step(&mut l, &stages.regular, |_| 1.0);
        }
        let t0 = step_num as f64 * h;
        let t1 = (step_num + 1) as f64 * h;
        l.begin_part(t0, t1, true);
        let p0 = l.place(&stages.regular[0]);
        let p1 = l.place(&stages.regular[1]);
        assert!((p0 - GAMMA).abs() < 1e-14, "p0 = {p0}");
        assert_eq!(p1, 1.0);
    }

    /// Time of `rhs` and `advance` per line and step in a ring of n lines,
    /// against the three elements of a pi section (not run in CI; pin it to
    /// one core, see `bergeron_ring_per_step` in tests/benchmark.rs):
    ///
    /// cargo test --release -p openkeito-core --lib -- --ignored --nocapture --test-threads=1 time_of_the_elements
    #[test]
    #[ignore = "run by hand with --release"]
    fn time_of_the_elements_of_a_ring() {
        use crate::clock::Stopwatch;

        use crate::element::{CapacitanceMatrix, CoupledRl};
        use openkeito_model::{line_modes, LineParameters};

        let per_km = LineParameters::Sequence {
            r1_ohm_per_km: 0.03,
            l1_h_per_km: 0.9e-3,
            c1_f_per_km: 12.7e-9,
            r0_ohm_per_km: 0.2,
            l0_h_per_km: 3e-3,
            c0_f_per_km: 7e-9,
        };
        let stages = Stages::new(Method::TwoStageDirk, 1e-5);
        let steps = 20_000;
        println!("| Lines | Elements | rhs (ns/line/step) | advance (ns/line/step) |");
        println!("|---|---|---|---|");
        for n in [10, 30, 100, 300, 1000] {
            let bus = |k: usize| {
                let k = k % n;
                [Some(3 * k), Some(3 * k + 1), Some(3 * k + 2)]
            };
            let modes = line_modes(&per_km, 10.0).unwrap();
            let (r, l, c) = per_km.matrices();
            let scale = |m: &Mat3, f: f64| m.map(|row| row.map(|v| v * f));
            for kind in ["bergeron", "pi"] {
                let mut elements: Vec<Box<dyn Element>> = Vec::new();
                for k in 0..n {
                    if kind == "bergeron" {
                        elements.push(Box::new(BergeronLine::new(
                            format!("L{k}"),
                            bus(k),
                            bus(k + 1),
                            &modes,
                        )));
                    } else {
                        elements.push(Box::new(CoupledRl::new(
                            format!("L{k}"),
                            3,
                            bus(k),
                            bus(k + 1),
                            scale(&r, 10.0),
                            scale(&l, 10.0),
                        )));
                        for end in [k, k + 1] {
                            elements.push(Box::new(CapacitanceMatrix::new(
                                format!("C{k}"),
                                3,
                                bus(end),
                                [None; 3],
                                scale(&c, 5.0),
                            )));
                        }
                    }
                }
                for e in elements.iter_mut() {
                    e.prepare(&stages).unwrap();
                }
                let mut x = vec![0.0; 3 * n];
                let mut b = vec![0.0; 3 * n];
                for stage in &stages.initial {
                    for e in elements.iter_mut() {
                        e.advance(stage, &x);
                    }
                }
                let mut best = (f64::INFINITY, f64::INFINITY);
                for _ in 0..5 {
                    let (mut in_rhs, mut in_advance) = (0.0, 0.0);
                    for step in 0..steps {
                        for stage in &stages.regular {
                            b.fill(0.0);
                            let started = Stopwatch::start();
                            for e in &elements {
                                e.rhs(0.0, stage, &mut b);
                            }
                            in_rhs += started.elapsed_s();
                            // Any solution: a sine and what the sources hold.
                            for (k, v) in x.iter_mut().enumerate() {
                                *v = (0.003 * step as f64 + k as f64).sin() + 1e-3 * b[k];
                            }
                            let started = Stopwatch::start();
                            for e in elements.iter_mut() {
                                e.advance(stage, &x);
                            }
                            in_advance += started.elapsed_s();
                        }
                    }
                    best = (best.0.min(in_rhs), best.1.min(in_advance));
                }
                let per = 1e9 / (steps as f64 * n as f64);
                println!(
                    "| {n} | {kind} | {:.1} | {:.1} |",
                    best.0 * per,
                    best.1 * per
                );
            }
        }
    }

    /// The ring of tests/benchmark.rs with lines of `line_type`.
    fn ring(n: usize, line_type: &str) -> openkeito_model::Model {
        let w = 2.0 * std::f64::consts::PI * 50.0;
        let mut c = Vec::new();
        for k in 0..n {
            let next = (k + 1) % n;
            c.push(format!(
                r#"{{"type": "{line_type}", "name": "L{k}", "buses": ["b{k}", "b{next}"], "length_km": 10.0,
                "per_km": {{"type": "sequence", "r1_ohm_per_km": 0.03, "l1_h_per_km": 0.9e-3, "c1_f_per_km": 12.7e-9,
                "r0_ohm_per_km": 0.2, "l0_h_per_km": 3e-3, "c0_f_per_km": 7e-9}}}}"#
            ));
            let (p, q, v) = (10e6_f64, 3e6_f64, 66e3_f64);
            let s2 = p * p + q * q;
            let (r, l) = (v * v * p / s2, v * v * q / s2 / w);
            c.push(format!(
                r#"{{"type": "coupled_rl", "name": "LD{k}", "buses": ["b{k}", "gnd"],
                "impedance": {{"type": "sequence", "r1_ohm": {r}, "l1_h": {l}, "r0_ohm": {r}, "l0_h": {l}}}}}"#
            ));
            if k % 10 == 0 {
                let ls = 0.1 / w;
                c.push(format!(
                    r#"{{"type": "three_phase_source", "name": "G{k}", "bus": "g{k}", "line_voltage_rms_v": 11e3,
                    "frequency_hz": 50.0, "impedance": {{"type": "sequence", "r1_ohm": 0.01, "l1_h": {ls}, "r0_ohm": 0.01, "l0_h": {ls}}}}}"#
                ));
                c.push(format!(
                    r#"{{"type": "three_phase_transformer", "name": "T{k}", "buses": ["b{k}", "g{k}"],
                    "vector_group": "YNd11", "rated_voltage_kv": [66.0, 11.0], "rated_power_mva": 100.0,
                    "rated_frequency_hz": 50.0, "impedance_percent": 12.0, "resistance_percent": 0.3,
                    "no_load_current_percent": 0.2, "no_load_loss_kw": 50.0}}"#
                ));
            }
        }
        let text = format!(
            r#"{{"format": "openkeito-model", "version": "0.5",
            "simulation": {{"time_step_s": 1e-5, "end_time_s": 0.2}},
            "components": [{}]}}"#,
            c.join(",\n")
        );
        openkeito_model::load_str(&text).unwrap().model
    }

    /// Where a step of the ring spends its time, per kind of line (not run
    /// in CI; pin it to one core, see `bergeron_ring_per_step` in
    /// tests/benchmark.rs):
    ///
    /// cargo test --release -p openkeito-core --lib -- --ignored --nocapture --test-threads=1 time_of_a_step
    #[test]
    #[ignore = "run by hand with --release"]
    fn time_of_a_step_of_the_ring_by_part() {
        use crate::clock::Stopwatch;

        let stages = Stages::new(Method::TwoStageDirk, 1e-5);
        let steps = 20_000;
        println!("| Buses | Line | Unknowns | rhs | solve | advance | sum (us/step) |");
        println!("|---|---|---|---|---|---|---|");
        for n in [10, 30, 100, 300] {
            for line_type in ["pi_line", "bergeron_line"] {
                let model = ring(n, line_type);
                let mut net = crate::netlist::build(&model);
                for e in net.elements.iter_mut() {
                    e.prepare(&stages).unwrap();
                }
                let solver = net.solver(model.simulation.linear_solver);
                let elements = &mut net.elements;
                let lu = solver
                    .factor(|m| {
                        for e in elements.iter() {
                            e.stamp(stages.alpha(), m);
                        }
                    })
                    .unwrap();
                let unknowns = net.labels.len();
                let mut x = vec![0.0; unknowns];
                let mut b = vec![0.0; unknowns];
                for stage in &stages.initial {
                    for e in elements.iter_mut() {
                        e.advance(stage, &x);
                    }
                }
                let mut best = [f64::INFINITY; 3];
                for _ in 0..5 {
                    let mut spent = [0.0; 3];
                    for step in 0..steps {
                        let t = step as f64 * 1e-5;
                        for stage in &stages.regular {
                            let started = Stopwatch::start();
                            b.fill(0.0);
                            for e in elements.iter() {
                                e.rhs(t, stage, &mut b);
                            }
                            let after_rhs = Stopwatch::start();
                            lu.solve(&b, &mut x);
                            let after_solve = Stopwatch::start();
                            for e in elements.iter_mut() {
                                e.advance(stage, &x);
                            }
                            spent[0] += after_rhs.since(&started);
                            spent[1] += after_solve.since(&after_rhs);
                            spent[2] += after_solve.elapsed_s();
                        }
                    }
                    for (b, s) in best.iter_mut().zip(spent) {
                        *b = b.min(s);
                    }
                }
                let us = best.map(|s| s * 1e6 / steps as f64);
                println!(
                    "| {n} | {line_type} | {unknowns} | {:.2} | {:.2} | {:.2} | {:.2} |",
                    us[0],
                    us[1],
                    us[2],
                    us.iter().sum::<f64>()
                );
            }
        }
    }
}
