//! Loads and injections: the constant-impedance load and the three-phase
//! current source.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::three_phase::{bus_and_neutral, check_bus_and_neutral, ground, neutral_pairs, BusRef};
use super::{Branch, ComponentInitialization, ComponentSpec, SteadyState};
use crate::catalog::{self, p, ComponentInfo, BUS};
use crate::check::{positive, range_error};
use crate::diagnostic::Diagnostic;

/// A balanced constant-impedance load (or shunt compensator) in star: at its
/// rated line-to-line voltage it takes `active_power_mw` and
/// `reactive_power_mvar` (negative for a capacitive one). Each phase is a
/// resistance in parallel with an inductance (or a capacitance) from the bus
/// to `neutral`.
///
/// The phase currents I(<name>.a) etc. flow from the bus into the load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreePhaseLoad {
    pub name: String,
    pub bus: BusRef,
    /// Star point. Default "gnd".
    #[serde(default = "ground")]
    pub neutral: String,
    /// Line-to-line voltage, rms, at which the load takes its powers. Must
    /// be > 0.
    pub rated_voltage_kv: f64,
    /// Frequency at which the reactive power holds. Must be > 0.
    pub rated_frequency_hz: f64,
    /// Three-phase active power at the rated voltage, >= 0.
    pub active_power_mw: f64,
    /// Three-phase reactive power at the rated voltage: positive inductive,
    /// negative capacitive.
    pub reactive_power_mvar: f64,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ComponentSpec for ThreePhaseLoad {
    fn current_suffixes(&self) -> Option<Vec<String>> {
        Some(super::phase_suffixes(&[""]))
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "three_phase_load"
    }
    fn since(&self) -> &'static str {
        "0.4"
    }
    fn node_names(&self) -> Vec<String> {
        bus_and_neutral(&self.bus, &self.neutral)
    }
    fn terminals(&self) -> Vec<super::Terminal> {
        super::bus_and_neutral_terminals(&self.bus, &self.neutral)
    }
    fn check_terminals(&self, p: &str, d: &mut Vec<Diagnostic>) {
        check_bus_and_neutral(&self.name, &self.bus, &self.neutral, p, d);
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        let (l, name) = (self, self.name.as_str());
        let positive_field = |d: &mut Vec<Diagnostic>, field: &str, v: f64, hint: &str| {
            if !positive(v) {
                range_error(d, p, name, field, v, hint);
            }
        };
        positive_field(
            d,
            "rated_voltage_kv",
            l.rated_voltage_kv,
            "The line-to-line voltage at which the powers hold must be > 0 kV.",
        );
        positive_field(
            d,
            "rated_frequency_hz",
            l.rated_frequency_hz,
            "The frequency at which the reactive power holds, such as 50 or 60.",
        );
        if !l.active_power_mw.is_finite() || l.active_power_mw < 0.0 {
            range_error(
                d,
                p,
                name,
                "active_power_mw",
                l.active_power_mw,
                "A load takes active power >= 0. For a generation, use three_phase_current_source \
             or three_phase_source.",
            );
        }
        if !l.reactive_power_mvar.is_finite() {
            range_error(
                d,
                p,
                name,
                "reactive_power_mvar",
                l.reactive_power_mvar,
                "Use a finite value: positive inductive, negative capacitive.",
            );
        } else if l.active_power_mw == 0.0 && l.reactive_power_mvar == 0.0 {
            range_error(
                d,
                p,
                name,
                "active_power_mw",
                0.0,
                "The load takes no power: give active_power_mw or reactive_power_mvar, or \
             remove it.",
            );
        }
    }
    fn connections(&self) -> Vec<(String, String)> {
        neutral_pairs(&self.bus, &self.neutral)
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "three_phase_load",
            category: "circuit",
            summary: "Balanced constant-impedance load or shunt compensator in star: at rated_voltage_kv it takes active_power_mw and reactive_power_mvar. Each phase is R in parallel with L (or C for negative reactive power) from the bus to the neutral.",
            sign_convention: "Phase currents I(<name>.a) etc. flow from the bus into the load; branch voltages U(<name>.a) are V(bus.p) - V(neutral). Reactive power: positive inductive, negative capacitive.",
            parameters: vec![
                catalog::name(),
                p("bus", "", true, BUS),
                p("neutral", "", false, "Node of the star point. Default \"gnd\"."),
                p("rated_voltage_kv", "kV", true, "Line-to-line voltage, rms, at which the powers hold, > 0."),
                p("rated_frequency_hz", "Hz", true, "Frequency at which the reactive power holds, > 0."),
                p("active_power_mw", "MW", true, "Three-phase active power at the rated voltage, >= 0."),
                p("reactive_power_mvar", "Mvar", true, "Three-phase reactive power at the rated voltage: positive inductive, negative capacitive."),
            ],
            example: json!({"type": "three_phase_load", "name": "LD1", "bus": "b3", "neutral": "gnd", "rated_voltage_kv": 6.6,
                "rated_frequency_hz": 50.0, "active_power_mw": 5.0, "reactive_power_mvar": 1.5}),
        }
    }
}

