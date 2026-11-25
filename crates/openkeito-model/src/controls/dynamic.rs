//! The blocks with a state: transfer function, integrator, first-order lag,
//! lead-lag and delay. Their states are integrated with the trapezoidal rule
//! from one grid point to the next.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{check_limit_values, one_input, ControlSpec, LimitValue, Scope, StartValue};
use crate::catalog::{control, p, ComponentInfo, SIGNAL};
use crate::check::{finite, positive, range_error};
use crate::diagnostic::Diagnostic;

const INPUT: &str = "Name of the input signal.";

const INITIAL_INPUT: &str = "The block starts in the steady state of this constant input (0: at rest). When omitted: in the steady state of the input at t = 0.";

/// More samples than this in the history of a delay cannot be meant.
const MAX_DELAY_STEPS: f64 = 1e8;

fn default_one() -> f64 {
    1.0
}

fn check_initial_input(d: &mut Vec<Diagnostic>, p: &str, name: &str, v: Option<f64>) {
    if let Some(v) = v {
        finite(d, p, name, "initial_input", v);
    }
}

/// Transfer function N(s)/D(s) with the coefficients in descending powers of
/// s: numerator [1, 2] and denominator [1, 3, 2] is (s + 2)/(s^2 + 3 s + 2).
/// The order of the numerator must not exceed that of the denominator.
///
/// The block is written in controllable canonical form and integrated with
/// the trapezoidal rule. Split a transfer function of high order into blocks
/// of first and second order: the canonical form is badly conditioned when
/// the coefficients differ by many orders of magnitude.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferFunction {
    pub name: String,
    pub input: String,
    /// Coefficients of N(s), highest power first. At least one, and not more
    /// than the denominator has.
    pub numerator: Vec<f64>,
    /// Coefficients of D(s), highest power first. The first must not be 0.
    pub denominator: Vec<f64>,
    /// The block starts in the steady state of this constant input. When
    /// omitted: of the input at t = 0. A transfer function with a pole at
    /// s = 0 (last denominator coefficient 0) has no such state: it starts
    /// with every state at 0, and this field must be omitted or 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_input: Option<f64>,
}

impl TransferFunction {
    /// True when the output depends on the input at the same instant: the
    /// numerator has the order of the denominator.
    pub fn has_direct_term(&self) -> bool {
        self.numerator.len() == self.denominator.len()
            && self.numerator.first().is_some_and(|b| *b != 0.0)
    }
}

impl ControlSpec for TransferFunction {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "transfer_function"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        one_input(&self.input)
    }
    fn is_direct(&self, _field: &str, _time_step_s: f64) -> bool {
        self.has_direct_term()
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        let name = self.name.as_str();
        let error = |d: &mut Vec<Diagnostic>, field: &str, message: String, hint: &str| {
            d.push(Diagnostic::error(
                "E241",
                format!("{p}.{field}"),
                message,
                hint.to_string(),
            ));
        };
        for (field, v) in [
            ("numerator", &self.numerator),
            ("denominator", &self.denominator),
        ] {
            if v.is_empty() {
                error(
                    d,
                    field,
                    format!("{field} of \"{name}\" has no coefficients"),
                    "Give the coefficients in descending powers of s, for example [1.0] for 1 and \
                     [0.1, 1.0] for 0.1 s + 1.",
                );
            } else if let Some(k) = v.iter().position(|c| !c.is_finite()) {
                error(
                    d,
                    &format!("{field}[{k}]"),
                    format!("coefficient {} of \"{name}\" is not a finite number", v[k]),
                    "Use finite numbers.",
                );
            }
        }
        if self.denominator.first() == Some(&0.0) {
            error(
                d,
                "denominator[0]",
                format!("the first denominator coefficient of \"{name}\" is 0"),
                "The first coefficient belongs to the highest power of s and must not be 0: \
                 remove leading zeros.",
            );
        }
        if self.numerator.len() > self.denominator.len() && !self.denominator.is_empty() {
            error(
                d,
                "numerator",
                format!(
                    "the numerator of \"{name}\" has {} coefficients and the denominator {}: the \
                     transfer function is not proper",
                    self.numerator.len(),
                    self.denominator.len()
                ),
                "The order of the numerator must not exceed that of the denominator (a pure \
                 derivative amplifies without bound). Add a pole, for example s/(1 + 0.001 s) in \
                 place of s.",
            );
        }
        check_initial_input(d, p, name, self.initial_input);
        if self.denominator.len() > 1
            && self.denominator.last() == Some(&0.0)
            && self.initial_input.is_some_and(|u| u != 0.0)
        {
            error(
                d,
                "initial_input",
                format!(
                    "\"{name}\" has a pole at s = 0 (last denominator coefficient 0) and so no \
                     steady state for a constant input of {}",
                    self.initial_input.unwrap_or(0.0)
                ),
                "Omit initial_input: the block then starts with every state at 0. For an initial \
                 value, split off the pole as an integrator block with initial_value.",
            );
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "transfer_function",
            category: "control",
            summary: "Transfer function N(s)/D(s), coefficients in descending powers of s (numerator [1, 2], denominator [1, 3, 2]: (s + 2)/(s^2 + 3 s + 2)). Controllable canonical form, integrated with the trapezoidal rule. A strictly proper transfer function (numerator of lower order) may close a loop of blocks.",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("input", "", true, INPUT),
                p("numerator", "", true, "Coefficients of N(s), highest power of s first; not more than the denominator has."),
                p("denominator", "", true, "Coefficients of D(s), highest power of s first; the first is not 0."),
                p("initial_input", "", false, "The block starts in the steady state of this constant input (0: at rest). When omitted: of the input at t = 0. With a pole at s = 0 the states start at 0 and this must be omitted or 0."),
            ]),
            example: json!({"type": "transfer_function", "name": "filtered", "input": "v_meas", "numerator": [1.0], "denominator": [1e-6, 1.4e-3, 1.0], "initial_input": 0.0}),
        }
    }
}

