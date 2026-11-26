//! The states of diodes, thyristors and controlled switches: thresholds, the solution
//! that settles the diode states after a switching action, and their events.

use super::parts::peak_magnitudes;
use super::report::push_event;
use super::{Lu, RunStats, SimError, Simulator, Work};
use crate::element::{SignalResponse, Stage, SwitchAction, Tolerance};

impl Simulator {
    /// The current of the two-state element `e` in the solution `x`.
    #[inline]
    fn conduction_current(&self, e: usize, x: &[f64]) -> f64 {
        self.layout
            .with(e, x, |x| self.elements[e].conduction_current(x))
    }

    /// Largest node voltage and largest current (sources, switches, diodes)
    /// of the solution `x`.
    pub(super) fn magnitudes(&self, x: &[f64]) -> (f64, f64) {
        if self.layout.is_whole() {
            peak_magnitudes(
                &self.voltage_unknowns,
                &self.current_unknowns,
                &self.two_state,
                x,
                |i| self.elements[i].conduction_current(x),
            )
        } else {
            peak_magnitudes(
                &self.voltage_unknowns,
                &self.current_unknowns,
                &self.two_state,
                x,
                |i| self.conduction_current(i, x),
            )
        }
    }

    /// Thresholds below which a diode voltage or current counts as zero:
    /// 1e-6 of the largest voltage and current seen in the run so far, or
    /// more when the matrix is badly conditioned (1e-14 times the condition
    /// estimate, about the size of the rounding errors). Peaks rather than
    /// the present values, because at a commutation every current of the
    /// circuit can be near zero at once. At the slopes of a switching circuit
    /// the thresholds move a decision by far less than a time step. A part of
    /// the circuit held only by blocking resistances, such as the DC side of
    /// a bridge with every diode off, has a potential the equations fix only
    /// weakly; the condition estimate covers it.
    pub(super) fn tolerance(&self, x: &[f64], lu: &Lu) -> Tolerance {
        let (v, i) = self.magnitudes(x);
        let relative = 1e-6_f64.max(1e-14 * lu.condition_estimate());
        Tolerance {
            voltage: (relative * v.max(self.peaks.0)).max(1e-12),
            current: (relative * i.max(self.peaks.1)).max(1e-12),
        }
    }

    /// The error for diode states that do not settle; names the diodes that
    /// changed during the step or contradict the last solution.
    pub(super) fn not_settled(
        &self,
        t0: f64,
        changed: &[bool],
        bad: &[usize],
        resolves: u64,
    ) -> SimError {
        let components = (0..changed.len())
            .filter(|k| changed[*k] || bad.contains(k))
            .map(|k| self.elements[self.diodes[k]].name().to_string())
            .collect();
        SimError::SwitchingNotSettled {
            time_s: t0,
            components,
            resolves,
        }
    }

    pub(super) fn diode_states(&self) -> Vec<bool> {
        let mut states = Vec::new();
        self.diode_states_into(&mut states);
        states
    }

    /// `diode_states` into a vector that is kept.
    pub(super) fn diode_states_into(&self, states: &mut Vec<bool>) {
        states.clear();
        states.extend(
            self.diodes
                .iter()
                .map(|&i| self.elements[i].conducting().unwrap_or(false)),
        );
    }

