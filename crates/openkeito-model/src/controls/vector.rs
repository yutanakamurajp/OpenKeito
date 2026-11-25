//! Blocks that act on a vector in the rotating frame: the limit of its
//! magnitude with a priority, and the pair of PI controllers with a common
//! limit. They are what the current control of a converter needs beyond the
//! scalar blocks.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{input_list, ControlSpec, Scope, StartValue};
use crate::catalog::{control, p, ComponentInfo};
use crate::check::{finite, positive, range_error};
use crate::diagnostic::Diagnostic;

const VECTOR_SIGN: &str = "The block's name is the prefix of its output signals <name>.d and <name>.q. Blocks are evaluated at every grid point; their outputs act on the circuit over the next time step.";

/// Which component of a vector keeps its value when the magnitude is limited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DqPriority {
    /// d is held within +-maximum; q gets what is left,
    /// sqrt(maximum^2 - d^2).
    D,
    /// q is held within +-maximum; d gets what is left.
    Q,
    /// Both are scaled by the same factor: the direction is kept.
    #[default]
    Proportional,
}

impl DqPriority {
    pub fn as_str(self) -> &'static str {
        match self {
            DqPriority::D => "d",
            DqPriority::Q => "q",
            DqPriority::Proportional => "proportional",
        }
    }
}

/// Limit of the magnitude of a vector (d, q): the outputs `<name>.d` and
/// `<name>.q` satisfy d^2 + q^2 <= maximum^2. With priority "d", d is
/// clamped to +-maximum and q to +-sqrt(maximum^2 - d^2); with "q" the other
/// way round; with "proportional" both are scaled by maximum / sqrt(d^2 +
/// q^2). The current limit of a converter with priority of the active or of
/// the reactive current is the first two [WECC REEC_A].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DqLimiter {
    pub name: String,
    /// Signal of the d component.
    pub d: String,
    /// Signal of the q component.
    pub q: String,
    /// Largest magnitude of the output. Must be > 0.
    pub maximum: f64,
    /// "d", "q" or "proportional" (default).
    #[serde(default)]
    pub priority: DqPriority,
}

impl ControlSpec for DqLimiter {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "dq_limiter"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        vec![
            ("d".to_string(), self.d.as_str()),
            ("q".to_string(), self.q.as_str()),
        ]
    }
    fn outputs(&self) -> Vec<&'static str> {
        vec!["d", "q"]
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        if !positive(self.maximum) {
            range_error(
                d,
                p,
                &self.name,
                "maximum",
                self.maximum,
                "The largest magnitude must be > 0, for example 1.1 for a current limit in per unit.",
            );
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "dq_limiter",
            category: "control",
            summary: "Limit of the magnitude of a vector (d, q). Outputs <name>.d, <name>.q with d^2 + q^2 <= maximum^2. Priority \"d\": d clamped to +-maximum, q to +-sqrt(maximum^2 - d^2); \"q\": the other way round; \"proportional\": both scaled by the same factor. The current limit of a converter with priority of the active (d) or reactive (q) current.",
            sign_convention: VECTOR_SIGN,
            parameters: control(vec![
                p("d", "", true, "Name of the signal of the d component."),
                p("q", "", true, "Name of the signal of the q component."),
                p("maximum", "", true, "Largest magnitude of the output, > 0."),
                p("priority", "", false, "d | q | proportional. The component that keeps its value when the magnitude is limited. Default proportional."),
            ]),
            example: json!({"type": "dq_limiter", "name": "i_ref", "d": "i_d_cmd", "q": "i_q_cmd", "maximum": 1.1, "priority": "q"}),
        }
    }
}

/// Two PI controllers, one for each axis of the rotating frame, with a
/// common limit of the magnitude of their output vector:
///
/// ```text
/// u_d = proportional_gain e_d + integral_d + feedforward_d
/// u_q = proportional_gain e_q + integral_q + feedforward_q
/// d(integral)/dt = integral_gain_per_s e     (trapezoidal rule)
/// ```
///
/// With `output_max`, (u_d, u_q) is scaled back to that magnitude, and both
/// integrals are held while the magnitude is beyond the limit and the step
/// of the integrals would drive it further out (no wind-up). The outputs are
/// `<name>.d` and `<name>.q`. This is the current controller of a converter:
/// the errors of the currents in, the voltage reference out, the terminal
/// voltage and the decoupling terms as feedforward, and the largest voltage
/// the dc side allows as the limit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PiDq {
    pub name: String,
    /// The error signals [d, q].
    pub inputs: [String; 2],
    /// Signals [d, q] added to the outputs before the limit. None when
    /// omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedforward: Option<[String; 2]>,
    pub proportional_gain: f64,
    /// Integral gain in 1/s.
    pub integral_gain_per_s: f64,
    /// Values of the integrals [d, q] at t = 0: each a number, or the name
    /// of a signal whose value at t = 0 is taken. Default [0, 0].
    #[serde(default)]
    pub initial_integral: [StartValue; 2],
    /// Largest magnitude of the output vector. No limit when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_max: Option<f64>,
}

impl ControlSpec for PiDq {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "pi_dq"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        let mut v = input_list(&self.inputs);
        if let Some(f) = &self.feedforward {
            v.push(("feedforward[0]".to_string(), f[0].as_str()));
            v.push(("feedforward[1]".to_string(), f[1].as_str()));
        }
        for (k, start) in self.initial_integral.iter().enumerate() {
            if let Some(s) = start.signal() {
                v.push((format!("initial_integral[{k}]"), s));
            }
        }
        v
    }
    fn outputs(&self) -> Vec<&'static str> {
        vec!["d", "q"]
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        let name = self.name.as_str();
        finite(d, p, name, "proportional_gain", self.proportional_gain);
        finite(d, p, name, "integral_gain_per_s", self.integral_gain_per_s);
        for (k, start) in self.initial_integral.iter().enumerate() {
            start.check(d, p, name, &format!("initial_integral[{k}]"));
        }
        if let Some(m) = self.output_max {
            if !positive(m) {
                range_error(
                    d,
                    p,
                    name,
                    "output_max",
                    m,
                    "The largest magnitude of the output must be > 0; omit the field for no limit.",
                );
            }
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "pi_dq",
            category: "control",
            summary: "Two PI controllers, one per axis of the rotating frame, with a common limit: u = proportional_gain*e + integral + feedforward on each axis, d(integral)/dt = integral_gain_per_s*e (trapezoidal rule). Outputs <name>.d, <name>.q. With output_max the vector (u_d, u_q) is scaled back to that magnitude and both integrals are held while the step would drive it further beyond the limit. The current controller of a converter.",
            sign_convention: VECTOR_SIGN,
            parameters: control(vec![
                p("inputs", "", true, "Two signal names [d, q]: the errors."),
                p("feedforward", "", false, "Two signal names [d, q] added to the outputs before the limit. None when omitted."),
                p("proportional_gain", "", true, "Proportional gain of both axes."),
                p("integral_gain_per_s", "1/s", true, "Integral gain of both axes."),
                p("initial_integral", "", false, "Integrals [d, q] at t = 0: each a number or the name of a signal whose value at t = 0 is taken. Default [0, 0]."),
                p("output_max", "", false, "Largest magnitude of the output vector, > 0. None when omitted."),
            ]),
            example: json!({"type": "pi_dq", "name": "e_ref", "inputs": ["i_error_d", "i_error_q"], "feedforward": ["ff_d", "ff_q"], "proportional_gain": 0.48, "integral_gain_per_s": 5.0, "initial_integral": ["r_i_d", 0.0], "output_max": 1.2}),
        }
    }
}
