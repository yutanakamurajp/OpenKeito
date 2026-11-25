//! The blocks without a state of their own that are not linear: limiter,
//! rate limiter, dead band, product, quotient, function, selector and the
//! block that keeps the value its input had at t = 0.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{check_limit_values, input_list, one_input, ControlSpec, LimitValue, Scope};
use crate::catalog::{control, p, ComponentInfo, SIGNAL};
use crate::check::{finite, positive, range_error};
use crate::diagnostic::Diagnostic;

const INPUT: &str = "Name of the input signal.";

/// E172: a block with the wrong number of inputs.
fn input_count(d: &mut Vec<Diagnostic>, p: &str, name: &str, count: usize, hint: &str) {
    d.push(Diagnostic::error(
        "E172",
        format!("{p}.inputs"),
        format!("\"{name}\" has {count} inputs"),
        hint.to_string(),
    ));
}

/// Limiter: the input, held between `minimum` and `maximum`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Limiter {
    pub name: String,
    pub input: String,
    /// Lower limit: a number, or (from version 0.7) the name of a control
    /// signal. No limit when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<LimitValue>,
    /// Upper limit: a number, or (from version 0.7) the name of a control
    /// signal. No limit when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<LimitValue>,
}

impl ControlSpec for Limiter {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "limiter"
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
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        check_limit_values(
            d,
            p,
            &self.name,
            ("minimum", self.minimum.as_ref()),
            ("maximum", self.maximum.as_ref()),
        );
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "limiter",
            category: "control",
            summary: "The input, held between minimum and maximum.",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("input", "", true, INPUT),
                p(
                    "minimum",
                    "",
                    false,
                    "Lower limit: a number, or the name of a control signal (version 0.7). None when omitted.",
                ),
                p(
                    "maximum",
                    "",
                    false,
                    "Upper limit, > minimum when both are numbers: a number, or the name of a control signal (version 0.7). None when omitted.",
                ),
            ]),
            example: json!({"type": "limiter", "name": "i_ref_limited", "input": "i_ref", "minimum": -1.1, "maximum": 1.1}),
        }
    }
}

/// Rate limiter: the output follows the input, rising by at most
/// rising_rate_per_s and falling by at most falling_rate_per_s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RateLimiter {
    pub name: String,
    pub input: String,
    /// Largest rise of the output per second. Must be > 0.
    pub rising_rate_per_s: f64,
    /// Largest fall of the output per second, as a positive number. Must be
    /// > 0.
    pub falling_rate_per_s: f64,
    /// Output at t = 0. When omitted: the input at t = 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_output: Option<f64>,
}

impl ControlSpec for RateLimiter {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "rate_limiter"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        one_input(&self.input)
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        let name = self.name.as_str();
        for (field, v) in [
            ("rising_rate_per_s", self.rising_rate_per_s),
            ("falling_rate_per_s", self.falling_rate_per_s),
        ] {
            if !positive(v) {
                range_error(
                    d,
                    p,
                    name,
                    field,
                    v,
                    "A rate is a positive number per second (also the falling rate).",
                );
            }
        }
        if let Some(v) = self.initial_output {
            finite(d, p, name, "initial_output", v);
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "rate_limiter",
            category: "control",
            summary: "The output follows the input, rising by at most rising_rate_per_s and falling by at most falling_rate_per_s.",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("input", "", true, INPUT),
                p("rising_rate_per_s", "1/s", true, "Largest rise per second, > 0."),
                p("falling_rate_per_s", "1/s", true, "Largest fall per second as a positive number, > 0."),
                p("initial_output", "", false, "Output at t = 0. When omitted: the input at t = 0."),
            ]),
            example: json!({"type": "rate_limiter", "name": "p_ref_ramped", "input": "p_ref", "rising_rate_per_s": 0.1, "falling_rate_per_s": 0.5, "initial_output": 0.0}),
        }
    }
}

