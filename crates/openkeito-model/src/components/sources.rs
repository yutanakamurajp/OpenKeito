//! Sources: single-phase voltage and current sources with their waveforms,
//! the three-phase voltage source, and the sources whose values are control
//! signals.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::three_phase::{
    bus_and_neutral, check_bus_and_neutral, ground, neutral_pairs, BusRef, Impedance, PhaseMatrix,
};
use super::{Branch, ComponentInitialization, ComponentSpec, SteadyState};
use crate::catalog::{
    name, p, with, ComponentInfo, BUS, COMMON_SIGN, IMPEDANCE, THREE_PHASE_SOURCE_SIGN, WAVEFORM,
};
use crate::check::{check_impedance, positive, range_error};
use crate::diagnostic::Diagnostic;

/// Ideal voltage source: V(nodes[0]) - V(nodes[1]) = waveform(t), in volt.
/// Its branch current follows the common convention (from nodes[0] to
/// nodes[1] through the source), so a source that delivers power with
/// nodes[0] positive has a negative branch current.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VoltageSource {
    pub name: String,
    pub nodes: [String; 2],
    /// Waveform values are in volt.
    pub waveform: Waveform,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ComponentSpec for VoltageSource {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "voltage_source"
    }
    fn since(&self) -> &'static str {
        "0.1"
    }
    fn two_nodes(&self) -> Option<&[String; 2]> {
        Some(&self.nodes)
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        self.waveform.check(&self.name, p, d);
    }
    fn steady_state(&self) -> SteadyState {
        self.waveform.steady_state()
    }
    fn sine_frequency(&self) -> Option<f64> {
        self.waveform.sine_frequency()
    }
    fn branches(&self, _t: f64) -> Vec<Branch> {
        let [a, b] = &self.nodes;
        vec![Branch::Voltage(a.clone(), b.clone())]
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "voltage_source",
            category: "circuit",
            summary: "Ideal voltage source: V(nodes[0]) - V(nodes[1]) = waveform(t).",
            sign_convention:
                "Branch current is positive from nodes[0] to nodes[1] through the source, so a source delivering power with nodes[0] positive has a negative current.",
            parameters: with(vec![p("waveform", "V", true, WAVEFORM)]),
            example: json!({"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0, "phase_deg": 0.0, "offset": 0.0}}),
        }
    }
}

/// Ideal current source: waveform(t) ampere flows through the source from
/// nodes[0] to nodes[1], that is, it is injected into nodes[1].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CurrentSource {
    pub name: String,
    pub nodes: [String; 2],
    /// Waveform values are in ampere.
    pub waveform: Waveform,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ComponentSpec for CurrentSource {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "current_source"
    }
    fn since(&self) -> &'static str {
        "0.1"
    }
    fn two_nodes(&self) -> Option<&[String; 2]> {
        Some(&self.nodes)
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        self.waveform.check(&self.name, p, d);
    }
    fn steady_state(&self) -> SteadyState {
        self.waveform.steady_state()
    }
    fn sine_frequency(&self) -> Option<f64> {
        self.waveform.sine_frequency()
    }
    // A current source holds no voltage.
    fn branches(&self, _t: f64) -> Vec<Branch> {
        Vec::new()
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "current_source",
            category: "circuit",
            summary: "Ideal current source: waveform(t) flows through the source from nodes[0] to nodes[1], that is, into nodes[1].",
            sign_convention: COMMON_SIGN,
            parameters: with(vec![p("waveform", "A", true, WAVEFORM)]),
            example: json!({"type": "current_source", "name": "J1", "nodes": ["gnd", "n1"], "waveform": {"type": "dc", "value": 1.0}}),
        }
    }
}

/// Source waveform. Values are in volt for a voltage source and in ampere for
/// a current source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Waveform {
    /// Constant value, present from t = 0.
    Dc { value: f64 },
    /// `before`, then `after`. The source changes like a switch: at the first
    /// grid point at or after time_s, right after the output of that grid
    /// point (the row there still shows `before`).
    Step {
        time_s: f64,
        #[serde(default)]
        before: f64,
        after: f64,
    },
    /// offset + amplitude * sin(2*pi*frequency_hz*t + phase_deg). `amplitude`
    /// is the peak value. Use phase_deg = 90 for a cosine.
    Sine {
        amplitude: f64,
        frequency_hz: f64,
        #[serde(default)]
        phase_deg: f64,
        #[serde(default)]
        offset: f64,
    },
}

