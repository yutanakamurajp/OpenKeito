//! Switching instants within a time step (`simulation.switching` =
//! "interpolated").
//!
//! The methods of the run are one-step methods: a step needs nothing of the
//! steps before it but the states (and, for the trapezoidal rule, their
//! derivatives). A step from t0 to t1 with a switching instant t* inside is
//! therefore solved as two steps of the same method, [t0, t*] and [t*, t1],
//! with the switching action between them. The solution comes back to the
//! grid point t1 without any interpolation of states, and the output stays
//! on the grid.
//!
//! Instants known before the part is solved (the time of a switch, a fault
//! or a step source; the instant at which a gate signal crosses its
//! threshold) end a part directly. Zero crossings (a diode, a contact that
//! opens at a current zero) are found on the solution of a part: every such
//! element has a zero-crossing function, the part is discarded when one of
//! them changed sign, and solved again up to the crossing.
//!
//! At every instant the logic of a grid point applies: the actions are
//! taken, then the diode states are settled with the trial solution
//! (`settle`). No part is shorter than `min_part`: an instant closer than
//! that to a grid point, or to the instant before it, is moved there.

use openkeito_model::Method;

use super::parts::Layout;
use super::report::{cancel_reversals, push_event};
use super::stages::StepKind;
use super::{GridPoint, Matrix, RunStats, SimError, Simulator, Stages, Steady, Work};
use crate::element::{Element, SignalResponse, Stage, Tolerance};

/// Shortest part of a time step as a fraction of it, when the matrix is
/// well conditioned. The validation knows it: it checks the series
/// impedances for the stage coefficient of such a part (E139).
const MIN_PART: f64 = openkeito_model::SHORTEST_STEP_PART;
/// The fraction above which a part is never refused.
const SAFE_PART: f64 = 0.05;
/// The largest condition estimate allowed for the matrix of a part. A part
/// that covers the fraction f of a step has the stage coefficient alpha / f;
/// conductances alpha C grow and 1/(alpha L) shrink against the blocking and
/// conducting resistances, so the condition estimate grows as 1/f at most.
/// 1e11 keeps the smallest scaled pivot 100 times above the threshold of a
/// singular matrix (1e-13) and the thresholds of the diode states (1e-14
/// times the estimate) at 1e-3.
const MAX_PART_CONDITION: f64 = 1e11;

/// Where the zero-crossing functions are evaluated.
#[derive(Debug, Clone, Copy)]
pub(super) enum At {
    /// The start of a part (the solution there).
    Start,
    /// After the first of two stages.
    Stage,
    /// The end of the part.
    End,
}

/// The zero-crossing functions of the elements (`Element::event_functions`)
/// and their values in the part of a step being solved.
#[derive(Debug, Default)]
pub(super) struct Crossings {
    /// Element, index of its first function, number of its functions.
    owners: Vec<(usize, usize, usize)>,
    /// Element and index within the element, per function.
    functions: Vec<(usize, usize)>,
    /// [value, threshold] per function.
    start: Vec<[f64; 2]>,
    stage: Vec<[f64; 2]>,
    end: Vec<[f64; 2]>,
    /// `start` holds the values at the end of the part solved last, which
    /// are those at the start of the next part unless an action follows.
    pub start_valid: bool,
}

/// The fraction of a part at which a zero-crossing function, positive while
/// the state holds, crossed zero, given that its value `end` at the end of
/// the part contradicts the state. `start` is its value at the start of the
/// part when that is known, `stage` the fraction and the value after the
/// first of two stages. Linear interpolation between neighbouring values;
/// without a start value, the line through the stage value and the end
/// value is followed back. 0 means the state did not hold from the start.
/// With one stage and no start value nothing places the crossing; it is
/// left to the end of the part (1).
fn crossing(start: Option<f64>, stage: Option<(f64, f64)>, end: f64) -> f64 {
    let start = start.filter(|g| g.is_finite());
    match stage.filter(|s| s.1.is_finite()) {
        Some((phi, mid)) => {
            if mid > 0.0 {
                phi + (1.0 - phi) * mid / (mid - end)
            } else {
                match start {
                    Some(g0) if g0 > 0.0 => phi * g0 / (g0 - mid),
                    Some(_) => 0.0,
                    None if end < mid => (phi + (1.0 - phi) * mid / (mid - end)).max(0.0),
                    None => 0.0,
                }
            }
        }
        None => match start {
            Some(g0) if g0 > 0.0 => g0 / (g0 - end),
            Some(_) => 0.0,
            None => 1.0,
        },
    }
}

