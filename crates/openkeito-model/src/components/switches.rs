//! Switching components: the time-controlled switch, the diode, the switch
//! driven by a control signal and the fault.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::three_phase::{BusRef, Phase};
use super::{Branch, ComponentInitialization, ComponentSpec, SteadyState};
use crate::catalog::{self, p, resistances, with, ComponentInfo, BUS, COMMON_SIGN, TWO_STATE};
use crate::check::{bus_ref as bus, order_error, positive, range_error, same_node as same};
use crate::diagnostic::Diagnostic;
use crate::model::GROUND;

/// Ideal time-controlled switch (0 ohm closed, open circuit when open).
///
/// A switching action takes effect at the first time grid point at or after
/// its time, right after the output of that grid point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Switch {
    pub name: String,
    pub nodes: [String; 2],
    /// State at t = 0 before any action. Default false (open).
    #[serde(default)]
    pub initially_closed: bool,
    /// Time of the closing action in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_time_s: Option<f64>,
    /// Time of the opening command in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_time_s: Option<f64>,
    /// When true (default), the switch opens at the first grid point after
    /// the opening command at which its current has crossed zero. When false
    /// it opens at the command, chopping the current.
    #[serde(default = "default_true")]
    pub open_at_current_zero: bool,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl Switch {
    /// The state the switch is commanded to at time `t`: the last of its
    /// actions at or before `t` (the opening when both are at one time),
    /// `initially_closed` before them.
    pub fn commanded_closed(&self, t: f64) -> bool {
        let mut events = [(self.close_time_s, true), (self.open_time_s, false)];
        events.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut state = self.initially_closed;
        for (time, closed) in events {
            if time.is_some_and(|time| time <= t) {
                state = closed;
            }
        }
        state
    }
}

impl ComponentSpec for Switch {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "switch"
    }
    fn since(&self) -> &'static str {
        "0.1"
    }
    fn two_nodes(&self) -> Option<&[String; 2]> {
        Some(&self.nodes)
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        let s = self;
        for (field, t) in [
            ("close_time_s", s.close_time_s),
            ("open_time_s", s.open_time_s),
        ] {
            if let Some(t) = t {
                if !t.is_finite() || t < 0.0 {
                    range_error(d, p, &s.name, field, t, "Switching times must be >= 0 s.");
                }
            }
        }
        if let (Some(a), Some(b)) = (s.close_time_s, s.open_time_s) {
            if a == b {
                d.push(Diagnostic::error(
                    "E131",
                    format!("{p}.open_time_s"),
                    format!(
                        "close_time_s and open_time_s of \"{}\" are both {a}",
                        s.name
                    ),
                    "Use different times for closing and opening.",
                ));
            }
        }
    }
    // Closed, the switch is an ideal branch; open, it is no branch at all.
    fn branches(&self, t: f64) -> Vec<Branch> {
        let [a, b] = &self.nodes;
        if self.commanded_closed(t) {
            vec![Branch::Voltage(a.clone(), b.clone())]
        } else {
            Vec::new()
        }
    }
    fn switching_times(&self) -> Vec<f64> {
        let times = self.close_time_s.into_iter().chain(self.open_time_s);
        times.collect()
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "switch",
            category: "circuit",
            summary: "Ideal time-controlled switch. An action takes effect at the first time grid point at or after its time, right after the output of that grid point.",
            sign_convention: COMMON_SIGN,
            parameters: with(vec![
                p("initially_closed", "", false, "State at t = 0. Default false."),
                p("close_time_s", "s", false, "Time of the closing action."),
                p("open_time_s", "s", false, "Time of the opening command."),
                p(
                    "open_at_current_zero",
                    "",
                    false,
                    "true (default): open at the first grid point after the command at which the current has crossed zero. false: open at the command.",
                ),
            ]),
            example: json!({"type": "switch", "name": "SW1", "nodes": ["n1", "n2"], "initially_closed": false, "close_time_s": 0.001, "open_time_s": 0.05, "open_at_current_zero": true}),
        }
    }
}

fn default_true() -> bool {
    true
}