impl Waveform {
    /// Checks the waveform of the source `name` at `{p}.waveform`.
    fn check(&self, name: &str, p: &str, d: &mut Vec<Diagnostic>) {
        match self {
            Waveform::Dc { .. } => {}
            Waveform::Step { time_s, .. } => {
                if !time_s.is_finite() {
                    range_error(
                        d,
                        p,
                        name,
                        "waveform.time_s",
                        *time_s,
                        "Use a finite time in seconds.",
                    );
                }
            }
            Waveform::Sine { frequency_hz, .. } => {
                if !frequency_hz.is_finite() || *frequency_hz < 0.0 {
                    range_error(
                        d,
                        p,
                        name,
                        "waveform.frequency_hz",
                        *frequency_hz,
                        "Frequency must be >= 0 Hz.",
                    );
                }
            }
        }
    }

    /// A sine without offset has a phasor; a dc source of 0 is no source.
    fn steady_state(&self) -> SteadyState {
        match self {
            Waveform::Sine {
                frequency_hz,
                offset,
                ..
            } if *offset == 0.0 && *frequency_hz > 0.0 => SteadyState::Sine(*frequency_hz),
            Waveform::Dc { value } if *value == 0.0 => SteadyState::Linear,
            _ => SteadyState::NotSine("waveform"),
        }
    }

    fn sine_frequency(&self) -> Option<f64> {
        match self {
            Waveform::Sine { frequency_hz, .. } => Some(*frequency_hz),
            _ => None,
        }
    }
}

