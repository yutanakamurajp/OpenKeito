//! The transformations between phase quantities and the rotating dq0 frame,
//! and the phase-locked loop.
//!
//! With the angle theta of the d axis ahead of the axis of phase a, and the q
//! axis 90 degrees ahead of the d axis:
//!
//! ```text
//! d    =  2/3 (a cos(theta) + b cos(theta - 2 pi/3) + c cos(theta + 2 pi/3))
//! q    = -2/3 (a sin(theta) + b sin(theta - 2 pi/3) + c sin(theta + 2 pi/3))
//! zero =  1/3 (a + b + c)
//! ```
//!
//! The factor 2/3 keeps amplitudes: for balanced phases with the phasor X
//! (rms, cosine reference, sequence a, b, c) and theta = w t + theta0,
//! d + j q = sqrt(2) X e^(-j theta0).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{input_list, ControlSpec, Scope};
use crate::catalog::{control, p, ComponentInfo};
use crate::check::{finite, positive, range_error};
use crate::diagnostic::Diagnostic;

const DQ_SIGN: &str = "The block's name is the prefix of its output signals. theta is the angle of the d axis ahead of the axis of phase a, in radians; the q axis is 90 degrees ahead of the d axis; amplitudes are kept (factor 2/3). For balanced phases with phasor X (rms, cosine reference, sequence a, b, c) and theta = w t + theta0: d + j q = sqrt(2) X e^(-j theta0). Blocks are evaluated at every grid point; their outputs act on the circuit over the next time step.";

/// Phase quantities to the rotating frame: the signals `<name>.d`,
/// `<name>.q` and `<name>.zero` (see the conventions of this module).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AbcToDq0 {
    pub name: String,
    /// The signals of the phases [a, b, c].
    pub inputs: [String; 3],
    /// Signal of the angle theta of the d axis, in radians.
    pub angle: String,
}

impl ControlSpec for AbcToDq0 {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "abc_to_dq0"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        let mut v = input_list(&self.inputs);
        v.push(("angle".to_string(), self.angle.as_str()));
        v
    }
    fn outputs(&self) -> Vec<&'static str> {
        vec!["d", "q", "zero"]
    }
    fn check(&self, _p: &str, _scope: &Scope, _d: &mut Vec<Diagnostic>) {}
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "abc_to_dq0",
            category: "control",
            summary: "Phase quantities to the rotating frame. Outputs <name>.d, <name>.q, <name>.zero: d = 2/3 (a cos(theta) + b cos(theta - 2pi/3) + c cos(theta + 2pi/3)), q = -2/3 (a sin(theta) + b sin(theta - 2pi/3) + c sin(theta + 2pi/3)), zero = (a + b + c)/3.",
            sign_convention: DQ_SIGN,
            parameters: control(vec![
                p("inputs", "", true, "Three signal names [a, b, c]."),
                p("angle", "rad", true, "Name of the signal of the angle theta."),
            ]),
            example: json!({"type": "abc_to_dq0", "name": "v_dq", "inputs": ["v.a", "v.b", "v.c"], "angle": "pll.theta"}),
        }
    }
}

/// Rotating frame to phase quantities, the inverse of `abc_to_dq0`: the
/// signals `<name>.a`, `<name>.b` and `<name>.c`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Dq0ToAbc {
    pub name: String,
    /// Signal of the d component.
    pub d: String,
    /// Signal of the q component.
    pub q: String,
    /// Signal of the zero component. 0 when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zero: Option<String>,
    /// Signal of the angle theta of the d axis, in radians.
    pub angle: String,
}

impl ControlSpec for Dq0ToAbc {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "dq0_to_abc"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        let mut v = vec![
            ("d".to_string(), self.d.as_str()),
            ("q".to_string(), self.q.as_str()),
        ];
        if let Some(z) = &self.zero {
            v.push(("zero".to_string(), z.as_str()));
        }
        v.push(("angle".to_string(), self.angle.as_str()));
        v
    }
    fn outputs(&self) -> Vec<&'static str> {
        vec!["a", "b", "c"]
    }
    fn check(&self, _p: &str, _scope: &Scope, _d: &mut Vec<Diagnostic>) {}
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "dq0_to_abc",
            category: "control",
            summary: "Rotating frame to phase quantities, the inverse of abc_to_dq0. Outputs <name>.a, <name>.b, <name>.c: a = d cos(theta) - q sin(theta) + zero, b and c with theta - 2pi/3 and theta + 2pi/3.",
            sign_convention: DQ_SIGN,
            parameters: control(vec![
                p("d", "", true, "Name of the signal of the d component."),
                p("q", "", true, "Name of the signal of the q component."),
                p("zero", "", false, "Name of the signal of the zero component. 0 when omitted."),
                p("angle", "rad", true, "Name of the signal of the angle theta."),
            ]),
            example: json!({"type": "dq0_to_abc", "name": "v_ref", "d": "v_d_ref", "q": "v_q_ref", "zero": "v_0_ref", "angle": "pll.theta"}),
        }
    }
}