/// Default conducting resistance of a diode or controlled switch, in ohm.
pub const DEFAULT_ON_RESISTANCE_OHM: f64 = 1e-3;
/// Default blocking resistance of a diode or controlled switch, in ohm.
pub const DEFAULT_OFF_RESISTANCE_OHM: f64 = 1e6;

/// A resistance of a diode or controlled switch: its value in ohm, and
/// whether the model gives it or leaves it to the default. In JSON it is a
/// number, as before; a model that writes the default value has given it.
/// The run warns when an element with a default resistance blocks more
/// than one device can (W271).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TwoStateResistance {
    ohm: f64,
    given: bool,
}

impl TwoStateResistance {
    /// A value that the model gives.
    pub fn given(ohm: f64) -> Self {
        TwoStateResistance { ohm, given: true }
    }

    /// The value in ohm.
    pub fn ohm(self) -> f64 {
        self.ohm
    }

    /// True when the model does not give the value.
    pub fn is_default(self) -> bool {
        !self.given
    }
}

impl PartialEq<f64> for TwoStateResistance {
    fn eq(&self, other: &f64) -> bool {
        self.ohm == *other
    }
}

impl Serialize for TwoStateResistance {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.ohm)
    }
}

impl<'de> Deserialize<'de> for TwoStateResistance {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        f64::deserialize(deserializer).map(TwoStateResistance::given)
    }
}

fn default_on_resistance() -> TwoStateResistance {
    TwoStateResistance {
        ohm: DEFAULT_ON_RESISTANCE_OHM,
        given: false,
    }
}

fn default_off_resistance() -> TwoStateResistance {
    TwoStateResistance {
        ohm: DEFAULT_OFF_RESISTANCE_OHM,
        given: false,
    }
}

/// Diode, modelled as a resistor with two values: `on_resistance_ohm` while
/// conducting and `off_resistance_ohm` while blocking. `nodes` is
/// [anode, cathode], so the forward current is a positive branch current.
///
/// A blocking diode turns on when its voltage is positive, a conducting one
/// turns off when its current is negative. The states are chosen so that
/// they hold at the end of every time step, and a change takes effect from
/// the start of that step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Diode {
    pub name: String,
    /// [anode, cathode].
    pub nodes: [String; 2],
    /// Resistance while conducting, in ohm. Must be > 0. Default 1e-3.
    #[serde(default = "default_on_resistance")]
    #[schemars(with = "f64")]
    pub on_resistance_ohm: TwoStateResistance,
    /// Resistance while blocking, in ohm. Must be greater than
    /// on_resistance_ohm. Default 1e6.
    #[serde(default = "default_off_resistance")]
    #[schemars(with = "f64")]
    pub off_resistance_ohm: TwoStateResistance,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ComponentSpec for Diode {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "diode"
    }
    fn since(&self) -> &'static str {
        "0.2"
    }
    fn two_nodes(&self) -> Option<&[String; 2]> {
        Some(&self.nodes)
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        check_resistances(
            &self.name,
            self.on_resistance_ohm.ohm(),
            self.off_resistance_ohm.ohm(),
            p,
            d,
        );
    }
    fn steady_state(&self) -> SteadyState {
        SteadyState::NoPhasor
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "diode",
            category: "circuit",
            summary: "Diode as a resistor with two values. It turns on when its voltage is positive and off when its current is negative; the states are chosen so that they hold at the end of every time step, and a change takes effect from the start of that step.",
            sign_convention: "nodes is [anode, cathode]; the forward current is a positive branch current. The element is a resistor of on_resistance_ohm or off_resistance_ohm.",
            parameters: with(resistances()),
            example: json!({"type": "diode", "name": "D1", "nodes": ["gnd", "sw"], "on_resistance_ohm": 1e-3, "off_resistance_ohm": 1e6}),
        }
    }
}