impl Crossings {
    pub fn of(elements: &[Box<dyn Element>]) -> Crossings {
        let mut c = Crossings::default();
        for (i, e) in elements.iter().enumerate() {
            let n = e.event_count();
            if n > 0 {
                c.owners.push((i, c.functions.len(), n));
                c.functions.extend((0..n).map(|k| (i, k)));
            }
        }
        let n = c.functions.len();
        c.start = vec![[f64::INFINITY, 0.0]; n];
        c.stage = c.start.clone();
        c.end = c.start.clone();
        c
    }

    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    pub fn len(&self) -> usize {
        self.functions.len()
    }

    /// The part just evaluated at its end was solved to that end: its end
    /// values are the start values of the part that follows.
    pub fn accept(&mut self) {
        std::mem::swap(&mut self.start, &mut self.end);
        self.start_valid = true;
    }

    /// Element and index within the element of function `n`.
    pub fn function(&self, n: usize) -> (usize, usize) {
        self.functions[n]
    }

    /// Evaluates every function in the solution `x`. The thresholds are
    /// only needed at the end of a part.
    pub fn evaluate(
        &mut self,
        elements: &[Box<dyn Element>],
        layout: &Layout,
        x: &[f64],
        tol: Option<&Tolerance>,
        at: At,
    ) {
        let zero = Tolerance {
            voltage: 0.0,
            current: 0.0,
        };
        let tol = tol.unwrap_or(&zero);
        let g = match at {
            At::Start => &mut self.start,
            At::Stage => &mut self.stage,
            At::End => &mut self.end,
        };
        for &(i, first, n) in &self.owners {
            layout.with(i, x, |x| {
                elements[i].event_functions(x, tol, &mut g[first..first + n])
            });
        }
    }

    /// The functions whose value at the end of the part contradicts the
    /// state, each with the fraction of the part at which it crossed zero.
    pub fn locate(
        &self,
        start_known: bool,
        stage_fraction: Option<f64>,
        out: &mut Vec<(usize, f64)>,
    ) {
        for (n, end) in self.end.iter().enumerate() {
            if end[0] < -end[1] {
                let start = start_known.then(|| self.start[n][0]);
                let stage = stage_fraction.map(|phi| (phi, self.stage[n][0]));
                out.push((n, crossing(start, stage, end[0]).clamp(0.0, 1.0)));
            }
        }
    }
}

/// A switching instant that is known before a part is solved.
pub(super) struct Scheduled {
    /// The instant; the start of the part when it lies within `min_part`
    /// after it.
    pub at: f64,
    /// The time at which a gate signal has crossed its threshold, when that
    /// is (one of) the reasons.
    pub gate: Option<f64>,
}

impl Simulator {
    /// True when switching instants are placed within time steps.
    pub(super) fn places_instants(&self) -> bool {
        self.switching == openkeito_model::Switching::Interpolated
    }

    /// The shortest part of a time step, in seconds, at time `t`: 1e-6 of
    /// the step, more when the matrix of the regular stages is badly
    /// conditioned (so that the estimate for the part stays below
    /// `MAX_PART_CONDITION`; never more than 5 % of the step), and not less
    /// than the rounding of the time itself.
    pub(super) fn min_part(&self, t: f64) -> f64 {
        let fraction =
            (self.regular_condition.get() / MAX_PART_CONDITION).clamp(MIN_PART, SAFE_PART);
        (fraction * self.h).max(self.time_tolerance(t))
    }

    /// Largest number of switching instants placed within one time step.
    /// Beyond it the states stay as they are until the grid point, where the
    /// rules of switching at grid points take over: the step always ends.
    pub(super) fn instant_limit(&self) -> u64 {
        20 + 2 * (self.two_state.len() + self.crossings.len()) as u64
    }

    /// True when an element would switch on the control signals previewed
    /// at time `t`.
    fn would_switch_at(&self, t: f64, w: &mut Work) -> bool {
        self.controls
            .preview_with_overrides(t, &mut w.preview, &w.overrides);
        self.signal_readers
            .iter()
            .any(|&i| self.elements[i].would_switch(&w.preview))
    }