/// Integrator: d(output)/dt = gain_per_s * input, integrated with the
/// trapezoidal rule. With limits the state itself stays within them (it does
/// not wind up: it leaves a limit as soon as the input changes sign), or,
/// with `wrap`, it is folded back into [minimum, maximum) (an angle).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Integrator {
    pub name: String,
    pub input: String,
    /// Gain in 1/s. Default 1.
    #[serde(default = "default_one")]
    pub gain_per_s: f64,
    /// Output at t = 0: a number, or the name of a signal whose value at
    /// t = 0 is taken. Default 0.
    #[serde(default)]
    pub initial_value: StartValue,
    /// Lower limit of the output: a number, or (from version 0.7) the name of
    /// a control signal. No limit when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<LimitValue>,
    /// Upper limit of the output: a number, or (from version 0.7) the name of
    /// a control signal. No limit when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<LimitValue>,
    /// True: the output is folded back into [minimum, maximum) instead of
    /// being held at a limit, as for an angle between 0 and 2 pi. Needs both
    /// limits as constant numbers. Default false.
    #[serde(default)]
    pub wrap: bool,
}

impl ControlSpec for Integrator {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "integrator"
    }
    fn since(&self) -> &'static str {
        if self.minimum.as_ref().and_then(LimitValue::signal).is_some()
            || self.maximum.as_ref().and_then(LimitValue::signal).is_some()
        {
            "0.7"
        } else {
            "0.5"
        }
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        let mut v = one_input(&self.input);
        if let Some(s) = self.initial_value.signal() {
            v.push(("initial_value".to_string(), s));
        }
        if let Some(s) = self.minimum.as_ref().and_then(LimitValue::signal) {
            v.push(("minimum".to_string(), s));
        }
        if let Some(s) = self.maximum.as_ref().and_then(LimitValue::signal) {
            v.push(("maximum".to_string(), s));
        }
        v
    }
    fn is_direct(&self, field: &str, _time_step_s: f64) -> bool {
        // The initial value is read at t = 0 directly.
        field != "input"
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        let name = self.name.as_str();
        finite(d, p, name, "gain_per_s", self.gain_per_s);
        self.initial_value.check(d, p, name, "initial_value");
        check_limit_values(
            d,
            p,
            name,
            ("minimum", self.minimum.as_ref()),
            ("maximum", self.maximum.as_ref()),
        );
        if self.wrap {
            if self.minimum.is_none() || self.maximum.is_none() {
                d.push(Diagnostic::error(
                    "E132",
                    format!("{p}.wrap"),
                    format!("\"{name}\" wraps its output but has not both limits"),
                    "Give minimum and maximum, for example 0 and 6.283185307179586 for an angle in \
                     radians, or remove wrap.",
                ));
            } else if self.minimum.as_ref().and_then(LimitValue::signal).is_some()
                || self.maximum.as_ref().and_then(LimitValue::signal).is_some()
            {
                d.push(Diagnostic::error(
                    "E132",
                    format!("{p}.wrap"),
                    format!("\"{name}\" wraps its output but uses a signal as a limit"),
                    "Use constant numbers for minimum and maximum when wrap is true, or remove wrap.",
                ));
            }
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "integrator",
            category: "control",
            summary: "Integrator: d(output)/dt = gain_per_s*input (trapezoidal rule). With limits the state stays within them and leaves a limit as soon as the input changes sign (no wind-up); with wrap it is folded back into [minimum, maximum), as an angle. May close a loop of blocks.",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("input", "", true, INPUT),
                p("gain_per_s", "1/s", false, "Gain. Default 1."),
                p("initial_value", "", false, "Output at t = 0: a number, or the name of a signal whose value at t = 0 is taken. Default 0."),
                p("minimum", "", false, "Lower limit of the output: a number, or the name of a control signal (version 0.7). None when omitted."),
                p("maximum", "", false, "Upper limit of the output, > minimum when both are numbers: a number, or the name of a control signal (version 0.7). None when omitted."),
                p("wrap", "", false, "true: fold the output back into [minimum, maximum) instead of holding it at a limit (needs both limits as constant numbers). Default false."),
            ]),
            example: json!({"type": "integrator", "name": "theta", "input": "omega", "gain_per_s": 1.0, "initial_value": 0.0, "minimum": 0.0, "maximum": std::f64::consts::TAU, "wrap": true}),
        }
    }
}

