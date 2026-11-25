//! The blocks that read circuit quantities into the controls.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{ControlSpec, Scope};
use crate::catalog::{control, p, ComponentInfo, SIGNAL};
use crate::check::{check_bus, check_component, check_node, join};
use crate::components::{Component, PHASES};
use crate::diagnostic::Diagnostic;

/// A circuit quantity read into the controls (volt or ampere), taken from
/// the circuit solution at the grid point at which the controls are
/// evaluated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Measurement {
    pub name: String,
    pub quantity: Quantity,
}

/// A circuit quantity.
///
/// A current or voltage of a component with several (a three-phase
/// component, a transformer) is chosen with `terminal` and `phase`, which
/// are the parts of its column name: `I(LINE1.0.a)` is
/// `{"type": "branch_current", "component": "LINE1", "terminal": 0, "phase": "a"}`,
/// `I(Z1.b)` has only `"phase": "b"`, `I(T1.1)` only `"terminal": 1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Quantity {
    /// Voltage of a node against gnd.
    NodeVoltage { node: String },
    /// Current of a component, with the sign of its column `I(...)`: from
    /// nodes[0] to nodes[1] through a component with two nodes.
    BranchCurrent {
        component: String,
        /// Terminal (or winding) of a component with several: 0 or 1.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal: Option<u32>,
        /// Phase of a three-phase component: "a", "b" or "c" ("g" for the
        /// current of a fault to ground).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<String>,
    },
    /// Branch voltage of a component, with the sign of its column `U(...)`:
    /// V(nodes[0]) - V(nodes[1]).
    BranchVoltage {
        component: String,
        /// Phase of a three-phase component: "a", "b" or "c".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<String>,
    },
    /// An internal quantity that a component offers as a signal (the speed
    /// of a machine, for example), at the grid point.
    ComponentSignal { component: String, signal: String },
}

impl Quantity {
    /// The suffix of the column of a chosen current or voltage: "",
    /// ".a", ".0", ".0.a".
    pub fn column_suffix(terminal: Option<u32>, phase: Option<&str>) -> String {
        let mut s = String::new();
        if let Some(t) = terminal {
            s.push_str(&format!(".{t}"));
        }
        if let Some(ph) = phase {
            s.push_str(&format!(".{ph}"));
        }
        s
    }