/// Switch driven by a control signal; it conducts in both directions while
/// on. Modelled as a resistor with two values, like the diode.
///
/// The switch is on while its gate signal is >= 0.5. The signal computed at
/// a grid point decides the state over the following time step. Before the
/// first step the switch is off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControlledSwitch {
    pub name: String,
    pub nodes: [String; 2],
    /// Name of the control signal that drives the switch.
    pub gate: String,
    /// Resistance while on, in ohm. Must be > 0. Default 1e-3.
    #[serde(default = "default_on_resistance")]
    #[schemars(with = "f64")]
    pub on_resistance_ohm: TwoStateResistance,
    /// Resistance while off, in ohm. Must be greater than on_resistance_ohm.
    /// Default 1e6.
    #[serde(default = "default_off_resistance")]
    #[schemars(with = "f64")]
    pub off_resistance_ohm: TwoStateResistance,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ComponentSpec for ControlledSwitch {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "controlled_switch"
    }
    fn since(&self) -> &'static str {
        "0.2"
    }
    fn two_nodes(&self) -> Option<&[String; 2]> {
        Some(&self.nodes)
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        check_resistances(
            &self.name,
            self.on_resistance_ohm.ohm(),
            self.off_resistance_ohm.ohm(),
            p,
            d,
        );
    }
    fn signals(&self) -> Vec<(&'static str, &str)> {
        vec![("gate", self.gate.as_str())]
    }
    fn steady_state(&self) -> SteadyState {
        SteadyState::NoPhasor
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "controlled_switch",
            category: "circuit",
            summary: "Switch driven by a control signal, conducting in both directions while on (gate signal >= 0.5). The signal at a grid point decides the state over the next time step; before the first step the switch is off.",
            sign_convention: TWO_STATE,
            parameters: with({
                let mut v = vec![p("gate", "", true, "Name of the control signal (a control block) that drives the switch.")];
                v.extend(resistances());
                v
            }),
            example: json!({"type": "controlled_switch", "name": "S1", "nodes": ["n1", "sw"], "gate": "pwm", "on_resistance_ohm": 1e-3, "off_resistance_ohm": 1e6}),
        }
    }
}

/// The two resistances of a diode or controlled switch.
fn check_resistances(name: &str, on: f64, off: f64, p: &str, d: &mut Vec<Diagnostic>) {
    let mut range =
        |field: &str, value: f64, what: &str| range_error(d, p, name, field, value, what);
    if !positive(on) {
        range(
            "on_resistance_ohm",
            on,
            "The conducting resistance must be > 0 (default 1e-3). An ideal 0 ohm would make \
             the equations singular when a conducting diode and a switch short a source.",
        );
    }
    if !positive(off) {
        range(
            "off_resistance_ohm",
            off,
            "The blocking resistance must be > 0 and finite (default 1e6).",
        );
    }
    if positive(on) && positive(off) && off <= on {
        d.push(Diagnostic::error(
            "E132",
            format!("{p}.off_resistance_ohm"),
            format!(
                "off_resistance_ohm ({off}) of \"{name}\" is not greater than on_resistance_ohm ({on})"
            ),
            "The blocking resistance must be greater than the conducting one, \
             typically by many orders (defaults 1e6 and 1e-3).",
        ));
    }
}

