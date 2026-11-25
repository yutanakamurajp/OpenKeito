//! The blocks that compute a signal from other signals or from time:
//! constant, gain, sum, triangular carrier, comparator and PI controller.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{check_limit_values, input_list, ControlSpec, LimitValue, Scope, StartValue};
use crate::catalog::{control, p, ComponentInfo, SIGNAL};
use crate::check::{finite, positive, range_error};
use crate::diagnostic::Diagnostic;

/// Constant signal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Constant {
    pub name: String,
    pub value: f64,
}

impl ControlSpec for Constant {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "constant"
    }
    fn since(&self) -> &'static str {
        "0.2"
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        finite(d, p, &self.name, "value", self.value);
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "constant",
            category: "control",
            summary: "Constant signal.",
            sign_convention: SIGNAL,
            parameters: control(vec![p("value", "", true, "Value of the signal.")]),
            example: json!({"type": "constant", "name": "duty", "value": 0.4}),
        }
    }
}

/// gain * input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Gain {
    pub name: String,
    pub input: String,
    pub gain: f64,
}

impl ControlSpec for Gain {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "gain"
    }
    fn since(&self) -> &'static str {
        "0.2"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        vec![("input".to_string(), self.input.as_str())]
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        finite(d, p, &self.name, "gain", self.gain);
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "gain",
            category: "control",
            summary: "gain * input.",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("input", "", true, "Name of the input signal."),
                p("gain", "", true, "Factor."),
            ]),
            example: json!({"type": "gain", "name": "v_pu", "input": "v_out", "gain": 0.01}),
        }
    }
}

/// Sum of signs[i] * inputs[i]. Every sign is 1 or -1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sum {
    pub name: String,
    pub inputs: Vec<String>,
    pub signs: Vec<f64>,
}

impl ControlSpec for Sum {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "sum"
    }
    fn since(&self) -> &'static str {
        "0.2"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        input_list(&self.inputs)
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        let (s, name) = (self, self.name.as_str());
        if s.inputs.is_empty() || s.inputs.len() != s.signs.len() {
            d.push(Diagnostic::error(
                "E172",
                format!("{p}.signs"),
                format!(
                    "\"{name}\" has {} inputs and {} signs",
                    s.inputs.len(),
                    s.signs.len()
                ),
                "Give at least one input and exactly one sign (1 or -1) per input.",
            ));
        }
        for (k, sign) in s.signs.iter().enumerate() {
            if *sign != 1.0 && *sign != -1.0 {
                d.push(Diagnostic::error(
                    "E172",
                    format!("{p}.signs[{k}]"),
                    format!("sign {sign} of \"{name}\" is neither 1 nor -1"),
                    "Write 1 to add the input and -1 to subtract it. Use a gain block to scale it.",
                ));
            }
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "sum",
            category: "control",
            summary: "Sum of signs[i] * inputs[i].",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p(
                    "inputs",
                    "",
                    true,
                    "Names of the input signals, at least one.",
                ),
                p("signs", "", true, "1 or -1 for each input."),
            ]),
            example: json!({"type": "sum", "name": "error", "inputs": ["v_ref", "v_out"], "signs": [1.0, -1.0]}),
        }
    }
}

/// Symmetric triangular carrier. With phase_deg = 0 it is `minimum` at
/// t = 0, rises to `maximum` at half the period and falls back to `minimum`
/// at the end of the period. A positive phase_deg advances it by
/// phase_deg / 360 of a period.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TriangleWave {
    pub name: String,
    /// Must be > 0.
    pub frequency_hz: f64,
    /// Default 0.
    #[serde(default)]
    pub minimum: f64,
    /// Must be greater than minimum. Default 1.
    #[serde(default = "default_one")]
    pub maximum: f64,
    /// Default 0.
    #[serde(default)]
    pub phase_deg: f64,
}

fn default_one() -> f64 {
    1.0
}

impl ControlSpec for TriangleWave {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "triangle_wave"
    }
    fn since(&self) -> &'static str {
        "0.2"
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        let (w, name) = (self, self.name.as_str());
        let finite = |d: &mut Vec<Diagnostic>, field: &str, v: f64| finite(d, p, name, field, v);
        if !positive(w.frequency_hz) {
            range_error(
                d,
                p,
                name,
                "frequency_hz",
                w.frequency_hz,
                "Frequency must be > 0 Hz.",
            );
        }
        finite(d, "minimum", w.minimum);
        finite(d, "maximum", w.maximum);
        finite(d, "phase_deg", w.phase_deg);
        if w.minimum.is_finite() && w.maximum.is_finite() && w.maximum <= w.minimum {
            d.push(Diagnostic::error(
                "E132",
                format!("{p}.maximum"),
                format!(
                    "maximum ({}) of \"{name}\" is not greater than minimum ({})",
                    w.maximum, w.minimum
                ),
                "Use maximum > minimum, for example 0 and 1 for a duty-cycle reference.",
            ));
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "triangle_wave",
            category: "control",
            summary: "Symmetric triangular carrier: minimum at t = 0 (phase_deg = 0), maximum at half the period. A positive phase_deg advances it by phase_deg/360 of a period.",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("frequency_hz", "Hz", true, "Carrier frequency, > 0."),
                p("minimum", "", false, "Lowest value. Default 0."),
                p("maximum", "", false, "Highest value, > minimum. Default 1."),
                p("phase_deg", "deg", false, "Phase advance. Default 0."),
            ]),
            example: json!({"type": "triangle_wave", "name": "carrier", "frequency_hz": 10000.0, "minimum": 0.0, "maximum": 1.0, "phase_deg": 0.0}),
        }
    }
}