/// First-order lag gain/(1 + s time_constant_s), integrated with the
/// trapezoidal rule. With limits the state itself stays within them
/// (non-windup limiter): the output leaves a limit as soon as gain * input
/// falls back inside it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Lag {
    pub name: String,
    pub input: String,
    /// Gain at s = 0. Default 1.
    #[serde(default = "default_one")]
    pub gain: f64,
    /// Time constant in seconds. Must be > 0.
    pub time_constant_s: f64,
    /// Lower limit of the output: a number, or (from version 0.7) the name of
    /// a control signal. No limit when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<LimitValue>,
    /// Upper limit of the output: a number, or (from version 0.7) the name of
    /// a control signal. No limit when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<LimitValue>,
    /// The block starts in the steady state of this constant input. When
    /// omitted: of the input at t = 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_input: Option<f64>,
}

fn check_time_constant(
    d: &mut Vec<Diagnostic>,
    p: &str,
    name: &str,
    field: &str,
    v: f64,
    time_step_s: f64,
) {
    if !positive(v) {
        range_error(d, p, name, field, v, "A time constant must be > 0 s.");
    } else if positive(time_step_s) && !(time_step_s / (2.0 * v)).is_finite() {
        range_error(
            d,
            p,
            name,
            field,
            v,
            "The ratio time_step_s / (2 * time_constant) is not finite in double precision. Use \
             a larger time constant or a smaller time_step_s.",
        );
    }
}

impl ControlSpec for Lag {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "lag"
    }
    fn since(&self) -> &'static str {
        if self.minimum.as_ref().and_then(LimitValue::signal).is_some()
            || self.maximum.as_ref().and_then(LimitValue::signal).is_some()
        {
            "0.7"
        } else {
            "0.5"
        }
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        let mut v = one_input(&self.input);
        if let Some(s) = self.minimum.as_ref().and_then(LimitValue::signal) {
            v.push(("minimum".to_string(), s));
        }
        if let Some(s) = self.maximum.as_ref().and_then(LimitValue::signal) {
            v.push(("maximum".to_string(), s));
        }
        v
    }
    fn is_direct(&self, field: &str, _time_step_s: f64) -> bool {
        // The limits act instantaneously at each grid point.
        field != "input"
    }
    fn check(&self, p: &str, scope: &Scope, d: &mut Vec<Diagnostic>) {
        let name = self.name.as_str();
        finite(d, p, name, "gain", self.gain);
        check_time_constant(
            d,
            p,
            name,
            "time_constant_s",
            self.time_constant_s,
            scope.time_step_s,
        );
        check_limit_values(
            d,
            p,
            name,
            ("minimum", self.minimum.as_ref()),
            ("maximum", self.maximum.as_ref()),
        );
        check_initial_input(d, p, name, self.initial_input);
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "lag",
            category: "control",
            summary: "First-order lag gain/(1 + s*time_constant_s) (trapezoidal rule). With limits the state stays within them (non-windup): the output leaves a limit as soon as gain*input is back inside it. May close a loop of blocks.",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("input", "", true, INPUT),
                p("gain", "", false, "Gain at s = 0. Default 1."),
                p("time_constant_s", "s", true, "Time constant, > 0."),
                p("minimum", "", false, "Lower limit of the output: a number, or the name of a control signal (version 0.7). None when omitted."),
                p("maximum", "", false, "Upper limit of the output, > minimum when both are numbers: a number, or the name of a control signal (version 0.7). None when omitted."),
                p("initial_input", "", false, INITIAL_INPUT),
            ]),
            example: json!({"type": "lag", "name": "v_filtered", "input": "v_meas", "gain": 1.0, "time_constant_s": 0.02, "minimum": 0.0, "maximum": 1.2, "initial_input": 0.0}),
        }
    }
}