    /// E173: `component` has no current (`letter` "I") or branch voltage
    /// ("U") whose column has the suffix `suffix`; `existing` are the
    /// suffixes of those it has.
    pub fn missing(
        letter: &str,
        component: &str,
        suffix: &str,
        existing: &[String],
        path: &str,
    ) -> Diagnostic {
        let hint = if existing.is_empty() {
            format!(
                "\"{component}\" has no branch voltages. Measure the voltages of its nodes with \
                 node_voltage, or of a bus with a three_phase_measurement block."
            )
        } else {
            format!(
                "\"terminal\" and \"phase\" are the parts of a column name after the name of \
                 the component. \"{component}\" has: {}.",
                existing
                    .iter()
                    .map(|s| format!("{letter}({component}{s})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        Diagnostic::error(
            "E173",
            path,
            format!("\"{component}\" has no quantity {letter}({component}{suffix})"),
            hint,
        )
    }
}

/// Reports a current ("I") or branch voltage ("U") of `target` that the
/// suffix does not name. False when it was reported.
fn check_suffix(
    target: &Component,
    letter: &str,
    suffix: &str,
    path: &str,
    d: &mut Vec<Diagnostic>,
) -> bool {
    let existing = match letter {
        "I" => match target.current_suffixes() {
            Some(e) => e,
            // The solver reports it when it builds the circuit.
            None => return true,
        },
        _ => target.voltage_suffixes(),
    };
    if existing.iter().any(|e| e == suffix) {
        return true;
    }
    d.push(Quantity::missing(
        letter,
        target.name(),
        suffix,
        &existing,
        path,
    ));
    false
}

/// E173: a quantity of a component that the choice of terminal and phase
/// does not name.
fn choice_error(d: &mut Vec<Diagnostic>, path: String, message: String, hint: &str) {
    d.push(Diagnostic::error("E173", path, message, hint.to_string()));
}

const CHOICE_HINT: &str = "Choose one with \"terminal\" and \"phase\", the parts of its column \
    name: I(LINE1.0.a) is {\"type\": \"branch_current\", \"component\": \"LINE1\", \"terminal\": 0, \
    \"phase\": \"a\"}; I(Z1.b) has only \"phase\": \"b\". A three_phase_measurement block reads the \
    three phases at once.";

fn find<'a>(scope: &'a Scope, component: &str) -> Option<&'a Component> {
    scope.components.iter().find(|x| x.name() == component)
}

impl ControlSpec for Measurement {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "measurement"
    }
    fn since(&self) -> &'static str {
        match &self.quantity {
            Quantity::NodeVoltage { .. } => "0.2",
            Quantity::BranchCurrent {
                terminal, phase, ..
            } if terminal.is_none() && phase.is_none() => "0.2",
            Quantity::BranchVoltage { phase, .. } if phase.is_none() => "0.2",
            _ => "0.5",
        }
    }
    fn check(&self, p: &str, scope: &Scope, d: &mut Vec<Diagnostic>) {
        let (component, chosen, letter, suffix) = match &self.quantity {
            Quantity::NodeVoltage { node } => {
                check_node(node, format!("{p}.quantity.node"), scope.nodes, d);
                return;
            }
            Quantity::BranchCurrent {
                component,
                terminal,
                phase,
            } => (
                component,
                terminal.is_some() || phase.is_some(),
                "I",
                Quantity::column_suffix(*terminal, phase.as_deref()),
            ),
            Quantity::BranchVoltage { component, phase } => (
                component,
                phase.is_some(),
                "U",
                Quantity::column_suffix(None, phase.as_deref()),
            ),
            Quantity::ComponentSignal { component, signal } => {
                let path = format!("{p}.quantity.component");
                check_component(component, path, scope.component_names, d);
                if let Some(target) = find(scope, component) {
                    let offered = target.signal_names();
                    if !offered.contains(&signal.as_str()) {
                        d.push(Diagnostic::error(
                            "E244",
                            format!("{p}.quantity.signal"),
                            format!(
                                "\"{component}\" ({}) does not offer a signal \"{signal}\"",
                                target.type_name()
                            ),
                            if offered.is_empty() {
                                "This component type offers no signals. Measure its currents and \
                                 voltages with branch_current, branch_voltage or node_voltage."
                                    .to_string()
                            } else {
                                format!("Its signals: {}.", join(offered.iter().copied()))
                            },
                        ));
                    }
                }
                return;
            }
        };
        check_component(
            component,
            format!("{p}.quantity.component"),
            scope.component_names,
            d,
        );
        if let Some(target) = find(scope, component) {
            let single = target.two_nodes().is_some();
            if !single && !chosen {
                choice_error(
                    d,
                    format!("{p}.quantity.component"),
                    format!(
                        "\"{component}\" ({}) has more than one current and voltage",
                        target.type_name()
                    ),
                    CHOICE_HINT,
                );
            } else if single && chosen {
                choice_error(
                    d,
                    format!("{p}.quantity"),
                    format!(
                        "\"{component}\" ({}) has one current and one voltage, without terminal \
                         or phase",
                        target.type_name()
                    ),
                    "Remove \"terminal\" and \"phase\".",
                );
            } else if !single {
                check_suffix(target, letter, &suffix, &format!("{p}.quantity"), d);
            }
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "measurement",
            category: "control",
            summary: "A circuit quantity (V or A) read into the controls from the circuit solution at the grid point at which the controls are evaluated.",
            sign_convention: SIGNAL,
            parameters: control(vec![p(
                "quantity",
                "V or A",
                true,
                "{\"type\":\"node_voltage\",\"node\":..} | {\"type\":\"branch_current\",\"component\":..,\"terminal\":..,\"phase\":..} | {\"type\":\"branch_voltage\",\"component\":..,\"phase\":..} | {\"type\":\"component_signal\",\"component\":..,\"signal\":..}, with the sign conventions of the outputs. terminal (0 or 1) and phase (\"a\", \"b\", \"c\") choose a quantity of a component with several, as in its column name: I(LINE1.0.a) is component LINE1, terminal 0, phase a.",
            )]),
            example: json!({"type": "measurement", "name": "v_out", "quantity": {"type": "node_voltage", "node": "out"}}),
        }
    }
}