/// Balanced three-phase voltage source behind an impedance. Phase a is
/// v_a(t) = sqrt(2/3) * line_voltage_rms_v * cos(2*pi*frequency_hz*t + angle_deg);
/// phase b lags by 120 degrees and phase c leads by 120 degrees. The source
/// is connected between the star point `neutral` and `bus`, in series with
/// `impedance` (an ideal source when omitted).
///
/// The phase currents I(<name>.a) etc. flow from the bus into the source, as
/// for a voltage_source with nodes [bus, neutral]: a source that delivers
/// power has currents opposite to its voltages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreePhaseSource {
    pub name: String,
    pub bus: BusRef,
    /// Star point of the source. Default "gnd".
    #[serde(default = "ground")]
    pub neutral: String,
    /// Line-to-line voltage, rms, in volt. Must be >= 0.
    pub line_voltage_rms_v: f64,
    /// Must be > 0.
    pub frequency_hz: f64,
    /// Angle of the phase-a voltage phasor (cosine reference), the same as
    /// a power-flow voltage angle. Default 0.
    #[serde(default)]
    pub angle_deg: f64,
    /// Internal impedance. An ideal source when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impedance: Option<Impedance>,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ComponentSpec for ThreePhaseSource {
    fn current_suffixes(&self) -> Option<Vec<String>> {
        Some(super::phase_suffixes(&[""]))
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "three_phase_source"
    }
    fn since(&self) -> &'static str {
        "0.3"
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
        let name = self.name.as_str();
        if !self.line_voltage_rms_v.is_finite() || self.line_voltage_rms_v < 0.0 {
            range_error(
                d,
                p,
                name,
                "line_voltage_rms_v",
                self.line_voltage_rms_v,
                "Use a finite value >= 0.",
            );
        }
        if !positive(self.frequency_hz) {
            range_error(
                d,
                p,
                name,
                "frequency_hz",
                self.frequency_hz,
                "Frequency must be > 0 Hz, for example 50 or 60.",
            );
        }
        if !self.angle_deg.is_finite() {
            range_error(
                d,
                p,
                name,
                "angle_deg",
                self.angle_deg,
                "Use a finite angle in degrees.",
            );
        }
        if let Some(z) = &self.impedance {
            check_impedance(z, &format!("{p}.impedance"), name, d);
        }
    }
    fn connections(&self) -> Vec<(String, String)> {
        neutral_pairs(&self.bus, &self.neutral)
    }
    fn series_impedance(&self) -> Option<(PhaseMatrix, PhaseMatrix, &'static str)> {
        let (r, l) = self.impedance.as_ref()?.matrices();
        Some((r, l, "impedance"))
    }
    fn steady_state(&self) -> SteadyState {
        SteadyState::Sine(self.frequency_hz)
    }
    fn sine_frequency(&self) -> Option<f64> {
        Some(self.frequency_hz)
    }
    // Without an impedance the three sources are ideal branches at the bus.
    fn branches(&self, _t: f64) -> Vec<Branch> {
        let pairs = neutral_pairs(&self.bus, &self.neutral).into_iter();
        if self.impedance.is_some() {
            pairs.map(|(a, b)| Branch::Conductance(a, b)).collect()
        } else {
            pairs.map(|(a, b)| Branch::Voltage(a, b)).collect()
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "three_phase_source",
            category: "circuit",
            summary: "Balanced three-phase voltage source behind a coupled impedance. Phase a is sqrt(2/3)*V_LL*cos(2*pi*f*t + angle_deg); b lags by 120 degrees, c leads by 120 degrees.",
            sign_convention: THREE_PHASE_SOURCE_SIGN,
            parameters: vec![
                name(),
                p("bus", "", true, BUS),
                p("neutral", "", false, "Node of the star point. Default \"gnd\"."),
                p("line_voltage_rms_v", "V", true, "Line-to-line voltage, rms, >= 0."),
                p("frequency_hz", "Hz", true, "Frequency, > 0."),
                p("angle_deg", "deg", false, "Angle of the phase-a voltage phasor (cosine reference, as in a power flow). Default 0."),
                p("impedance", "ohm, H", false, IMPEDANCE),
            ],
            example: json!({"type": "three_phase_source", "name": "G1", "bus": "src", "neutral": "gnd", "line_voltage_rms_v": 66000.0, "frequency_hz": 50.0, "angle_deg": 0.0,
                "impedance": {"type": "sequence", "r1_ohm": 0.5, "l1_h": 0.0159, "r0_ohm": 1.0, "l0_h": 0.0477}}),
        }
    }
}

/// What every controlled source says of its signal.
const CONTROLLED: &str = "Name of the control signal. The source follows it one time step late: over the step from one grid point to the next it moves in a straight line from the value the signal had at the grid point before to the value it has at the start of the step, so source(t) = signal(t - time_step_s), with the signal joined by straight lines between grid points. At t = 0 the source has the value of the signal at t = 0.";

/// Phasor of a controlled voltage source in the steady-state initialization:
/// v(t) = sqrt(2) * voltage_rms_v * cos(w t + angle_deg).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SteadyVoltage {
    /// Rms value in volt. Must be >= 0.
    pub voltage_rms_v: f64,
    /// Angle of the phasor (cosine reference). Default 0.
    #[serde(default)]
    pub angle_deg: f64,
}

/// Phasor of a controlled current source in the steady-state initialization:
/// i(t) = sqrt(2) * current_rms_a * cos(w t + angle_deg); for a three-phase
/// source that of phase a, with b lagging and c leading by 120 degrees.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SteadyCurrent {
    /// Rms value in ampere. Must be >= 0.
    pub current_rms_a: f64,
    /// Angle of the phasor (cosine reference). Default 0.
    #[serde(default)]
    pub angle_deg: f64,
}

/// Phasors of a three-phase controlled voltage source in the steady-state
/// initialization, as those of a three_phase_source: phase a is
/// sqrt(2/3) * line_voltage_rms_v * cos(w t + angle_deg), b lags and c leads
/// by 120 degrees.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SteadyLineVoltage {
    /// Line-to-line voltage, rms, in volt. Must be >= 0.
    pub line_voltage_rms_v: f64,
    /// Angle of the phase-a voltage phasor (cosine reference). Default 0.
    #[serde(default)]
    pub angle_deg: f64,
}