/// 1 when inputs[0] > inputs[1], otherwise 0. For PWM, compare a reference
/// (inputs[0]) with a carrier (inputs[1]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Comparator {
    pub name: String,
    pub inputs: [String; 2],
}

impl ControlSpec for Comparator {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "comparator"
    }
    fn since(&self) -> &'static str {
        "0.2"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        input_list(&self.inputs)
    }
    fn check(&self, _p: &str, _scope: &Scope, _d: &mut Vec<Diagnostic>) {}
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "comparator",
            category: "control",
            summary:
                "1 when inputs[0] > inputs[1], otherwise 0. For PWM: inputs = [reference, carrier].",
            sign_convention: SIGNAL,
            parameters: control(vec![p("inputs", "", true, "Two signal names [a, b].")]),
            example: json!({"type": "comparator", "name": "pwm", "inputs": ["duty", "carrier"]}),
        }
    }
}

/// Anti-windup rule when output limits are set on a PI block.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PiAntiWindup {
    /// Conditional integration: holds the integral whenever Kp * input + integral + delta moves further past an active limit (default).
    #[default]
    Conditional,
    /// IEEE 421.5 non-windup integrator state limit: limits the integral state itself within [output_min, output_max], then clamps Kp * input + integral within [output_min, output_max] (version 0.7).
    State,
}

impl PiAntiWindup {
    pub fn is_conditional(&self) -> bool {
        *self == PiAntiWindup::Conditional
    }
}

/// Proportional-integral controller: output = proportional_gain * input +
/// integral, where d(integral)/dt = integral_gain_per_s * input is
/// integrated with the trapezoidal rule from one grid point to the next.
/// With output limits the output is clamped, and anti-windup follows
/// `anti_windup` ("conditional" by default, or "state" for IEEE 421.5
/// non-windup integrator state limiting).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Pi {
    pub name: String,
    pub input: String,
    pub proportional_gain: f64,
    /// Integral gain in 1/s.
    pub integral_gain_per_s: f64,
    /// Value of the integral at t = 0: a number, or (from version 0.5) the
    /// name of a signal whose value at t = 0 is taken. Default 0.
    #[serde(default)]
    pub initial_integral: StartValue,
    /// Lower limit of the output: a number, or (from version 0.7) the name of
    /// a control signal. No limit when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_min: Option<LimitValue>,
    /// Upper limit of the output: a number, or (from version 0.7) the name of
    /// a control signal. No limit when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_max: Option<LimitValue>,
    /// Anti-windup rule when output limits are set: "conditional" (default)
    /// or "state" (version 0.7).
    #[serde(default, skip_serializing_if = "PiAntiWindup::is_conditional")]
    pub anti_windup: PiAntiWindup,
}

impl ControlSpec for Pi {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "pi"
    }
    fn since(&self) -> &'static str {
        if self.anti_windup == PiAntiWindup::State
            || self
                .output_min
                .as_ref()
                .and_then(LimitValue::signal)
                .is_some()
            || self
                .output_max
                .as_ref()
                .and_then(LimitValue::signal)
                .is_some()
        {
            "0.7"
        } else if self.initial_integral.signal().is_some() {
            "0.5"
        } else {
            "0.2"
        }
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        let mut v = vec![("input".to_string(), self.input.as_str())];
        if let Some(s) = self.initial_integral.signal() {
            v.push(("initial_integral".to_string(), s));
        }
        if let Some(s) = self.output_min.as_ref().and_then(LimitValue::signal) {
            v.push(("output_min".to_string(), s));
        }
        if let Some(s) = self.output_max.as_ref().and_then(LimitValue::signal) {
            v.push(("output_max".to_string(), s));
        }
        v
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        let (c, name) = (self, self.name.as_str());
        let finite = |d: &mut Vec<Diagnostic>, field: &str, v: f64| finite(d, p, name, field, v);
        finite(d, "proportional_gain", c.proportional_gain);
        finite(d, "integral_gain_per_s", c.integral_gain_per_s);
        c.initial_integral.check(d, p, name, "initial_integral");
        check_limit_values(
            d,
            p,
            name,
            ("output_min", c.output_min.as_ref()),
            ("output_max", c.output_max.as_ref()),
        );
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "pi",
            category: "control",
            summary: "Proportional-integral controller: output = proportional_gain*input + integral, d(integral)/dt = integral_gain_per_s*input (trapezoidal rule between grid points). With limits the output is clamped and the integral is held while the input drives the output further beyond a limit.",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("input", "", true, "Name of the input signal (usually an error)."),
                p("proportional_gain", "", true, "Proportional gain."),
                p("integral_gain_per_s", "1/s", true, "Integral gain."),
                p("initial_integral", "", false, "Integral at t = 0: a number, or the name of a signal whose value at t = 0 is taken (version 0.5). Default 0."),
                p("output_min", "", false, "Lower output limit: a number, or the name of a control signal (version 0.7). None when omitted."),
                p("output_max", "", false, "Upper output limit, > output_min when both are numbers: a number, or the name of a control signal (version 0.7). None when omitted."),
                p("anti_windup", "", false, "Anti-windup rule when output limits are set: \"conditional\" (default: hold integral while output is saturated) or \"state\" (version 0.7: IEEE 421.5 non-windup integrator state limit)."),
            ]),
            example: json!({"type": "pi", "name": "duty", "input": "error", "proportional_gain": 0.005, "integral_gain_per_s": 20.0, "initial_integral": 0.0, "output_min": 0.0, "output_max": 0.95}),
        }
    }
}
