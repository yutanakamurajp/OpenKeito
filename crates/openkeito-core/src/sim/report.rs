//! What a run reports: its columns, its switching events and its counts.

use openkeito_model::Diagnostic;
use serde::Serialize;

use crate::element::SwitchAction;
use crate::linalg::Complex64;

/// One recorded quantity (CSV column after `time_s`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Column {
    /// Column header: `V(<node>)`, `I(<component>)`, `U(<component>)` or
    /// `S(<signal>)`.
    pub name: String,
    /// "node_voltage", "branch_current", "branch_voltage" or "signal".
    pub kind: &'static str,
    pub unit: &'static str,
    /// Node, component or signal name.
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SwitchEvent {
    /// Time at which the action took effect: a grid point, or with
    /// `switching` "interpolated" the instant within a step.
    pub time_s: f64,
    pub component: String,
    /// "close" (starts conducting), "open" (stops conducting) or "step" (a
    /// source with a step waveform changes its value).
    pub action: &'static str,
    /// Current interrupted by an opening action, in ampere.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interrupted_current_a: Option<f64>,
}

/// Wall-clock time spent in the parts of a run, in seconds.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Timing {
    /// Building and factorizing system matrices.
    pub factorize_s: f64,
    /// Right-hand sides, solutions, controls and switching logic: the rest
    /// of the time loop.
    pub solve_s: f64,
    /// Evaluating control blocks when detail timing is enabled.
    #[serde(skip)]
    pub control_s: f64,
    /// Handing rows to the output sink (formatting and writing the CSV).
    pub output_s: f64,
    pub total_s: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RunStats {
    pub unknowns: usize,
    /// "dense" or "sparse"; "mixed" when the parts of a divided circuit do
    /// not all use the same.
    pub linear_solver: &'static str,
    /// Number of parts the circuit is divided into at its travelling-wave
    /// lines, each with its own matrix (1: not divided), and the number of
    /// unknowns of the largest.
    pub parts: usize,
    pub largest_part_unknowns: usize,
    /// Number of lines that join parts.
    pub links: usize,
    /// Number of threads that solved the parts.
    pub threads: usize,
    /// Largest condition estimate (reciprocal of the smallest scaled pivot)
    /// of the factorizations. Rounding errors in the solutions are about
    /// this many times the machine epsilon (2.2e-16), relative to their
    /// largest entries.
    pub max_condition_estimate: f64,
    pub steps: u64,
    /// Output rows, including the row at t = 0.
    pub rows: u64,
    pub factorizations: u64,
    pub solves: u64,
    /// Solutions beyond one per stage: the trial solution that fixes the
    /// diode states after every switching action (also when no diode
    /// changes), and one more for every change of the diode states.
    pub resolves: u64,
    /// Largest number of additional solutions in one step.
    pub max_resolves_per_step: u64,
    /// Steps in which the diode states came back to an earlier combination,
    /// after which only one diode was changed per solution.
    pub cycle_fallbacks: u64,
    /// Solutions of a stage beyond the first, made because an element moved
    /// its operating point (the segment of a piecewise-linear
    /// characteristic). 0 in a circuit without such elements.
    pub operating_point_iterations: u64,
    /// Largest number of such additional solutions in one stage.
    pub max_operating_point_iterations: u64,
    /// Switching instants placed within a time step (with `switching`
    /// "interpolated"): each is one instant at which one or more elements
    /// changed.
    pub instants_within_steps: u64,
    /// Time steps that were solved in parts because of such instants.
    pub split_steps: u64,
    /// Time steps in which the limit of instants within one step was
    /// reached; the later instants moved to the next grid point (W270).
    pub instant_limit_steps: u64,
    /// Passes over the control blocks of loops beyond the first pass of
    /// each grid point.
    pub control_loop_iterations: u64,
    pub events: Vec<SwitchEvent>,
    pub timing: Timing,
    /// The steady state the run started from, with `initialization`
    /// "steady_state".
    pub initialization: Option<SteadyStateReport>,
    /// Warnings found during the run.
    pub warnings: Vec<Diagnostic>,
}