/// E130 for a steady-state phasor with an rms value below 0 or values that
/// are not finite.
fn check_steady(
    d: &mut Vec<Diagnostic>,
    p: &str,
    name: &str,
    (field, rms): (&str, f64),
    angle_deg: f64,
) {
    if !rms.is_finite() || rms < 0.0 {
        range_error(
            d,
            p,
            name,
            &format!("steady_state.{field}"),
            rms,
            "Use a finite rms value >= 0.",
        );
    }
    if !angle_deg.is_finite() {
        range_error(
            d,
            p,
            name,
            "steady_state.angle_deg",
            angle_deg,
            "Use a finite angle in degrees.",
        );
    }
}

/// Voltage source whose value is a control signal:
/// V(nodes[0]) - V(nodes[1]) = signal, in volt, one time step late.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControlledVoltageSource {
    pub name: String,
    pub nodes: [String; 2],
    /// Name of the control signal, in volt.
    pub signal: String,
    /// Phasor of the source in the steady-state initialization. 0 when
    /// omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steady_state: Option<SteadyVoltage>,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ComponentSpec for ControlledVoltageSource {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "controlled_voltage_source"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn two_nodes(&self) -> Option<&[String; 2]> {
        Some(&self.nodes)
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        if let Some(s) = &self.steady_state {
            check_steady(
                d,
                p,
                &self.name,
                ("voltage_rms_v", s.voltage_rms_v),
                s.angle_deg,
            );
        }
    }
    fn signals(&self) -> Vec<(&'static str, &str)> {
        vec![("signal", self.signal.as_str())]
    }
    fn lacks_steady_state_phasor(&self) -> bool {
        self.steady_state.is_none()
    }
    fn branches(&self, _t: f64) -> Vec<Branch> {
        let [a, b] = &self.nodes;
        vec![Branch::Voltage(a.clone(), b.clone())]
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "controlled_voltage_source",
            category: "circuit",
            summary: "Voltage source whose value is a control signal: V(nodes[0]) - V(nodes[1]) = signal(t - time_step_s), the signal joined by straight lines between grid points.",
            sign_convention:
                "Branch current is positive from nodes[0] to nodes[1] through the source, so a source delivering power with nodes[0] positive has a negative current.",
            parameters: with(vec![
                p("signal", "V", true, CONTROLLED),
                p("steady_state", "V, deg", false, "{\"voltage_rms_v\":..,\"angle_deg\":..}. Phasor of the source in the steady-state initialization (rms, cosine reference: v(t) = sqrt(2) rms cos(w t + angle_deg)). Without it the source is 0 in the phasor solution (warning W240)."),
            ]),
            example: json!({"type": "controlled_voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "signal": "v_ref", "steady_state": {"voltage_rms_v": 70.7, "angle_deg": 0.0}}),
        }
    }
}

/// Current source whose value is a control signal: the signal, in ampere,
/// flows through the source from nodes[0] to nodes[1] (into nodes[1]), one
/// time step late.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControlledCurrentSource {
    pub name: String,
    pub nodes: [String; 2],
    /// Name of the control signal, in ampere.
    pub signal: String,
    /// Phasor of the source in the steady-state initialization. 0 when
    /// omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steady_state: Option<SteadyCurrent>,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ComponentSpec for ControlledCurrentSource {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "controlled_current_source"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn two_nodes(&self) -> Option<&[String; 2]> {
        Some(&self.nodes)
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        if let Some(s) = &self.steady_state {
            check_steady(
                d,
                p,
                &self.name,
                ("current_rms_a", s.current_rms_a),
                s.angle_deg,
            );
        }
    }
    fn signals(&self) -> Vec<(&'static str, &str)> {
        vec![("signal", self.signal.as_str())]
    }
    fn lacks_steady_state_phasor(&self) -> bool {
        self.steady_state.is_none()
    }
    // A current source holds no voltage.
    fn branches(&self, _t: f64) -> Vec<Branch> {
        Vec::new()
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "controlled_current_source",
            category: "circuit",
            summary: "Current source whose value is a control signal: signal(t - time_step_s) flows through the source from nodes[0] to nodes[1], that is, into nodes[1]; the signal is joined by straight lines between grid points.",
            sign_convention: COMMON_SIGN,
            parameters: with(vec![
                p("signal", "A", true, CONTROLLED),
                p("steady_state", "A, deg", false, "{\"current_rms_a\":..,\"angle_deg\":..}. Phasor of the source in the steady-state initialization (rms, cosine reference). Without it the source is 0 in the phasor solution (warning W240)."),
            ]),
            example: json!({"type": "controlled_current_source", "name": "J1", "nodes": ["gnd", "n1"], "signal": "i_ref", "steady_state": {"current_rms_a": 10.0, "angle_deg": 0.0}}),
        }
    }
}