/// Dead band: 0 while the input is between `lower` and `upper`; input - upper
/// above and input - lower below, so the output is continuous.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeadBand {
    pub name: String,
    pub input: String,
    /// Lower edge of the band.
    pub lower: f64,
    /// Upper edge of the band. Must be >= lower.
    pub upper: f64,
}

impl ControlSpec for DeadBand {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "dead_band"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        one_input(&self.input)
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        let name = self.name.as_str();
        finite(d, p, name, "lower", self.lower);
        finite(d, p, name, "upper", self.upper);
        if self.lower.is_finite() && self.upper.is_finite() && self.upper < self.lower {
            d.push(Diagnostic::error(
                "E132",
                format!("{p}.upper"),
                format!(
                    "upper ({}) of \"{name}\" is less than lower ({})",
                    self.upper, self.lower
                ),
                "Use upper >= lower, for example -0.01 and 0.01.",
            ));
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "dead_band",
            category: "control",
            summary: "0 while lower <= input <= upper; input - upper above the band and input - lower below it (continuous).",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("input", "", true, INPUT),
                p("lower", "", true, "Lower edge of the band."),
                p("upper", "", true, "Upper edge of the band, >= lower."),
            ]),
            example: json!({"type": "dead_band", "name": "df_effective", "input": "df", "lower": -0.01, "upper": 0.01}),
        }
    }
}

/// Product of the inputs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Product {
    pub name: String,
    /// At least one signal.
    pub inputs: Vec<String>,
}

impl ControlSpec for Product {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "product"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        input_list(&self.inputs)
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        if self.inputs.is_empty() {
            input_count(d, p, &self.name, 0, "Give at least one input signal.");
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "product",
            category: "control",
            summary: "Product of the inputs.",
            sign_convention: SIGNAL,
            parameters: control(vec![p(
                "inputs",
                "",
                true,
                "Names of the input signals, at least one.",
            )]),
            example: json!({"type": "product", "name": "power", "inputs": ["v", "i"]}),
        }
    }
}

/// Quotient inputs[0] / inputs[1].
///
/// A denominator of 0 gives a value that is not finite, which stops the run
/// (E251), unless `minimum_denominator` is given: a denominator of smaller
/// magnitude is then replaced by `minimum_denominator` with its sign (the
/// positive value for 0).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Quotient {
    pub name: String,
    /// [numerator, denominator].
    pub inputs: [String; 2],
    /// Smallest magnitude of the denominator. Must be > 0. No guard when
    /// omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_denominator: Option<f64>,
}

impl ControlSpec for Quotient {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "quotient"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        input_list(&self.inputs)
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        if let Some(v) = self.minimum_denominator {
            if !positive(v) {
                range_error(
                    d,
                    p,
                    &self.name,
                    "minimum_denominator",
                    v,
                    "Use a value > 0, or omit the field.",
                );
            }
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "quotient",
            category: "control",
            summary: "inputs[0] / inputs[1]. A denominator of 0 stops the run (E251) unless minimum_denominator is given: a denominator of smaller magnitude is replaced by it, with the sign of the denominator (positive for 0).",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("inputs", "", true, "Two signal names [numerator, denominator]."),
                p("minimum_denominator", "", false, "Smallest magnitude of the denominator, > 0. No guard when omitted."),
            ]),
            example: json!({"type": "quotient", "name": "i_ref", "inputs": ["p_ref", "v_d"], "minimum_denominator": 0.01}),
        }
    }
}

