//! The blocks that are functions of time: step, ramp, sine wave and time
//! series. They write set points and schedules.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{ControlSpec, Scope};
use crate::catalog::{control, p, ComponentInfo, SIGNAL};
use crate::check::{finite, range_error};
use crate::diagnostic::Diagnostic;

/// Step: `before` while t < time_s, `after` from the first grid point at or
/// after time_s on. The circuit sees the change one time step later, as every
/// control signal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub name: String,
    /// Time of the step in seconds.
    pub time_s: f64,
    /// Value before the step. Default 0.
    #[serde(default)]
    pub before: f64,
    /// Value from the step on.
    pub after: f64,
}

impl ControlSpec for Step {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "step"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        let name = self.name.as_str();
        finite(d, p, name, "time_s", self.time_s);
        finite(d, p, name, "before", self.before);
        finite(d, p, name, "after", self.after);
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "step",
            category: "control",
            summary:
                "before while t < time_s, after from the first grid point at or after time_s on.",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("time_s", "s", true, "Time of the step."),
                p("before", "", false, "Value before the step. Default 0."),
                p("after", "", true, "Value from the step on."),
            ]),
            example: json!({"type": "step", "name": "p_ref", "time_s": 0.1, "before": 0.5, "after": 0.8}),
        }
    }
}

/// Ramp: `initial` until start_time_s, then rising by slope_per_s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Ramp {
    pub name: String,
    /// Time at which the ramp starts, in seconds. Default 0.
    #[serde(default)]
    pub start_time_s: f64,
    /// Rise per second (negative: a fall).
    pub slope_per_s: f64,
    /// Value until the ramp starts. Default 0.
    #[serde(default)]
    pub initial: f64,
}

impl ControlSpec for Ramp {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "ramp"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        let name = self.name.as_str();
        finite(d, p, name, "start_time_s", self.start_time_s);
        finite(d, p, name, "slope_per_s", self.slope_per_s);
        finite(d, p, name, "initial", self.initial);
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "ramp",
            category: "control",
            summary: "initial + slope_per_s*(t - start_time_s) from start_time_s on; initial before. Limit it with a limiter block.",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("start_time_s", "s", false, "Time at which the ramp starts. Default 0."),
                p("slope_per_s", "1/s", true, "Rise per second (negative: a fall)."),
                p("initial", "", false, "Value until the ramp starts. Default 0."),
            ]),
            example: json!({"type": "ramp", "name": "angle", "start_time_s": 0.0, "slope_per_s": 314.1592653589793, "initial": 0.0}),
        }
    }
}

/// Sine wave: offset + amplitude * sin(2 pi frequency_hz t + phase_deg), as
/// the sine waveform of a source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SineWave {
    pub name: String,
    /// Peak value.
    pub amplitude: f64,
    /// Must be >= 0.
    pub frequency_hz: f64,
    /// Default 0. Use 90 for a cosine.
    #[serde(default)]
    pub phase_deg: f64,
    /// Default 0.
    #[serde(default)]
    pub offset: f64,
}

impl ControlSpec for SineWave {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "sine_wave"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn check(&self, p: &str, _scope: &Scope, d: &mut Vec<Diagnostic>) {
        let name = self.name.as_str();
        finite(d, p, name, "amplitude", self.amplitude);
        if !self.frequency_hz.is_finite() || self.frequency_hz < 0.0 {
            range_error(
                d,
                p,
                name,
                "frequency_hz",
                self.frequency_hz,
                "Frequency must be >= 0 Hz.",
            );
        }
        finite(d, p, name, "phase_deg", self.phase_deg);
        finite(d, p, name, "offset", self.offset);
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "sine_wave",
            category: "control",
            summary: "offset + amplitude*sin(2*pi*frequency_hz*t + phase_deg); amplitude is the peak value.",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("amplitude", "", true, "Peak value."),
                p("frequency_hz", "Hz", true, "Frequency, >= 0."),
                p("phase_deg", "deg", false, "Phase. Default 0; 90 for a cosine."),
                p("offset", "", false, "Default 0."),
            ]),
            example: json!({"type": "sine_wave", "name": "v_ref_a", "amplitude": 100.0, "frequency_hz": 50.0, "phase_deg": 90.0, "offset": 0.0}),
        }
    }
}

/// How a time series is read between its times.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SeriesInterpolation {
    /// Each value holds until the next time.
    #[default]
    Step,
    /// Straight lines between the values.
    Linear,
}

/// Time series: values at given times, held until the next time (`step`) or
/// joined by straight lines (`linear`). Before the first time the first
/// value holds, after the last time the last one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimeSeries {
    pub name: String,
    /// Times in seconds, strictly ascending, at least one.
    pub times_s: Vec<f64>,
    /// One value per time.
    pub values: Vec<f64>,
    /// "step" (default) or "linear".
    #[serde(default)]
    pub interpolation: SeriesInterpolation,
}

impl ControlSpec for TimeSeries {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "time_series"
    }
    fn since(&self) -> &'static str {
        "0.5"
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
        if self.times_s.is_empty() || self.times_s.len() != self.values.len() {
            error(
                d,
                "values".to_string(),
                format!(
                    "\"{name}\" has {} times and {} values",
                    self.times_s.len(),
                    self.values.len()
                ),
                "Give at least one time and exactly one value per time.",
            );
        }
        for (field, v) in [("times_s", &self.times_s), ("values", &self.values)] {
            if let Some(k) = v.iter().position(|x| !x.is_finite()) {
                error(
                    d,
                    format!("{field}[{k}]"),
                    format!("{field}[{k}] of \"{name}\" is {}", v[k]),
                    "Use finite numbers.",
                );
            }
        }
        if let Some(k) = (1..self.times_s.len()).find(|&k| self.times_s[k] <= self.times_s[k - 1]) {
            error(
                d,
                format!("times_s[{k}]"),
                format!(
                    "times_s[{k}] ({}) of \"{name}\" is not after times_s[{}] ({})",
                    self.times_s[k],
                    k - 1,
                    self.times_s[k - 1]
                ),
                "List the times in strictly ascending order. For a jump with \"linear\", use two \
                 times one time step apart.",
            );
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "time_series",
            category: "control",
            summary: "Values at given times: each held until the next time (step) or joined by straight lines (linear). The first value holds before the first time, the last one after the last time.",
            sign_convention: SIGNAL,
            parameters: control(vec![
                p("times_s", "s", true, "Times, strictly ascending, at least one."),
                p("values", "", true, "One value per time."),
                p("interpolation", "", false, "\"step\" (default) or \"linear\"."),
            ]),
            example: json!({"type": "time_series", "name": "p_schedule", "times_s": [0.0, 0.1, 0.2], "values": [0.5, 0.8, 0.6], "interpolation": "linear"}),
        }
    }
}