/// Three-phase voltage source whose phase voltages are three control
/// signals, behind an impedance: the source of the phases is connected
/// between the star point `neutral` and `bus`, in series with `impedance` (an
/// ideal source when omitted). The signals are the voltages of the phases a,
/// b and c against the neutral, in volt, one time step late.
///
/// The phase currents I(<name>.a) etc. flow from the bus into the source, as
/// those of a three_phase_source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreePhaseControlledSource {
    pub name: String,
    pub bus: BusRef,
    /// Star point of the source. Default "gnd".
    #[serde(default = "ground")]
    pub neutral: String,
    /// Names of the control signals of the phases [a, b, c], in volt.
    pub signals: [String; 3],
    /// Series impedance. An ideal source when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impedance: Option<Impedance>,
    /// Phasors of the source in the steady-state initialization. 0 when
    /// omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steady_state: Option<SteadyLineVoltage>,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ComponentSpec for ThreePhaseControlledSource {
    fn current_suffixes(&self) -> Option<Vec<String>> {
        Some(super::phase_suffixes(&[""]))
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "three_phase_controlled_source"
    }
    fn since(&self) -> &'static str {
        "0.5"
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
        let name = self.name.as_str();
        if let Some(z) = &self.impedance {
            check_impedance(z, &format!("{p}.impedance"), name, d);
        }
        if let Some(s) = &self.steady_state {
            check_steady(
                d,
                p,
                name,
                ("line_voltage_rms_v", s.line_voltage_rms_v),
                s.angle_deg,
            );
        }
    }
    fn connections(&self) -> Vec<(String, String)> {
        neutral_pairs(&self.bus, &self.neutral)
    }
    fn series_impedance(&self) -> Option<(PhaseMatrix, PhaseMatrix, &'static str)> {
        let (r, l) = self.impedance.as_ref()?.matrices();
        Some((r, l, "impedance"))
    }
    fn signals(&self) -> Vec<(&'static str, &str)> {
        vec![
            ("signals[0]", self.signals[0].as_str()),
            ("signals[1]", self.signals[1].as_str()),
            ("signals[2]", self.signals[2].as_str()),
        ]
    }
    fn lacks_steady_state_phasor(&self) -> bool {
        self.steady_state.is_none()
    }
    // Without an impedance the three sources are ideal branches at the bus.
    fn branches(&self, _t: f64) -> Vec<Branch> {
        let pairs = neutral_pairs(&self.bus, &self.neutral).into_iter();
        if self.impedance.is_some() {
            pairs.map(|(a, b)| Branch::Conductance(a, b)).collect()
        } else {
            pairs.map(|(a, b)| Branch::Voltage(a, b)).collect()
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "three_phase_controlled_source",
            category: "circuit",
            summary: "Three-phase voltage source whose phase voltages (against the neutral) are three control signals, behind a coupled impedance: the voltage behind the reactor of an averaged converter. Each phase is signal(t - time_step_s), the signal joined by straight lines between grid points.",
            sign_convention: THREE_PHASE_SOURCE_SIGN,
            parameters: vec![
                name(),
                p("bus", "", true, BUS),
                p("neutral", "", false, "Node of the star point. Default \"gnd\"."),
                p("signals", "V", true, "Names of the control signals of the phases [a, b, c]. The source follows them one time step late; at t = 0 it has their values at t = 0."),
                p("impedance", "ohm, H", false, IMPEDANCE),
                p("steady_state", "V, deg", false, "{\"line_voltage_rms_v\":..,\"angle_deg\":..}. Phasors of the source in the steady-state initialization, as those of a three_phase_source (phase a: sqrt(2/3) V_LL cos(w t + angle_deg)). Without it the source is 0 in the phasor solution (warning W240)."),
            ],
            example: json!({"type": "three_phase_controlled_source", "name": "VSC1", "bus": "b3", "neutral": "gnd", "signals": ["v_ref.a", "v_ref.b", "v_ref.c"],
                "impedance": {"type": "sequence", "r1_ohm": 0.05, "l1_h": 0.005, "r0_ohm": 0.05, "l0_h": 0.005},
                "steady_state": {"line_voltage_rms_v": 6600.0, "angle_deg": 0.0}}),
        }
    }
}