/// Phase-locked loop in the synchronous reference frame. The inputs are
/// transformed with the loop's own angle theta; a PI controller turns
/// q / rated_amplitude into a frequency deviation, which is integrated to
/// theta:
///
/// ```text
/// omega = 2 pi nominal_frequency_hz + proportional_gain e + integral_gain_per_s (integral of e),
/// e = q / rated_amplitude,   d(theta)/dt = omega
/// ```
///
/// Locked, theta is the angle of the phase-a cosine of the positive-sequence
/// input (q = 0, d its peak value). For small errors and an input of rated
/// amplitude the error of the angle follows s^2/(s^2 + Kp s + Ki): natural
/// frequency sqrt(Ki) rad/s, damping Kp / (2 sqrt(Ki)).
///
/// Outputs: `<name>.theta` (rad, in [0, 2 pi)), `<name>.frequency_hz`
/// (omega / 2 pi), `<name>.d` and `<name>.q` (the inputs in the loop's frame).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Pll {
    pub name: String,
    /// The signals of the phases [a, b, c].
    pub inputs: [String; 3],
    /// Frequency around which the loop works, in Hz. Must be > 0.
    pub nominal_frequency_hz: f64,
    /// Peak value of the phase quantities at which the gains are meant; the
    /// error is q divided by it. Must be > 0.
    pub rated_amplitude: f64,
    /// Kp in rad/s per unit of error.
    pub proportional_gain: f64,
    /// Ki in rad/s^2 per unit of error.
    pub integral_gain_per_s: f64,
    /// theta at t = 0 in radians. When omitted: the angle of the inputs at
    /// t = 0, so that the loop starts locked (q = 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_angle_rad: Option<f64>,
}

impl ControlSpec for Pll {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "pll"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        input_list(&self.inputs)
    }
    fn outputs(&self) -> Vec<&'static str> {
        vec!["theta", "frequency_hz", "d", "q"]
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        let name = self.name.as_str();
        if !positive(self.nominal_frequency_hz) {
            range_error(
                d,
                p,
                name,
                "nominal_frequency_hz",
                self.nominal_frequency_hz,
                "Frequency must be > 0 Hz, for example 50 or 60.",
            );
        }
        if !positive(self.rated_amplitude) {
            range_error(
                d,
                p,
                name,
                "rated_amplitude",
                self.rated_amplitude,
                "Use the peak value of the phase quantities, > 0: sqrt(2/3) times the \
                 line-to-line rms voltage, or 1 for inputs in per unit.",
            );
        }
        finite(d, p, name, "proportional_gain", self.proportional_gain);
        finite(d, p, name, "integral_gain_per_s", self.integral_gain_per_s);
        if let Some(v) = self.initial_angle_rad {
            finite(d, p, name, "initial_angle_rad", v);
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "pll",
            category: "control",
            summary: "Phase-locked loop in the synchronous reference frame: omega = 2 pi nominal_frequency_hz + PI(q / rated_amplitude), d(theta)/dt = omega, with q of the inputs in the loop's own frame. Outputs <name>.theta (rad, in [0, 2 pi)), <name>.frequency_hz, <name>.d, <name>.q. Locked: q = 0 and theta is the angle of the phase-a cosine. Small errors follow s^2/(s^2 + Kp s + Ki).",
            sign_convention: DQ_SIGN,
            parameters: control(vec![
                p("inputs", "", true, "Three signal names [a, b, c]."),
                p("nominal_frequency_hz", "Hz", true, "Frequency around which the loop works, > 0."),
                p("rated_amplitude", "", true, "Peak value of the phase quantities at which the gains are meant, > 0; the error is q divided by it."),
                p("proportional_gain", "rad/s", true, "Kp: damping Kp/(2 sqrt(Ki))."),
                p("integral_gain_per_s", "rad/s^2", true, "Ki: natural frequency sqrt(Ki) rad/s."),
                p("initial_angle_rad", "rad", false, "theta at t = 0. When omitted: the angle of the inputs at t = 0 (the loop starts locked)."),
            ]),
            example: json!({"type": "pll", "name": "pll", "inputs": ["v.a", "v.b", "v.c"], "nominal_frequency_hz": 50.0, "rated_amplitude": 5389.0, "proportional_gain": 177.7, "integral_gain_per_s": 15791.0, "initial_angle_rad": 0.0}),
        }
    }
}