/// Lead-lag gain * (1 + s lead_time_constant_s)/(1 + s lag_time_constant_s),
/// integrated with the trapezoidal rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LeadLag {
    pub name: String,
    pub input: String,
    /// Gain at s = 0. Default 1.
    #[serde(default = "default_one")]
    pub gain: f64,
    /// Time constant of the numerator in seconds. Must be >= 0.
    pub lead_time_constant_s: f64,
    /// Time constant of the denominator in seconds. Must be > 0.
    pub lag_time_constant_s: f64,
    /// The block starts in the steady state of this constant input. When
    /// omitted: of the input at t = 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_input: Option<f64>,
}

impl ControlSpec for LeadLag {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "lead_lag"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        one_input(&self.input)
    }
    fn is_direct(&self, _field: &str, _time_step_s: f64) -> bool {
        self.lead_time_constant_s != 0.0
    }
    fn check(&self, p: &str, scope: &Scope, d: &mut Vec<Diagnostic>) {
        let name = self.name.as_str();
        finite(d, p, name, "gain", self.gain);
        if !self.lead_time_constant_s.is_finite() || self.lead_time_constant_s < 0.0 {
            range_error(
                d,
                p,
                name,
                "lead_time_constant_s",
                self.lead_time_constant_s,
                "The time constant of the numerator must be >= 0 s.",
            );
        }
        check_time_constant(
            d,
            p,
            name,
            "lag_time_constant_s",
            self.lag_time_constant_s,
            scope.time_step_s,
        );
        check_initial_input(d, p, name, self.initial_input);
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "lead_lag",
            category: "control",
            summary: "Lead-lag gain*(1 + s*lead_time_constant_s)/(1 + s*lag_time_constant_s) (trapezoidal rule).",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("input", "", true, INPUT),
                p("gain", "", false, "Gain at s = 0. Default 1."),
                p("lead_time_constant_s", "s", true, "Time constant of the numerator, >= 0."),
                p("lag_time_constant_s", "s", true, "Time constant of the denominator, > 0."),
                p("initial_input", "", false, INITIAL_INPUT),
            ]),
            example: json!({"type": "lead_lag", "name": "compensated", "input": "error", "gain": 1.0, "lead_time_constant_s": 0.05, "lag_time_constant_s": 0.01, "initial_input": 0.0}),
        }
    }
}

/// Delay: output(t) = input(t - delay_s). The input is kept at the grid
/// points and interpolated linearly between them. A delay shorter than the
/// time step interpolates between the present and the previous input, so it
/// depends on the present input and cannot close a loop of blocks; a delay of
/// 0 passes the input on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Delay {
    pub name: String,
    pub input: String,
    /// Delay in seconds. Must be >= 0.
    pub delay_s: f64,
    /// Output while t < delay_s (the input before t = 0). When omitted: the
    /// input at t = 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_output: Option<f64>,
}

impl ControlSpec for Delay {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "delay"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        one_input(&self.input)
    }
    fn is_direct(&self, _field: &str, time_step_s: f64) -> bool {
        // Within rounding of one step counts as one step.
        self.delay_s < time_step_s * (1.0 - 1e-9)
    }
    fn check(&self, p: &str, scope: &Scope, d: &mut Vec<Diagnostic>) {
        let name = self.name.as_str();
        if !self.delay_s.is_finite() || self.delay_s < 0.0 {
            range_error(
                d,
                p,
                name,
                "delay_s",
                self.delay_s,
                "A delay must be >= 0 s.",
            );
        } else if positive(scope.time_step_s) && self.delay_s / scope.time_step_s > MAX_DELAY_STEPS
        {
            range_error(
                d,
                p,
                name,
                "delay_s",
                self.delay_s,
                "The delay keeps one value per time step: at most 1e8 time steps. Check the units \
                 (seconds).",
            );
        }
        if let Some(v) = self.initial_output {
            finite(d, p, name, "initial_output", v);
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "delay",
            category: "control",
            summary: "Delay: output(t) = input(t - delay_s), the input interpolated linearly between grid points. A delay of at least one time step may close a loop of blocks; a shorter one depends on the present input.",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("input", "", true, INPUT),
                p("delay_s", "s", true, "Delay, >= 0."),
                p("initial_output", "", false, "Output while t < delay_s. When omitted: the input at t = 0."),
            ]),
            example: json!({"type": "delay", "name": "delayed", "input": "command", "delay_s": 0.002, "initial_output": 0.0}),
        }
    }
}
