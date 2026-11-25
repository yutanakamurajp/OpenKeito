//! Passive components: resistor, inductor, capacitor and coupled RL
//! branches.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::three_phase::{check_series_buses, phase_pairs, BusRef, Impedance, PhaseMatrix};
use super::{ComponentInitialization, ComponentSpec};
use crate::catalog::{self, p, with, ComponentInfo, COMMON_SIGN, IMPEDANCE};
use crate::check::{check_impedance, positive, range_error};
use crate::diagnostic::Diagnostic;
use crate::model::Simulation;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Resistor {
    pub name: String,
    pub nodes: [String; 2],
    /// Resistance in ohm. Must be > 0.
    pub resistance_ohm: f64,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ComponentSpec for Resistor {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "resistor"
    }
    fn since(&self) -> &'static str {
        "0.1"
    }
    fn two_nodes(&self) -> Option<&[String; 2]> {
        Some(&self.nodes)
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        if !positive(self.resistance_ohm) {
            range_error(
                d,
                p,
                &self.name,
                "resistance_ohm",
                self.resistance_ohm,
                "Resistance must be > 0. For a short circuit, join the two nodes or use a closed switch.",
            );
        }
    }
    fn check_simulation(&self, _sim: &Simulation, p: &str, d: &mut Vec<Diagnostic>) {
        if positive(self.resistance_ohm) && !positive(1.0 / self.resistance_ohm) {
            range_error(
                d,
                p,
                &self.name,
                "resistance_ohm",
                self.resistance_ohm,
                "Its conductance (1 / resistance_ohm) is not finite in double precision. Use a \
                 larger resistance or a closed switch.",
            );
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "resistor",
            category: "circuit",
            summary: "Linear resistor.",
            sign_convention: COMMON_SIGN,
            parameters: with(vec![p("resistance_ohm", "ohm", true, "Resistance, > 0.")]),
            example: json!({"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 10.0}),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Inductor {
    pub name: String,
    pub nodes: [String; 2],
    /// Inductance in henry. Must be > 0.
    pub inductance_h: f64,
    /// Current at t = 0 from nodes[0] to nodes[1], in ampere. Default 0.
    #[serde(default)]
    pub initial_current_a: f64,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ComponentSpec for Inductor {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "inductor"
    }
    fn since(&self) -> &'static str {
        "0.1"
    }
    fn two_nodes(&self) -> Option<&[String; 2]> {
        Some(&self.nodes)
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        if !positive(self.inductance_h) {
            range_error(
                d,
                p,
                &self.name,
                "inductance_h",
                self.inductance_h,
                "Inductance must be > 0.",
            );
        }
        if !self.initial_current_a.is_finite() {
            range_error(
                d,
                p,
                &self.name,
                "initial_current_a",
                self.initial_current_a,
                "Use a finite value.",
            );
        }
    }
    fn check_simulation(&self, sim: &Simulation, p: &str, d: &mut Vec<Diagnostic>) {
        if !positive(self.inductance_h) {
            return;
        }
        if sim
            .stage_coefficients()
            .into_iter()
            .any(|alpha| !positive(1.0 / (alpha * self.inductance_h)))
        {
            range_error(
                d,
                p,
                &self.name,
                "inductance_h",
                self.inductance_h,
                "Its step conductance (1 / (alpha * inductance_h)) is zero or not finite in \
                 double precision for this time_step_s. Use a value within the solvable range or \
                 adjust time_step_s.",
            );
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "inductor",
            category: "circuit",
            summary: "Linear inductor.",
            sign_convention: COMMON_SIGN,
            parameters: with(vec![
                p("inductance_h", "H", true, "Inductance, > 0."),
                p(
                    "initial_current_a",
                    "A",
                    false,
                    "Current at t = 0. Default 0.",
                ),
            ]),
            example: json!({"type": "inductor", "name": "L1", "nodes": ["n2", "n3"], "inductance_h": 0.01, "initial_current_a": 0.0}),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Capacitor {
    pub name: String,
    pub nodes: [String; 2],
    /// Capacitance in farad. Must be > 0.
    pub capacitance_f: f64,
    /// Voltage V(nodes[0]) - V(nodes[1]) at t = 0, in volt. Default 0.
    #[serde(default)]
    pub initial_voltage_v: f64,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ComponentSpec for Capacitor {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "capacitor"
    }
    fn since(&self) -> &'static str {
        "0.1"
    }
    fn two_nodes(&self) -> Option<&[String; 2]> {
        Some(&self.nodes)
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        if !positive(self.capacitance_f) {
            range_error(
                d,
                p,
                &self.name,
                "capacitance_f",
                self.capacitance_f,
                "Capacitance must be > 0.",
            );
        }
        if !self.initial_voltage_v.is_finite() {
            range_error(
                d,
                p,
                &self.name,
                "initial_voltage_v",
                self.initial_voltage_v,
                "Use a finite value.",
            );
        }
    }
    fn check_simulation(&self, sim: &Simulation, p: &str, d: &mut Vec<Diagnostic>) {
        if !positive(self.capacitance_f) {
            return;
        }
        if sim
            .stage_coefficients()
            .into_iter()
            .any(|alpha| !positive(alpha * self.capacitance_f))
        {
            range_error(
                d,
                p,
                &self.name,
                "capacitance_f",
                self.capacitance_f,
                "Its step admittance (alpha * capacitance_f) is zero or not finite in double \
                 precision for this time_step_s. Use a value within the solvable range or adjust \
                 time_step_s.",
            );
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "capacitor",
            category: "circuit",
            summary: "Linear capacitor.",
            sign_convention: COMMON_SIGN,
            parameters: with(vec![
                p("capacitance_f", "F", true, "Capacitance, > 0."),
                p(
                    "initial_voltage_v",
                    "V",
                    false,
                    "Voltage at t = 0. Default 0.",
                ),
            ]),
            example: json!({"type": "capacitor", "name": "C1", "nodes": ["n3", "gnd"], "capacitance_f": 1e-6, "initial_voltage_v": 0.0}),
        }
    }
}

/// A scheduled change of the impedance of a `coupled_rl` branch (format 0.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CoupledRlChange {
    /// Time at which the branch takes this impedance, in seconds (>= 0).
    pub time_s: f64,
    pub impedance: Impedance,
}

/// Three coupled series branches with resistance and inductance:
/// v = R i + L di/dt, with v = V(buses[0].p) - V(buses[1].p) and i the
/// phase currents from buses[0] to buses[1]. A shunt load is a coupled_rl to
/// "gnd" (grounded star) or to ["n", "n", "n"] (star point n).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CoupledRl {
    pub name: String,
    pub buses: [BusRef; 2],
    pub impedance: Impedance,
    /// Scheduled impedance changes at times >= 0 s, in strictly ascending
    /// order (format 0.6).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub impedance_changes: Vec<CoupledRlChange>,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

impl ComponentSpec for CoupledRl {
    fn current_suffixes(&self) -> Option<Vec<String>> {
        Some(super::phase_suffixes(&[""]))
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "coupled_rl"
    }
    fn since(&self) -> &'static str {
        if self.impedance_changes.is_empty() {
            "0.3"
        } else {
            "0.6"
        }
    }
    fn node_names(&self) -> Vec<String> {
        self.buses.iter().flat_map(BusRef::nodes).collect()
    }
    fn terminals(&self) -> Vec<super::Terminal> {
        super::series_terminals(&self.buses)
    }
    fn check_terminals(&self, p: &str, d: &mut Vec<Diagnostic>) {
        check_series_buses(&self.name, &self.buses, p, d);
    }
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        check_impedance(&self.impedance, &format!("{p}.impedance"), &self.name, d);
        let mut prev = None;
        for (k, ch) in self.impedance_changes.iter().enumerate() {
            let cp = format!("{p}.impedance_changes[{k}]");
            if !ch.time_s.is_finite() || ch.time_s < 0.0 {
                range_error(
                    d,
                    &cp,
                    &self.name,
                    "time_s",
                    ch.time_s,
                    "Change times must be >= 0 s.",
                );
            } else if let Some(last) = prev {
                if ch.time_s <= last {
                    crate::check::order_error(
                        d,
                        format!("{cp}.time_s"),
                        format!(
                            "impedance_changes[{k}].time_s ({}) of \"{}\" is not after the previous time ({last})",
                            ch.time_s, self.name
                        ),
                        "List impedance_changes in strictly ascending order of time_s.",
                    );
                }
            }
            if ch.time_s.is_finite() && ch.time_s >= 0.0 {
                prev = Some(ch.time_s);
            }
            check_impedance(&ch.impedance, &format!("{cp}.impedance"), &self.name, d);
        }
    }
    fn connections(&self) -> Vec<(String, String)> {
        phase_pairs(&self.buses)
    }
    fn series_impedance(&self) -> Option<(PhaseMatrix, PhaseMatrix, &'static str)> {
        let (r, l) = self.impedance.matrices();
        Some((r, l, "impedance"))
    }
    fn series_impedances(&self) -> Vec<(PhaseMatrix, PhaseMatrix, String)> {
        let mut out = vec![{
            let (r, l) = self.impedance.matrices();
            (r, l, "impedance".to_string())
        }];
        for (k, ch) in self.impedance_changes.iter().enumerate() {
            let (r, l) = ch.impedance.matrices();
            out.push((r, l, format!("impedance_changes[{k}].impedance")));
        }
        out
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "coupled_rl",
            category: "circuit",
            summary: "Three coupled series branches: v = R i + L di/dt with 3x3 matrices. A shunt load is a coupled_rl to \"gnd\" or to a star point [\"n\", \"n\", \"n\"].",
            sign_convention: "Phase currents I(<name>.a), I(<name>.b), I(<name>.c) are positive from buses[0] to buses[1]; branch voltages U(<name>.a) etc. are V(buses[0].p) - V(buses[1].p).",
            parameters: vec![
                catalog::name(),
                p("buses", "", true, "[from, to]; each a bus or three node names. See bus."),
                p("impedance", "ohm, H", true, IMPEDANCE),
                p("impedance_changes", "s, ohm, H", false, "Scheduled impedance changes (format 0.6): [{\"time_s\": .., \"impedance\": ..}] in strictly ascending order of time_s."),
            ],
            example: json!({"type": "coupled_rl", "name": "LOAD1", "buses": ["b2", "gnd"],
                "impedance": {"type": "matrix", "resistance_ohm": [[40.0, 0.0, 0.0], [0.0, 40.0, 0.0], [0.0, 0.0, 40.0]], "inductance_h": [[0.05, 0.0, 0.0], [0.0, 0.05, 0.0], [0.0, 0.0, 0.05]]},
                "impedance_changes": [{"time_s": 0.1, "impedance": {"type": "matrix", "resistance_ohm": [[50.0, 0.0, 0.0], [0.0, 50.0, 0.0], [0.0, 0.0, 50.0]], "inductance_h": [[0.05, 0.0, 0.0], [0.0, 0.05, 0.0], [0.0, 0.0, 0.05]]}}]}),
        }
    }
}