/// The function of a `function` block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FunctionKind {
    /// sin(x), x in radians.
    Sin,
    /// cos(x), x in radians.
    Cos,
    /// tan(x), x in radians.
    Tan,
    /// atan(x), in radians between -pi/2 and pi/2.
    Atan,
    /// atan2(inputs[0], inputs[1]): the angle of the point
    /// (x, y) = (inputs[1], inputs[0]), in radians between -pi and pi.
    Atan2,
    /// |x|.
    Abs,
    /// Square root. A negative input is not a number and stops the run.
    Sqrt,
    /// e^x.
    Exp,
    /// Natural logarithm. An input <= 0 stops the run.
    Ln,
    /// -1, 0 or 1.
    Sign,
    /// Smallest of the inputs.
    Min,
    /// Largest of the inputs.
    Max,
    /// sqrt(inputs[0]^2 + inputs[1]^2).
    Hypot,
}

impl FunctionKind {
    /// Smallest and largest number of inputs.
    pub fn input_counts(self) -> (usize, usize) {
        match self {
            FunctionKind::Atan2 | FunctionKind::Hypot => (2, 2),
            FunctionKind::Min | FunctionKind::Max => (1, usize::MAX),
            _ => (1, 1),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            FunctionKind::Sin => "sin",
            FunctionKind::Cos => "cos",
            FunctionKind::Tan => "tan",
            FunctionKind::Atan => "atan",
            FunctionKind::Atan2 => "atan2",
            FunctionKind::Abs => "abs",
            FunctionKind::Sqrt => "sqrt",
            FunctionKind::Exp => "exp",
            FunctionKind::Ln => "ln",
            FunctionKind::Sign => "sign",
            FunctionKind::Min => "min",
            FunctionKind::Max => "max",
            FunctionKind::Hypot => "hypot",
        }
    }
}

/// A function of the inputs: sin, cos, tan, atan, abs, sqrt, exp, ln and sign
/// of one input; atan2 and hypot of two; min and max of one or more. Angles
/// are in radians. A result that is not a finite number (the square root of
/// a negative number, the logarithm of 0) stops the run (E251).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Function {
    pub name: String,
    pub function: FunctionKind,
    pub inputs: Vec<String>,
}

impl ControlSpec for Function {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "function"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        input_list(&self.inputs)
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        let (low, high) = self.function.input_counts();
        let n = self.inputs.len();
        if n < low || n > high {
            let wanted = match (low, high) {
                (1, 1) => "one input",
                (2, 2) => "two inputs",
                _ => "at least one input",
            };
            input_count(
                d,
                p,
                &self.name,
                n,
                &format!("{} takes {wanted}.", self.function.as_str()),
            );
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "function",
            category: "control",
            summary: "A function of the inputs. One input: sin, cos, tan, atan, abs, sqrt, exp, ln, sign. Two: atan2 (atan2(inputs[0], inputs[1]), the angle of the point x = inputs[1], y = inputs[0]), hypot. One or more: min, max. Angles in radians. A result that is not finite stops the run (E251).",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("function", "", true, "sin | cos | tan | atan | atan2 | abs | sqrt | exp | ln | sign | min | max | hypot."),
                p("inputs", "", true, "Names of the input signals: one, two (atan2, hypot) or at least one (min, max)."),
            ]),
            example: json!({"type": "function", "name": "v_magnitude", "function": "hypot", "inputs": ["v.d", "v.q"]}),
        }
    }
}

/// Selector: inputs[0] while `condition` >= 0.5, otherwise inputs[1].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    pub name: String,
    /// Signal that selects: >= 0.5 gives inputs[0] (as the gate of a
    /// controlled switch).
    pub condition: String,
    /// [value while the condition holds, value otherwise].
    pub inputs: [String; 2],
}

impl ControlSpec for Selector {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "selector"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        let mut v = vec![("condition".to_string(), self.condition.as_str())];
        v.extend(input_list(&self.inputs));
        v
    }
    fn check(&self, _p: &str, _scope: &Scope, _d: &mut Vec<Diagnostic>) {}
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "selector",
            category: "control",
            summary: "inputs[0] while condition >= 0.5, otherwise inputs[1]. A comparator gives such a condition.",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("condition", "", true, "Name of the signal that selects: >= 0.5 gives inputs[0]."),
                p("inputs", "", true, "Two signal names [value while the condition holds, value otherwise]."),
            ]),
            example: json!({"type": "selector", "name": "i_ref", "condition": "fault_mode", "inputs": ["i_fault", "i_normal"]}),
        }
    }
}