/// A balanced current injection: phase a is
/// i_a(t) = sqrt(2) * current_rms_a * cos(2*pi*frequency_hz*t + angle_deg),
/// phase b lags by 120 degrees and phase c leads by 120 degrees. The current
/// flows from `neutral` through the source into the bus (an injection, as a
/// current_source with nodes [neutral, bus]).
///
/// The phase currents I(<name>.a) etc. are these injections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreePhaseCurrentSource {
    pub name: String,
    pub bus: BusRef,
    /// Star point. Default "gnd".
    #[serde(default = "ground")]
    pub neutral: String,
    /// Phase current, rms, >= 0.
    pub current_rms_a: f64,
    /// Must be > 0.
    pub frequency_hz: f64,
    /// Angle of the phase-a current phasor (cosine reference). Default 0.
    #[serde(default)]
    pub angle_deg: f64,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ComponentSpec for ThreePhaseCurrentSource {
    fn current_suffixes(&self) -> Option<Vec<String>> {
        Some(super::phase_suffixes(&[""]))
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "three_phase_current_source"
    }
    fn since(&self) -> &'static str {
        "0.4"
    }
    fn node_names(&self) -> Vec<String> {
        bus_and_neutral(&self.bus, &self.neutral)
    }
    fn terminals(&self) -> Vec<super::Terminal> {
        super::bus_and_neutral_terminals(&self.bus, &self.neutral)
    }
    fn check_terminals(&self, p: &str, d: &mut Vec<Diagnostic>) {
        check_bus_and_neutral(&self.name, &self.bus, &self.neutral, p, d);
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        let (s, name) = (self, self.name.as_str());
        let positive_field = |d: &mut Vec<Diagnostic>, field: &str, v: f64, hint: &str| {
            if !positive(v) {
                range_error(d, p, name, field, v, hint);
            }
        };
        let non_negative = |d: &mut Vec<Diagnostic>, field: &str, v: f64| {
            if !v.is_finite() || v < 0.0 {
                range_error(d, p, name, field, v, "Use a finite value >= 0.");
            }
        };
        non_negative(d, "current_rms_a", s.current_rms_a);
        positive_field(
            d,
            "frequency_hz",
            s.frequency_hz,
            "Frequency must be > 0 Hz, for example 50 or 60.",
        );
        if !s.angle_deg.is_finite() {
            range_error(
                d,
                p,
                name,
                "angle_deg",
                s.angle_deg,
                "Use a finite angle in degrees.",
            );
        }
    }
    fn connections(&self) -> Vec<(String, String)> {
        neutral_pairs(&self.bus, &self.neutral)
    }
    fn steady_state(&self) -> SteadyState {
        SteadyState::Sine(self.frequency_hz)
    }
    fn sine_frequency(&self) -> Option<f64> {
        Some(self.frequency_hz)
    }
    // A current source holds no voltage.
    fn branches(&self, _t: f64) -> Vec<Branch> {
        Vec::new()
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "three_phase_current_source",
            category: "circuit",
            summary: "Balanced current injection into a bus: phase a is sqrt(2)*current_rms_a*cos(2*pi*f*t + angle_deg); b lags by 120 degrees, c leads by 120 degrees. For renewable generation, storage or a boundary injection from a power flow.",
            sign_convention: "Phase currents I(<name>.a) etc. flow from the neutral through the source into the bus (as a current_source with nodes [neutral, bus]). Phasors are rms with a cosine reference.",
            parameters: vec![
                catalog::name(),
                p("bus", "", true, BUS),
                p("neutral", "", false, "Node of the star point. Default \"gnd\"."),
                p("current_rms_a", "A", true, "Phase current, rms, >= 0."),
                p("frequency_hz", "Hz", true, "Frequency, > 0."),
                p("angle_deg", "deg", false, "Angle of the phase-a current phasor (cosine reference). Default 0."),
            ],
            example: json!({"type": "three_phase_current_source", "name": "PV1", "bus": "b3", "neutral": "gnd",
                "current_rms_a": 400.0, "frequency_hz": 50.0, "angle_deg": -10.0}),
        }
    }
}