    /// The earliest switching instant after `ta` that is known before the
    /// part from `ta` is solved and leaves at least `min_part` up to the
    /// grid point `t1`: a time at which an element acts on its own, or the
    /// instant from which an element would switch on the previewed control
    /// signals (found by bisection, to 2^-40 of the step). Later instants
    /// are left to the grid point.
    pub(super) fn next_scheduled(
        &mut self,
        ta: f64,
        t1: f64,
        min_part: f64,
        w: &mut Work,
    ) -> Option<Scheduled> {
        // Times up to ta + min_part were taken by `pre_step` at ta.
        while self
            .action_times
            .get(self.next_action)
            .is_some_and(|&t| t <= ta + min_part)
        {
            self.next_action += 1;
        }
        let mut at = self
            .action_times
            .get(self.next_action)
            .copied()
            .filter(|&t| t < t1 - min_part);
        let mut gate = None;
        if !self.signal_readers.is_empty() && !self.controls.is_empty() {
            let end = at.unwrap_or(t1);
            if self.would_switch_at(end, w) {
                let (mut lo, mut hi) = (ta, end);
                for _ in 0..40 {
                    let mid = 0.5 * (lo + hi);
                    if self.would_switch_at(mid, w) {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                if hi < t1 - min_part {
                    gate = Some(hi);
                    at = Some(if hi <= ta + min_part { ta } else { hi });
                }
            }
        }
        at.map(|at| Scheduled { at, gate })
    }

    /// Computes what the elements need for the stage coefficient `alpha`
    /// when it is not one of `stages`.
    fn use_alpha(&mut self, alpha: f64, stages: &Stages) -> Result<(), SimError> {
        if stages.alphas().contains(&alpha) {
            return Ok(());
        }
        for k in 0..self.elements.len() {
            let made = self.made(k);
            let e = &mut self.elements[made];
            e.prepare_alpha(alpha)
                .map_err(|detail| SimError::IllConditioned {
                    component: e.name().to_string(),
                    detail,
                })?;
        }
        Ok(())
    }

    /// Makes `matrix` hold the factors for `alpha` and the present states;
    /// `dirty` says that a state changed since the last factorization.
    pub(super) fn set_matrix(
        &mut self,
        matrix: &mut Matrix,
        alpha: f64,
        dirty: bool,
        t: f64,
        stages: &Stages,
        stats: &mut RunStats,
    ) -> Result<(), SimError> {
        if dirty || matrix.alpha != alpha {
            self.use_alpha(alpha, stages)?;
            self.refresh(&mut matrix.lu, alpha, t, stats)?;
            matrix.alpha = alpha;
        }
        Ok(())
    }

    /// The switching actions at an instant `t` within a step, to which the
    /// circuit has been brought: the zero crossings found at fractions up
    /// to `theta_max` of the part just solved, the actions of
    /// time-controlled elements due at `t`, and the elements that switch on
    /// the control signals previewed at `gate`. The diode states are then
    /// settled with `probe`, as at a grid point.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn act(
        &mut self,
        t: f64,
        t1: f64,
        theta_max: Option<f64>,
        gate: Option<f64>,
        probe: &Stage,
        stages: &Stages,
        matrix: &mut Matrix,
        w: &mut Work,
        steady: &mut Option<Steady>,
        stats: &mut RunStats,
    ) -> Result<GridPoint, SimError> {
        let start = self.diode_states();
        let start_current = self.diode_currents(&w.x);
        let mut changed = false;
        let mut reversed = Vec::new();
        if let Some(max) = theta_max {
            for n in 0..self.located.len() {
                let (function, theta) = self.located[n];
                if theta <= max {
                    let (i, k) = self.crossings.function(function);
                    let is_diode = if let Some(d) = self.diodes.iter().position(|&d| d == i) {
                        if start[d] {
                            reversed.push(d);
                        }
                        changed = true;
                        true
                    } else {
                        false
                    };
                    let before = w.actions.len();
                    self.elements[i].take_event(k, &mut w.actions);
                    if is_diode || w.actions.len() > before {
                        self.touch(i);
                    }
                }
            }
        }
        self.located.clear();
        let tol = self.min_part(t);
        changed |= self.pre_step(t, tol, &mut w.actions);
        for (name, action) in w.actions.drain(..) {
            changed = true;
            push_event(stats, t, &name, action);
        }
        if let Some(tg) = gate {
            self.controls
                .preview_with_overrides(tg, &mut w.preview, &w.overrides);
            for n in 0..self.signal_readers.len() {
                let i = self.signal_readers[n];
                let e = &mut self.elements[i];
                // Only the elements that switch read the previewed signals:
                // an element that takes a value from its signals (a
                // controlled source) reads them at grid points only.
                if !e.would_switch(&w.preview) {
                    continue;
                }
                if let SignalResponse::Switched(action) = e.read_signals(&w.preview) {
                    changed = true;
                    let name = e.name().to_string();
                    push_event(stats, t, &name, action);
                    self.touch(i);
                }
            }
        }
        let mut out = GridPoint {
            changed,
            ..GridPoint::default()
        };
        if changed {
            self.crossings.start_valid = false;
            stats.instants_within_steps += 1;
            for e in self.elements.iter_mut() {
                e.after_action();
            }
            if let Some(s) = steady.as_mut() {
                s.active = false;
            }
        }
        if changed && !self.diodes.is_empty() {
            self.use_alpha(probe.alpha, stages)?;
            if self.stage_history {
                // The trial solution lies in the rest of the step.
                for e in self.elements.iter_mut() {
                    e.begin_part(t, t1, true);
                }
            }
            self.settle(&mut matrix.lu, probe, t, w, stats)?;
            matrix.alpha = probe.alpha;
            out.fresh = true;
            self.diode_events(t, &start, &start_current, stats);
            let now = self.diode_states();
            out.opened_at_zero = reversed.iter().any(|&k| !now[k]);
            // A diode that was opened at this instant and whose state the
            // part after it contradicted from its start is closed here, at
            // the same instant (or the other way round): it has not
            // switched, and the two events are taken back.
            let names: Vec<&str> = self
                .diodes
                .iter()
                .map(|&i| self.elements[i].name())
                .collect();
            cancel_reversals(stats, t, &names);
        }
        Ok(out)
    }

    /// The stages of the part that follows what happened at its start.
    pub(super) fn kind_after(&self, after_action: bool, opened_at_zero: bool) -> StepKind {
        // See `step`: backward Euler after a diode cut a current under
        // 2S-DIRK.
        if opened_at_zero && self.method == Method::TwoStageDirk {
            StepKind::HalfSteps
        } else if after_action {
            StepKind::AfterEvent
        } else {
            StepKind::Regular
        }
    }

    /// One time step with switching instants within it: the actions at the
    /// grid point, then the parts between the instants up to the next grid
    /// point, and the controls there.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn step_split<C: super::ControllerHook>(
        &mut self,
        step: u64,
        call_controller: bool,
        stages: &Stages,
        matrix: &mut Matrix,
        w: &mut Work,
        steady: &mut Option<Steady>,
        controller: &mut C,
        stats: &mut RunStats,
    ) -> Result<(), SimError> {
        let h = self.h;
        let t0 = step as f64 * h;
        let t1 = (step + 1) as f64 * h;
        let at = if step > 0 {
            self.grid_point(t0, stages, matrix, w, steady, stats)?
        } else {
            GridPoint::default()
        };
        let min_part = self.min_part(t1);
        // The trial solution that settles the diode states at the start of
        // the part [t, t1]. Its interval is not made shorter than a part
        // that is always allowed.
        let probe_at = |t: f64| {
            if t == t0 {
                stages.probe
            } else {
                stages.probe_of_part(((t1 - t) / h).max(SAFE_PART))
            }
        };
        // What the part to be solved next follows: an action at its start
        // (then `w.x` is not the solution there).
        let mut after_action = at.changed;
        let mut opened_at_zero = at.opened_at_zero;
        // A state changed since `matrix.lu` was computed.
        let mut dirty = at.changed && !at.fresh;
        let limit = self.instant_limit();
        let mut instants = 0;
        let mut ta = t0;
        let mut split = false;
        loop {
            let placing = instants < limit;
            let mut te = t1;
            let mut gate = None;
            if placing {
                if let Some(s) = self.next_scheduled(ta, t1, min_part, w) {
                    if s.at <= ta {
                        let probe = probe_at(ta);
                        let r = self.act(
                            ta, t1, None, s.gate, &probe, stages, matrix, w, steady, stats,
                        )?;
                        instants += 1;
                        after_action |= r.changed;
                        opened_at_zero |= r.opened_at_zero;
                        if r.changed {
                            dirty = !r.fresh;
                        }
                        continue;
                    }
                    te = s.at;
                    gate = s.gate;
                }
            } else if instants == limit {
                stats.instant_limit_steps += 1;
                instants += 1;
            }
            let whole = ta == t0 && te == t1;
            let length = te - ta;
            let kind = self.kind_after(after_action, opened_at_zero);
            let part = stages.part(kind, if whole { 1.0 } else { length / h });
            if !whole {
                split = true;
            }
            if !whole || (self.stage_history && instants > 0) {
                for e in self.elements.iter_mut() {
                    e.begin_part(ta, te, te == t1);
                }
            }
            self.set_matrix(matrix, part.alpha(), dirty, ta, stages, stats)?;
            dirty = false;
            let locate = placing && !self.crossings.is_empty();
            let stages_now = part.stages();
            // A stage before the last one that commits changes what the
            // part starts from.
            let save = locate
                && (self.stage_history
                    || stages_now[..stages_now.len() - 1].iter().any(|s| s.commit));
            if save {
                for e in self.elements.iter_mut() {
                    e.save();
                }
            }
            // A crossing later than this is taken at the end of the part.
            let stop = 1.0 - min_part / length;
            let found = if whole
                && kind == StepKind::Regular
                && !locate
                && self.plain
                && !self.layout.is_whole()
            {
                // A whole step without an action at its start and without
                // zero crossings to look for: the parts on their own.
                self.crossings.start_valid = false;
                let t2 = (step + 2) as f64 * h;
                self.plain_step(&mut matrix.lu, stages_now, ta, te, t2, w, stats)?;
                None
            } else {
                self.attempt_located(
                    &mut matrix.lu,
                    stages_now,
                    ta,
                    te,
                    locate,
                    !after_action,
                    Some(stop),
                    w,
                    stats,
                )?
            };
            match found {
                Some(theta) if theta < stop => {
                    // The part is discarded and solved again up to the
                    // crossing.
                    if save {
                        for e in self.elements.iter_mut() {
                            e.restore();
                        }
                    }
                    let mut t_star = ta;
                    if theta * length > min_part {
                        t_star = ta + theta * length;
                        split = true;
                        let first = stages.part(kind, (t_star - ta) / h);
                        for e in self.elements.iter_mut() {
                            e.begin_part(ta, t_star, false);
                        }
                        self.set_matrix(matrix, first.alpha(), false, ta, stages, stats)?;
                        self.attempt_located(
                            &mut matrix.lu,
                            first.stages(),
                            ta,
                            t_star,
                            false,
                            false,
                            None,
                            w,
                            stats,
                        )?;
                        after_action = false;
                        opened_at_zero = false;
                    }
                    let theta_max = theta + min_part / length;
                    let probe = probe_at(t_star);
                    let r = self.act(
                        t_star,
                        t1,
                        Some(theta_max),
                        None,
                        &probe,
                        stages,
                        matrix,
                        w,
                        steady,
                        stats,
                    )?;
                    instants += 1;
                    after_action |= r.changed;
                    opened_at_zero |= r.opened_at_zero;
                    if r.changed {
                        dirty = !r.fresh;
                    }
                    ta = t_star;
                }
                _ => {
                    // The part is solved. Crossings near its end are left
                    // to the grid point, or taken with the instant that
                    // ends the part.
                    if te == t1 {
                        break;
                    }
                    let probe = probe_at(te);
                    let r = self.act(
                        te,
                        t1,
                        found.map(|_| 1.0),
                        gate,
                        &probe,
                        stages,
                        matrix,
                        w,
                        steady,
                        stats,
                    )?;
                    instants += 1;
                    after_action = r.changed;
                    opened_at_zero = r.opened_at_zero;
                    if r.changed {
                        dirty = !r.fresh;
                    }
                    ta = te;
                }
            }
        }
        self.end_of_step(t1, split, call_controller, w, steady, controller, stats)
    }
}