/// A fault on a bus: each phase in `phases` is connected to the fault point
/// through `fault_resistance_ohm`; with `ground`, the fault point is
/// connected to gnd through `ground_resistance_ohm`. The fault starts in all
/// phases at the first grid point at or after start_time_s (right after its
/// output, as a switch). After clear_time_s each phase opens at the first
/// grid point at which its own current has crossed zero (or at once, with
/// clear_at_current_zero false).
///
/// Currents: I(<name>.a) etc. from the bus into the fault, and with ground
/// I(<name>.g) from the fault point to gnd.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fault {
    pub name: String,
    pub bus: BusRef,
    /// Faulted phases, such as ["a"] (with ground: single line to ground),
    /// ["b", "c"] (line to line, or double line to ground) or ["a", "b", "c"].
    pub phases: Vec<Phase>,
    /// Whether the fault point is connected to ground. Default false.
    #[serde(default)]
    pub ground: bool,
    /// Resistance between each faulted phase and the fault point, in ohm
    /// (at least 0). Default 0.
    #[serde(default)]
    pub fault_resistance_ohm: f64,
    /// Resistance between the fault point and gnd, in ohm, >= 0. Default 0.
    #[serde(default)]
    pub ground_resistance_ohm: f64,
    /// Time at which the fault starts, in seconds, >= 0.
    pub start_time_s: f64,
    /// Time of the clearing command, in seconds, after start_time_s. The
    /// fault stays when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_time_s: Option<f64>,
    /// true (default): each phase opens at its first current zero after
    /// the command. false: all phases open at the command.
    #[serde(default = "default_true")]
    pub clear_at_current_zero: bool,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ComponentSpec for Fault {
    fn current_suffixes(&self) -> Option<Vec<String>> {
        let phases = self.phases.iter().map(|ph| format!(".{}", ph.as_str()));
        Some(
            phases
                .chain(self.ground.then(|| ".g".to_string()))
                .collect(),
        )
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "fault"
    }
    fn since(&self) -> &'static str {
        "0.3"
    }
    fn node_names(&self) -> Vec<String> {
        let nodes = self.bus.nodes();
        self.phases
            .iter()
            .map(|p| nodes[p.index()].clone())
            .collect()
    }
    fn terminals(&self) -> Vec<super::Terminal> {
        let phases: Vec<usize> = self.phases.iter().map(|p| p.index()).collect();
        let mut v = vec![super::Terminal::bus("bus", &self.bus).only(&phases)];
        // The fault point of a fault to ground: at gnd inside the component.
        if self.ground {
            v.push(super::Terminal::node("ground", GROUND));
        }
        v
    }
    fn check_terminals(&self, p: &str, d: &mut Vec<Diagnostic>) {
        let (f, name) = (self, self.name.as_str());
        bus(d, &f.bus, format!("{p}.bus"));
        if f.bus == BusRef::Bus(GROUND.into()) {
            d.push(Diagnostic::error(
                "E124",
                format!("{p}.bus"),
                format!("fault \"{name}\" is on gnd"),
                "Name the bus whose phases are faulted.",
            ));
        } else {
            // The nodes of the faulted phases, each phase once.
            let nodes = f.bus.nodes();
            let mut faulted: Vec<&str> = Vec::new();
            let mut seen = [false; 3];
            for ph in &f.phases {
                if !std::mem::replace(&mut seen[ph.index()], true) {
                    faulted.push(nodes[ph.index()].as_str());
                }
            }
            let distinct: BTreeSet<&str> = faulted.iter().copied().collect();
            if faulted.contains(&GROUND) {
                same(
                    d,
                    format!("{p}.bus"),
                    format!("a faulted phase of \"{name}\" is the node \"gnd\""),
                );
            } else if distinct.len() < faulted.len() {
                same(
                    d,
                    format!("{p}.bus"),
                    format!(
                        "the faulted phases of \"{name}\" are not different nodes: {}",
                        faulted.join(", ")
                    ),
                );
            }
        }
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        let (f, name) = (self, self.name.as_str());
        let non_negative = |d: &mut Vec<Diagnostic>, field: &str, v: f64| {
            if !v.is_finite() || v < 0.0 {
                range_error(d, p, name, field, v, "Use a finite value >= 0.");
            }
        };
        let mut seen = [false; 3];
        for (k, ph) in f.phases.iter().enumerate() {
            if std::mem::replace(&mut seen[ph.index()], true) {
                d.push(Diagnostic::error(
                    "E138",
                    format!("{p}.phases[{k}]"),
                    format!("phase {} of \"{name}\" is listed twice", ph.as_str()),
                    "List each faulted phase once.",
                ));
            }
        }
        if f.phases.is_empty() || (f.phases.len() == 1 && !f.ground) {
            d.push(Diagnostic::error(
                "E138",
                format!("{p}.phases"),
                format!(
                    "fault \"{name}\" has {} phase(s) and no ground: nothing is short-circuited",
                    f.phases.len()
                ),
                "A single phase needs \"ground\": true (single line to ground). A fault between \
             phases lists two or three phases.",
            ));
        }
        non_negative(d, "fault_resistance_ohm", f.fault_resistance_ohm);
        non_negative(d, "ground_resistance_ohm", f.ground_resistance_ohm);
        non_negative(d, "start_time_s", f.start_time_s);
        if let Some(clear) = f.clear_time_s {
            if !clear.is_finite() {
                range_error(
                    d,
                    p,
                    name,
                    "clear_time_s",
                    clear,
                    "Use a finite time in seconds.",
                );
            } else if f.start_time_s.is_finite() && clear <= f.start_time_s {
                order_error(
                    d,
                    format!("{p}.clear_time_s"),
                    format!(
                        "clear_time_s ({clear}) of \"{name}\" is not after start_time_s ({})",
                        f.start_time_s
                    ),
                    "The clearing command must come after the start of the fault. Omit \
                 clear_time_s for a fault that stays.",
                );
            }
        }
    }
    fn connections(&self) -> Vec<(String, String)> {
        let nodes = self.node_names();
        let mut v: Vec<(String, String)> = nodes
            .windows(2)
            .map(|w| (w[0].clone(), w[1].clone()))
            .collect();
        // No phase at all is an error of its own (E138).
        if let (true, Some(first)) = (self.ground, nodes.first()) {
            v.push((first.clone(), GROUND.to_string()));
        }
        v
    }
    fn has_branch_voltage(&self) -> bool {
        false
    }
    // Before it starts and after it is cleared the fault is no branch. While
    // it lasts, each phase is a resistance to the fault point, or an ideal
    // branch when fault_resistance_ohm is 0. The fault point is gnd, a node
    // with a resistance to gnd, or a node of its own that the solver holds
    // only while every phase is open.
    fn branches(&self, t: f64) -> Vec<Branch> {
        let closed = t >= self.start_time_s && self.clear_time_s.is_none_or(|clear| t < clear);
        let solid = self.ground && self.ground_resistance_ohm == 0.0;
        let point = if solid {
            GROUND.to_string()
        } else {
            format!("{}:f", self.name)
        };
        let mut v = Vec::new();
        if !solid && (self.ground || !closed) {
            v.push(Branch::Conductance(point.clone(), GROUND.to_string()));
        }
        if closed {
            for node in self.node_names() {
                v.push(if self.fault_resistance_ohm > 0.0 {
                    Branch::Conductance(node, point.clone())
                } else {
                    Branch::Voltage(node, point.clone())
                });
            }
        }
        v
    }
    fn switching_times(&self) -> Vec<f64> {
        let times = std::iter::once(self.start_time_s).chain(self.clear_time_s);
        times.collect()
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "fault",
            category: "circuit",
            summary: "Fault on a bus: each listed phase is connected to the fault point through fault_resistance_ohm; with ground, the fault point is connected to gnd through ground_resistance_ohm. Starts in all phases at the first grid point at or after start_time_s; after clear_time_s each phase opens at its own first current zero.",
            sign_convention: "Phase currents I(<name>.a) etc. flow from the bus into the fault; with ground, I(<name>.g) flows from the fault point to gnd. Opening and closing of each phase is a switching event of <name>.a etc.",
            parameters: vec![
                catalog::name(),
                p("bus", "", true, BUS),
                p("phases", "", true, "Faulted phases: [\"a\"] with ground (single line to ground), [\"b\", \"c\"] (line to line, or double line to ground), [\"a\", \"b\", \"c\"]."),
                p("ground", "", false, "true: the fault point is connected to gnd. Default false."),
                p("fault_resistance_ohm", "ohm", false, "Resistance of each phase to the fault point, >= 0. Default 0."),
                p("ground_resistance_ohm", "ohm", false, "Resistance of the fault point to gnd, >= 0. Default 0."),
                p("start_time_s", "s", true, "Start of the fault, >= 0."),
                p("clear_time_s", "s", false, "Clearing command, after start_time_s. The fault stays when omitted."),
                p("clear_at_current_zero", "", false, "true (default): each phase opens at its first current zero after the command. false: at the command."),
            ],
            example: json!({"type": "fault", "name": "F1", "bus": "b2", "phases": ["b", "c"], "ground": true, "fault_resistance_ohm": 0.0, "ground_resistance_ohm": 1.0,
                "start_time_s": 0.1, "clear_time_s": 0.2, "clear_at_current_zero": true}),
        }
    }
}
