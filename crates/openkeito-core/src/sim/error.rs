//! The errors that stop a run, and their diagnostics.

use openkeito_model::Diagnostic;

#[derive(Debug)]
pub enum SimError {
    /// The circuit equations have no unique solution.
    Singular {
        time_s: f64,
        unknown: String,
        /// The dependent sources of the part of the circuit whose equations
        /// are singular.
        dependent_sources: Vec<String>,
    },
    /// The phasor equations (steady state) have no unique solution.
    PhasorSingular { frequency_hz: f64, unknown: String },
    /// A solution contains NaN or infinity.
    NonFinite { time_s: f64, unknown: String },
    /// No combination of diode states held at the end of a step.
    SwitchingNotSettled {
        time_s: f64,
        components: Vec<String>,
        resolves: u64,
    },
    /// An element cannot be solved with the stage coefficients of the run.
    IllConditioned { component: String, detail: String },
    /// The operating points of some elements did not agree with the solution
    /// of a stage after the allowed number of solutions.
    OperatingPointNotConverged {
        time_s: f64,
        /// The elements that still moved their operating point.
        components: Vec<String>,
        iterations: u64,
    },
    /// The outputs of the control blocks of a loop still changed after the
    /// allowed number of passes at a grid point.
    ControlLoopNotConverged {
        time_s: f64,
        /// The blocks of the loop.
        blocks: Vec<String>,
        iterations: u64,
    },
    /// A control signal is NaN or infinite.
    SignalNotFinite {
        time_s: f64,
        signal: String,
        /// Type of the block that gives the signal.
        block_type: &'static str,
    },
    /// A user-supplied controller failed or produced an invalid value.
    Controller { time_s: f64, message: String },
    /// The output sink failed.
    Output(std::io::Error),
}

/// The hint of E200: the typical causes, and the gains of the dependent
/// sources when the part of the circuit has some.
fn singular_hint(dependent_sources: &[String]) -> String {
    let mut hint = String::from(
        "Typical causes: a node that is fed only by current sources or is left hanging by an open switch \
         (add a resistor to gnd); a loop made only of voltage sources and closed switches \
         (add a series resistor).",
    );
    if !dependent_sources.is_empty() {
        let mut names: Vec<String> = dependent_sources
            .iter()
            .take(8)
            .map(|n| format!("\"{n}\""))
            .collect();
        if dependent_sources.len() > 8 {
            names.push(format!("{} more", dependent_sources.len() - 8));
        }
        hint.push_str(&format!(
            " The dependent sources {} can be the cause: a loop through them whose gain is 1 \
             (or close to 1) leaves a voltage or a current undetermined. Change a gain, or put a \
             resistor into the loop.",
            names.join(", ")
        ));
    }
    hint
}