/// The phasor of a recorded quantity.
#[derive(Debug, Clone, PartialEq)]
pub struct PhasorValue {
    /// Column name, such as `V(b2.a)` or `I(LINE1.0.a)`.
    pub name: String,
    pub unit: &'static str,
    /// Rms phasor, cosine reference.
    pub value: Complex64,
}

/// The steady state a run started from, and how far the waveforms departed
/// from it before the first switching action.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SteadyStateReport {
    pub frequency_hz: f64,
    /// The waveforms were compared with the steady state from t = 0 to this
    /// time: the end of the run, or the first switching action.
    pub checked_until_s: f64,
    /// Largest |v(t) - sqrt(2) Re(V e^{jwt})| over every node voltage.
    pub max_voltage_deviation_v: f64,
    /// The same, divided by the largest steady-state node voltage amplitude.
    pub relative_voltage_deviation: f64,
    /// The same over the recorded currents (none recorded: `None`).
    pub max_current_deviation_a: Option<f64>,
    pub relative_current_deviation: Option<f64>,
    /// Phasors of every named node voltage and every recorded quantity.
    #[serde(skip)]
    pub phasors: Vec<PhasorValue>,
}

pub(super) fn push_event(stats: &mut RunStats, time_s: f64, component: &str, action: SwitchAction) {
    let (action, interrupted_current_a) = match action {
        SwitchAction::Close => ("close", None),
        SwitchAction::Open(i) => ("open", Some(i)),
        SwitchAction::Step => ("step", None),
    };
    stats.events.push(SwitchEvent {
        time_s,
        component: component.to_string(),
        action,
        interrupted_current_a,
    });
}

/// Removes, among the events of the instant `time_s` at the end of the
/// list, those of the components `names` that undo each other: an element
/// that opened and closed again at one instant has not switched, and of one
/// that changed three times only the first change is left. With switching
/// instants within steps, the states at an instant are settled more than
/// once when the part that follows contradicts them from its start
/// (`step_split`); at a grid point they are compared once, before and after.
pub(super) fn cancel_reversals(stats: &mut RunStats, time_s: f64, names: &[&str]) {
    let first = stats
        .events
        .iter()
        .rposition(|e| e.time_s != time_s)
        .map_or(0, |k| k + 1);
    if stats.events.len() - first < 2 {
        return;
    }
    for name in names {
        let of_name = |e: &SwitchEvent| e.component == *name && e.action != "step";
        let count = stats.events[first..].iter().filter(|e| of_name(e)).count();
        if count < 2 {
            continue;
        }
        // An odd number of changes is one change: the first.
        let mut keep = count % 2;
        let mut index = 0;
        stats.events.retain(|e| {
            index += 1;
            if index <= first || !of_name(e) {
                return true;
            }
            let kept = keep > 0;
            keep = 0;
            kept
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_that_undo_each_other_at_one_instant_leave_no_event() {
        let mut stats = RunStats::default();
        push_event(&mut stats, 1.0, "D1", SwitchAction::Close);
        push_event(&mut stats, 2.0, "D1", SwitchAction::Open(-0.01));
        push_event(&mut stats, 2.0, "D2", SwitchAction::Open(-0.02));
        push_event(&mut stats, 2.0, "S1", SwitchAction::Close);
        push_event(&mut stats, 2.0, "D1", SwitchAction::Close);
        push_event(&mut stats, 2.0, "D2", SwitchAction::Close);
        push_event(&mut stats, 2.0, "D2", SwitchAction::Open(-0.03));
        cancel_reversals(&mut stats, 2.0, &["D1", "D2"]);
        let left: Vec<(f64, &str, &str, Option<f64>)> = stats
            .events
            .iter()
            .map(|e| {
                (
                    e.time_s,
                    e.component.as_str(),
                    e.action,
                    e.interrupted_current_a,
                )
            })
            .collect();
        // D1 opened and closed: nothing. D2 opened, closed and opened: its
        // first opening. The close of D1 at another time and S1 stay.
        assert_eq!(
            left,
            [
                (1.0, "D1", "close", None),
                (2.0, "D2", "open", Some(-0.02)),
                (2.0, "S1", "close", None),
            ]
        );
        // Nothing to do when the last instant has one event.
        cancel_reversals(&mut stats, 3.0, &["D1", "D2"]);
        assert_eq!(stats.events.len(), 3);
    }
}