/// Three-phase current source whose phase currents are three control
/// signals: each signal, in ampere, is injected into its phase of the bus
/// from the neutral, one time step late.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreePhaseControlledCurrentSource {
    pub name: String,
    pub bus: BusRef,
    /// Node the currents return through. Default "gnd".
    #[serde(default = "ground")]
    pub neutral: String,
    /// Names of the control signals of the phases [a, b, c], in ampere.
    pub signals: [String; 3],
    /// Phasors of the source in the steady-state initialization. 0 when
    /// omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steady_state: Option<SteadyCurrent>,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ComponentSpec for ThreePhaseControlledCurrentSource {
    fn current_suffixes(&self) -> Option<Vec<String>> {
        Some(super::phase_suffixes(&[""]))
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "three_phase_controlled_current_source"
    }
    fn since(&self) -> &'static str {
        "0.5"
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
        if let Some(s) = &self.steady_state {
            check_steady(
                d,
                p,
                &self.name,
                ("current_rms_a", s.current_rms_a),
                s.angle_deg,
            );
        }
    }
    fn connections(&self) -> Vec<(String, String)> {
        neutral_pairs(&self.bus, &self.neutral)
    }
    fn signals(&self) -> Vec<(&'static str, &str)> {
        vec![
            ("signals[0]", self.signals[0].as_str()),
            ("signals[1]", self.signals[1].as_str()),
            ("signals[2]", self.signals[2].as_str()),
        ]
    }
    fn lacks_steady_state_phasor(&self) -> bool {
        self.steady_state.is_none()
    }
    // A current source holds no voltage.
    fn branches(&self, _t: f64) -> Vec<Branch> {
        Vec::new()
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "three_phase_controlled_current_source",
            category: "circuit",
            summary: "Three-phase current source whose phase currents are three control signals, injected into the bus from the neutral. Each phase is signal(t - time_step_s), the signal joined by straight lines between grid points.",
            sign_convention: "Phase currents I(<name>.a) etc. flow from the neutral through the source into the bus (as a current_source with nodes [neutral, bus]). Phasors are rms with a cosine reference.",
            parameters: vec![
                name(),
                p("bus", "", true, BUS),
                p("neutral", "", false, "Node the currents return through. Default \"gnd\"."),
                p("signals", "A", true, "Names of the control signals of the phases [a, b, c]. The source follows them one time step late; at t = 0 it has their values at t = 0."),
                p("steady_state", "A, deg", false, "{\"current_rms_a\":..,\"angle_deg\":..}. Phasors of the source in the steady-state initialization, as those of a three_phase_current_source (phase a: sqrt(2) I cos(w t + angle_deg)). Without it the source is 0 in the phasor solution (warning W240)."),
            ],
            example: json!({"type": "three_phase_controlled_current_source", "name": "INV1", "bus": "b3", "neutral": "gnd", "signals": ["i_ref.a", "i_ref.b", "i_ref.c"],
                "steady_state": {"current_rms_a": 100.0, "angle_deg": 0.0}}),
        }
    }
}