impl SimError {
    pub fn to_diagnostic(&self) -> Diagnostic {
        match self {
            SimError::Singular {
                time_s,
                unknown,
                dependent_sources,
            } => Diagnostic::error(
                "E200",
                unknown.clone(),
                format!("the circuit equations are singular at t = {time_s:e} s: {unknown} cannot be determined"),
                singular_hint(dependent_sources),
            ),
            SimError::PhasorSingular {
                frequency_hz,
                unknown,
            } => Diagnostic::error(
                "E203",
                unknown.clone(),
                format!(
                    "the steady-state (phasor) equations at {frequency_hz} Hz are singular: {unknown} \
                     cannot be determined"
                ),
                "Typical causes: a lossless series or parallel resonance at exactly this frequency \
                 (add a small resistance), or a part of the circuit connected only through \
                 capacitors. Set \"initialization\": \"none\" to start from rest instead.",
            ),
            SimError::NonFinite { time_s, unknown } => Diagnostic::error(
                "E201",
                unknown.clone(),
                format!("{unknown} is not a finite number at t = {time_s:e} s"),
                "Check parameter magnitudes and the time step. Rows written before this time are kept.",
            ),
            SimError::SwitchingNotSettled {
                time_s,
                components,
                resolves,
            } => Diagnostic::error(
                "E202",
                components.join(", "),
                format!(
                    "the diode states did not settle at t = {time_s:e} s after {resolves} re-solutions of the step; \
                     changing: {}",
                    components.join(", ")
                ),
                "Look for diodes or switches connected in series or in parallel with nothing between them, \
                 and for loops made only of voltage sources and diodes. Raise on_resistance_ohm, lower \
                 off_resistance_ohm, put an RC snubber (resistor and capacitor in series) across the element, \
                 or reduce time_step_s.",
            ),
            SimError::IllConditioned { component, detail } => {
                let hint = if detail.contains("travel time") {
                    "Reduce time_step_s to at most the shortest travel time, or model the line as a pi_line."
                } else {
                    "Keep the zero-sequence resistance and inductance within about 1e9 times the \
                     positive-sequence values, and the entries of a matrix within that range of each \
                     other. For a branch without a zero-sequence path, a ratio of 1e6 is ample."
                };
                Diagnostic::error(
                    "E204",
                    component.clone(),
                    format!("\"{component}\" cannot be solved: {detail}"),
                    hint,
                )
            }
            SimError::OperatingPointNotConverged {
                time_s,
                components,
                iterations,
            } => Diagnostic::error(
                "E205",
                components.join(", "),
                format!(
                    "the operating points did not converge at t = {time_s:e} s after {iterations} \
                     solutions of the stage; still changing: {}",
                    components.join(", ")
                ),
                "The solution keeps moving between the segments of a piecewise-linear \
                 characteristic (or between the operating points of a machine). Give the \
                 characteristic points with strictly increasing values and no jump between \
                 neighbouring segments, reduce time_step_s, or put a small resistance in series \
                 with the element (or a small capacitance across it) so that its voltage cannot \
                 jump within a step.",
            ),
            SimError::ControlLoopNotConverged {
                time_s,
                blocks,
                iterations,
            } => Diagnostic::error(
                "E250",
                blocks.join(", "),
                format!(
                    "the loop of control blocks {} did not settle at t = {time_s:e} s after \
                     {iterations} passes",
                    blocks.join(" -> ")
                ),
                "The blocks of a loop are evaluated in turn until their outputs agree, which \
                 converges while the gain around the loop within one step, about \
                 time_step_s / 2 times the gain of the loop in 1/s, is below 1: reduce \
                 time_step_s or the gains. At t = 0, a block that starts in the steady state of \
                 its input needs a loop gain below 1 at s = 0: give its initial_input instead. \
                 A delay block of at least one time step in the loop removes the iteration.",
            ),
            SimError::SignalNotFinite {
                time_s,
                signal,
                block_type,
            } => Diagnostic::error(
                "E251",
                signal.clone(),
                format!(
                    "the control signal \"{signal}\" ({block_type}) is not a finite number at \
                     t = {time_s:e} s"
                ),
                "A quotient divided by 0 (give it minimum_denominator), a function left its \
                 domain (sqrt of a negative number, ln of 0: limit its input with a limiter; acos \
                 or asin of a value beyond +/-1: limit it with min(max(x, -1), 1); both sides of \
                 && and || are evaluated, so guard the value itself, as in sqrt(max(x, 0))), a \
                 pll could not be solved (time_step_s / 2 times its proportional_gain must be far \
                 below 1), or a signal grew without bound (check the signs of the feedback and \
                 the gains). Rows written before this time are kept.",
            ),
            SimError::Controller { time_s, message } => Diagnostic::error(
                "E252",
                "controller",
                format!("the controller failed at t = {time_s:e} s: {message}"),
                "Check that the controller callback returns finite numbers for existing control \
                 signals and does not raise an exception. Rows written before this time are kept.",
            ),
            SimError::Output(e) => Diagnostic::error(
                "E210",
                "output",
                format!("writing the result failed: {e}"),
                "Check that the output directory is writable and has free space.",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_have_no_doubled_spaces() {
        let variants = [
            SimError::Singular {
                time_s: 0.0,
                unknown: "V(n1)".to_string(),
                dependent_sources: Vec::new(),
            },
            SimError::Singular {
                time_s: 0.0,
                unknown: "I(E1)".to_string(),
                dependent_sources: vec!["E1".to_string()],
            },
            SimError::PhasorSingular {
                frequency_hz: 50.0,
                unknown: "V(n1)".to_string(),
            },
            SimError::NonFinite {
                time_s: 1e-4,
                unknown: "V(n1)".to_string(),
            },
            SimError::SwitchingNotSettled {
                time_s: 1e-4,
                components: vec!["D1".to_string()],
                resolves: 10,
            },
            SimError::IllConditioned {
                component: "Z1".to_string(),
                detail: "too large ratio".to_string(),
            },
            SimError::IllConditioned {
                component: "L1".to_string(),
                detail: "the travel time of mode 1 is 1e-6 s, shorter than the time step 1e-5 s"
                    .to_string(),
            },
            SimError::OperatingPointNotConverged {
                time_s: 1e-4,
                components: vec!["NL1".to_string()],
                iterations: 50,
            },
            SimError::ControlLoopNotConverged {
                time_s: 1e-4,
                blocks: vec!["a".to_string(), "b".to_string()],
                iterations: 100,
            },
            SimError::SignalNotFinite {
                time_s: 1e-4,
                signal: "s".to_string(),
                block_type: "quotient",
            },
            SimError::Controller {
                time_s: 1e-4,
                message: "callback raised an error".to_string(),
            },
            SimError::Output(std::io::Error::other("disk full")),
        ];
        for err in variants {
            let d = err.to_diagnostic();
            assert!(!d.message.contains("  "), "{}: {}", d.code, d.message);
            assert!(!d.hint.contains("  "), "{}: {}", d.code, d.hint);
        }
    }
}