/// Three phase quantities read into the controls as the signals `<name>.a`,
/// `<name>.b` and `<name>.c`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreePhaseMeasurement {
    pub name: String,
    pub quantity: ThreePhaseQuantity,
}

/// The three phase quantities of a bus or of a three-phase component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ThreePhaseQuantity {
    /// The voltages of the phase nodes of a bus against gnd: the columns
    /// `V(<bus>.a)`, `V(<bus>.b)` and `V(<bus>.c)`.
    BusVoltage { bus: String },
    /// The phase currents of a three-phase component: the columns
    /// `I(<component>.a)` ..., or `I(<component>.<terminal>.a)` ... of a
    /// component with two terminals (a line, a transformer).
    BranchCurrent {
        component: String,
        /// Terminal of a component with two: 0 or 1.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal: Option<u32>,
    },
}

impl ControlSpec for ThreePhaseMeasurement {
    fn name(&self) -> &str {
        &self.name
    }
    fn type_name(&self) -> &'static str {
        "three_phase_measurement"
    }
    fn since(&self) -> &'static str {
        "0.5"
    }
    fn outputs(&self) -> Vec<&'static str> {
        vec!["a", "b", "c"]
    }
    fn check(&self, p: &str, scope: &Scope, d: &mut Vec<Diagnostic>) {
        match &self.quantity {
            ThreePhaseQuantity::BusVoltage { bus } => {
                check_bus(bus, &format!("{p}.quantity.bus"), scope.nodes, d);
            }
            ThreePhaseQuantity::BranchCurrent {
                component,
                terminal,
            } => {
                let path = format!("{p}.quantity.component");
                check_component(component, path.clone(), scope.component_names, d);
                if let Some(target) = find(scope, component) {
                    if target.two_nodes().is_some() {
                        choice_error(
                            d,
                            path,
                            format!(
                                "\"{component}\" ({}) has one current, not three phase currents",
                                target.type_name()
                            ),
                            "Read it with a measurement block: {\"type\": \"branch_current\", \
                             \"component\": ...}.",
                        );
                    } else {
                        // One diagnostic for the block is enough.
                        let at = format!("{p}.quantity");
                        PHASES.iter().all(|ph| {
                            let suffix = Quantity::column_suffix(*terminal, Some(ph));
                            check_suffix(target, "I", &suffix, &at, d)
                        });
                    }
                }
            }
        }
    }
    fn info() -> ComponentInfo {
        ComponentInfo {
            type_name: "three_phase_measurement",
            category: "control",
            summary: "Three phase quantities (V or A) read into the controls as the signals <name>.a, <name>.b and <name>.c, from the circuit solution at the grid point.",
            sign_convention: "The block's name is the prefix of its output signals. The values and signs are those of the columns V(<bus>.a) ... and I(<component>.a) ... (I(<component>.<terminal>.a) ... with a terminal). Blocks are evaluated at every grid point; their outputs act on the circuit over the next time step.",
            parameters: control(vec![p(
                "quantity",
                "V or A",
                true,
                "{\"type\":\"bus_voltage\",\"bus\":..} | {\"type\":\"branch_current\",\"component\":..,\"terminal\":..}; terminal (0 or 1) for a component with two terminals (a line, a transformer).",
            )]),
            example: json!({"type": "three_phase_measurement", "name": "v", "quantity": {"type": "bus_voltage", "bus": "b1"}}),
        }
    }
}