#[cfg(test)]
mod tests {
    use super::crossing;

    #[test]
    fn a_crossing_is_placed_between_the_values_around_it() {
        // One stage: between the start and the end.
        assert_eq!(crossing(Some(1.0), None, -1.0), 0.5);
        assert_eq!(crossing(Some(-0.1), None, -1.0), 0.0);
        // Without a start value nothing places it.
        assert_eq!(crossing(None, None, -1.0), 1.0);
        // Two stages (the first at 0.3): after the stage,
        assert!((crossing(Some(5.0), Some((0.3, 1.0)), -1.0) - 0.65).abs() < 1e-15);
        // before it,
        assert!((crossing(Some(1.0), Some((0.3, -1.0)), -3.0) - 0.15).abs() < 1e-15);
        // and before it without a start value: the line through the stage
        // (0.3, -1) and the end (1, -8) crosses zero at 0.2.
        assert!((crossing(None, Some((0.3, -1.0)), -8.0) - 0.2).abs() < 1e-15);
        // A line that would cross before the start: the state never held.
        assert_eq!(crossing(None, Some((0.3, -1.0)), -1.5), 0.0);
        // A function that was not armed at the start (infinity) counts as
        // unknown there.
        assert!((crossing(Some(f64::INFINITY), Some((0.5, 1.0)), -1.0) - 0.75).abs() < 1e-15);
    }
}