    /// Makes every diode state hold just after the grid point `t` at which
    /// a switch changed. The circuit is solved with `probe`, one
    /// backward-Euler step of length 1/alpha that is not kept (states stay as
    /// they are). Backward Euler damps fast modes without changing their
    /// sign, so a current that an opening switch forces into blocking
    /// resistances shows up as the voltage it drives. Contradicted diodes are
    /// changed until none is left: all at once, and after a combination of
    /// states comes back, one at a time (the contradicted diode with the
    /// least index). Leaves in `lu` the factors of the last solution (matrix
    /// with `probe.alpha`; only the parts in which a diode changed are
    /// factorized again); the solution is left in `w.x`.
    pub(super) fn settle(
        &mut self,
        lu: &mut Lu,
        probe: &Stage,
        t: f64,
        w: &mut Work,
        stats: &mut RunStats,
    ) -> Result<(), SimError> {
        let start = self.diode_states();
        let limit = 20 + 2 * self.diodes.len() as u64;
        let mut visited = vec![start.clone()];
        let mut changed = vec![false; self.diodes.len()];
        let mut single = false;
        let mut resolves = 0;
        let t_probe = t + 1.0 / probe.alpha;
        loop {
            self.refresh(lu, probe.alpha, t, stats)?;
            self.solve(lu, probe, t_probe, w, stats)?;
            resolves += 1;
            let tol = self.tolerance(&w.x, lu);
            let bad: Vec<usize> = (0..self.diodes.len())
                .filter(|&k| self.inconsistent(self.diodes[k], &w.x, &tol))
                .collect();
            if bad.is_empty() {
                stats.resolves += resolves;
                stats.max_resolves_per_step = stats.max_resolves_per_step.max(resolves);
                return Ok(());
            }
            if resolves >= limit {
                return Err(self.not_settled(t, &changed, &bad, resolves));
            }
            let flips: Vec<usize> = if single { vec![bad[0]] } else { bad.clone() };
            for &k in &flips {
                self.elements[self.diodes[k]].flip();
                self.touch(self.diodes[k]);
            }
            let mut state = self.diode_states();
            if visited.contains(&state) {
                if single {
                    return Err(self.not_settled(t, &changed, &bad, resolves));
                }
                // The states cycle. From now on change only the first
                // contradicted diode (least-index rule).
                for &k in &flips[1..] {
                    self.elements[self.diodes[k]].flip();
                    self.touch(self.diodes[k]);
                }
                single = true;
                stats.cycle_fallbacks += 1;
                state = self.diode_states();
                if visited.contains(&state) {
                    return Err(self.not_settled(t, &changed, &bad, resolves));
                }
            }
            for (k, (now, before)) in state.iter().zip(&start).enumerate() {
                changed[k] |= now != before;
            }
            changed[bad[0]] = true;
            visited.push(state);
        }
    }

    /// Currents of the diodes in the solution `x`.
    pub(super) fn diode_currents(&self, x: &[f64]) -> Vec<f64> {
        let mut currents = Vec::new();
        self.diode_currents_into(x, &mut currents);
        currents
    }

    /// `diode_currents` into a vector that is kept.
    pub(super) fn diode_currents_into(&self, x: &[f64], currents: &mut Vec<f64>) {
        currents.clear();
        if self.layout.is_whole() {
            let direct = |&i: &usize| self.elements[i].conduction_current(x);
            currents.extend(self.diodes.iter().map(direct));
        } else {
            let mapped = |&i: &usize| self.conduction_current(i, x);
            currents.extend(self.diodes.iter().map(mapped));
        }
    }

    /// Records the diodes whose state differs from `start` as switching
    /// events at time `t`; `current` holds their currents before the change.
    pub(super) fn diode_events(
        &self,
        t: f64,
        start: &[bool],
        current: &[f64],
        stats: &mut RunStats,
    ) {
        let end = self.diode_states();
        for (k, (now, before)) in end.iter().zip(start).enumerate() {
            if now != before {
                let action = if *now {
                    SwitchAction::Close
                } else {
                    SwitchAction::Open(current[k])
                };
                push_event(stats, t, self.elements[self.diodes[k]].name(), action);
            }
        }
    }

    /// Hands the control signals to the elements that read them, at the
    /// grid point `t`. Returns whether an element switched (recorded as an
    /// event) and whether a value that an element holds over the step
    /// changed.
    pub(super) fn read_signals(&mut self, t: f64, stats: &mut RunStats) -> (bool, bool) {
        let (mut switched, mut value_changed) = (false, false);
        for k in 0..self.signal_readers.len() {
            let i = self.signal_readers[k];
            let e = &mut self.elements[i];
            match e.read_signals(&self.controls.values) {
                SignalResponse::Unchanged => {}
                SignalResponse::ValueChanged => value_changed = true,
                // The diode states are settled: see `SignalResponse::Fired`.
                SignalResponse::Fired => switched = true,
                SignalResponse::Switched(action) => {
                    switched = true;
                    let name = e.name().to_string();
                    push_event(stats, t, &name, action);
                    self.touch(i);
                }
            }
        }
        (switched, value_changed)
    }
}