/// The value its input had at t = 0, kept for the whole run. It writes a set
/// point as "the value that makes the present operating point a steady
/// state": a reference that equals the measured value at t = 0 leaves a
/// controller without an error at the start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InitialValue {
    pub name: String,
    pub input: String,
}

impl ControlSpec for InitialValue {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "initial_value"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        one_input(&self.input)
    }
    fn check(&self, _p: &str, _scope: &Scope, _d: &mut Vec<Diagnostic>) {}
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "initial_value",
            category: "control",
            summary: "The value the input had at t = 0, kept for the whole run: a set point that makes the operating point at t = 0 a steady state.",
            sign_convention: SIGNAL,
            parameters: control(vec![p("input", "", true, INPUT)]),
            example: json!({"type": "initial_value", "name": "v_ref", "input": "v_meas"}),
        }
    }
}

/// Piecewise-linear 1D lookup table y(x): linearly interpolates between
/// adjacent breakpoints (x[k], y[k]) and holds y[0] for input <= x[0]
/// and y[last] for input >= x[last].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PiecewiseLinear {
    pub name: String,
    pub input: String,
    /// Input breakpoints in strictly ascending order; at least one point.
    pub x: Vec<f64>,
    /// Output values corresponding to `x` (same length as `x`).
    pub y: Vec<f64>,
}

impl ControlSpec for PiecewiseLinear {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "piecewise_linear"
    }
    fn since(&self) -> &'static str {
        "0.7"
    }
    fn inputs(&self) -> Vec<(String, &str)> {
        one_input(&self.input)
    }
    fn is_direct(&self, _field: &str, _time_step_s: f64) -> bool {
        true
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        let name = self.name.as_str();
        let error = |d: &mut Vec<Diagnostic>, field: String, message: String, hint: &str| {
            d.push(Diagnostic::error(
                "E242",
                format!("{p}.{field}"),
                message,
                hint.to_string(),
            ));
        };
        if self.x.is_empty() || self.x.len() != self.y.len() {
            error(
                d,
                "y".to_string(),
                format!(
                    "\"{name}\" has {} x breakpoints and {} y values",
                    self.x.len(),
                    self.y.len()
                ),
                "Give at least one breakpoint and one y value per x.",
            );
        }
        for (field, v) in [("x", &self.x), ("y", &self.y)] {
            if let Some(k) = v.iter().position(|val| !val.is_finite()) {
                error(
                    d,
                    format!("{field}[{k}]"),
                    format!("{field}[{k}] of \"{name}\" is {}", v[k]),
                    "Use finite numbers.",
                );
            }
        }
        if let Some(k) = (1..self.x.len()).find(|&k| self.x[k] <= self.x[k - 1]) {
            error(
                d,
                format!("x[{k}]"),
                format!(
                    "x[{k}] ({}) of \"{name}\" is not greater than x[{}] ({})",
                    self.x[k],
                    k - 1,
                    self.x[k - 1]
                ),
                "List the breakpoints x in strictly ascending order.",
            );
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "piecewise_linear",
            category: "control",
            summary: "Piecewise-linear 1D lookup table y(x): linearly interpolates between adjacent breakpoints (x[k], y[k]) and holds y[0] below x[0] and y[last] above x[last].",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("input", "", true, INPUT),
                p("x", "", true, "Input breakpoints, strictly ascending, at least one."),
                p("y", "", true, "Output values, same length as x."),
            ]),
            example: json!({"type": "piecewise_linear", "name": "gain_sched", "input": "v_meas", "x": [0.0, 0.5, 1.0], "y": [0.0, 0.5, 1.0]}),
        }
    }
}
