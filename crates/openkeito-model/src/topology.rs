//! The connections of a model as a document: which component is at which
//! node, which nodes are the phases of a bus, which control block reads which
//! signal and what in the circuit a signal acts on. It is what a diagram of
//! the model is drawn from, and what an agent reads to see the circuit
//! without following the names through the JSON.
//!
//! The document says where things are in the model with the paths of the
//! diagnostics (`components[2].nodes[0]`), and it is made of a model that
//! can be read, also when validation finds errors in it: `diagnostics` says
//! which components, blocks, nodes and buses each error and warning is about.

use std::collections::{BTreeSet, HashMap};

use serde::Serialize;
use serde_json::Value;

use crate::components::{expanded, Component, Terminal, PHASES};
use crate::controls::Control;
use crate::diagnostic::{Diagnostic, Severity, Subject};
use crate::model::{Model, Output, GROUND};

/// The connections of a model.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Topology {
    /// The components in the order of the model.
    pub components: Vec<ComponentTopology>,
    /// Every node that a component names, in the order of first use.
    pub nodes: Vec<NodeTopology>,
    /// Every bus that a three-phase terminal names, in the order of first use.
    pub buses: Vec<BusTopology>,
    /// The control blocks in the order of the model, then the blocks that
    /// the components bring with them (those of a grid-following converter),
    /// each with its `owner`.
    pub controls: Vec<ControlTopology>,
    /// Every control signal: the block that gives it and what reads it.
    pub signals: Vec<SignalTopology>,
    /// What each diagnostic of the model is about, errors first, in the
    /// order of the document of `validate`.
    pub diagnostics: Vec<Mark>,
}

/// A component and where it is connected.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ComponentTopology {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: &'static str,
    /// Its place in the model: "components[2]".
    pub path: String,
    pub terminals: Vec<TerminalTopology>,
    /// The control signals it reads (a gate, the value of a controlled
    /// source, the inputs of a machine).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<SignalUse>,
    /// The internal quantities it offers to `component_signal` measurements.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub offers: Vec<&'static str>,
}

/// A terminal of a component: the field that names it and its nodes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TerminalTopology {
    /// The field of the component: "nodes[0]", "bus", "buses[1]",
    /// "windings[0][1]", "neutral".
    pub name: String,
    /// Its place in the model: "components[2].nodes[0]". A field with a
    /// default that the model leaves out (`neutral`) has the path it would
    /// have.
    pub path: String,
    /// The bus of a three-phase terminal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bus: Option<String>,
    /// The phases of `nodes` of a three-phase terminal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phases: Option<Vec<&'static str>>,
    /// One node, or one per phase ("b1.a", "b1.b", "b1.c").
    pub nodes: Vec<String>,
}

/// A signal read by a component or a block.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignalUse {
    /// The field it is written in: "gate", "inputs[1]".
    pub field: String,
    pub path: String,
    pub signal: String,
}

/// A node of the circuit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeTopology {
    pub name: String,
    /// True for the reference node "gnd".
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ground: bool,
    /// The bus it is a phase of.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bus: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<&'static str>,
    /// The components with a terminal at the node, in the order of the model.
    pub components: Vec<String>,
}

/// A three-phase bus.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BusTopology {
    pub name: String,
    /// Its phase nodes a, b and c.
    pub nodes: [String; 3],
    /// The components with a three-phase terminal at the bus. A component
    /// at one of its phase nodes only is listed with that node.
    pub components: Vec<String>,
}

/// A control block.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ControlTopology {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: &'static str,
    /// Its place in the model: "controls[1]"; for a block of a component,
    /// the place of the component.
    pub path: String,
    /// The component that brings the block with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub inputs: Vec<SignalUse>,
    /// Its signals: `<name>`, or `<name>.<output>` of a block with several.
    pub outputs: Vec<String>,
    /// What a measurement block reads in the circuit: its `quantity` as the
    /// model writes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measures: Option<Value>,
}

/// A control signal with its ends.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignalTopology {
    pub name: String,
    /// The block that gives it; null for a name that no block gives.
    pub from: Option<String>,
    pub to: Vec<SignalEnd>,
}

/// What reads a signal: a block, a component or an output of the model.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignalEnd {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    /// The field of the block or component.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// The output of the model that records it: "outputs[3]".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// What a diagnostic is about. `code` and `path` are those of the
/// diagnostic; the lists name what a diagram marks.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Mark {
    pub severity: Severity,
    pub code: &'static str,
    pub path: String,
    pub components: Vec<String>,
    pub controls: Vec<String>,
    pub nodes: Vec<String>,
    pub buses: Vec<String>,
}

/// Adds `item` unless it is the last one already (a component at a node
/// with several of its terminals is listed once).
fn push_once(list: &mut Vec<String>, item: &str) {
    if list.last().map(String::as_str) != Some(item) {
        list.push(item.to_string());
    }
}

/// "components[2]" and what follows it of a path such as
/// "components[2].nodes[0]": the index and the rest without its dot.
fn indexed<'a>(path: &'a str, list: &str) -> Option<(usize, &'a str)> {
    let rest = path.strip_prefix(list)?.strip_prefix('[')?;
    let (index, rest) = rest.split_once(']')?;
    let rest = rest.strip_prefix('.').unwrap_or(rest);
    Some((index.parse().ok()?, rest))
}

/// The connections of a model that could be read. `diagnostics` are those
/// of `validate` for it (or none).
pub fn topology(model: &Model, diagnostics: &[Diagnostic]) -> Topology {
    let mut nodes: Vec<NodeTopology> = Vec::new();
    let mut node_at: HashMap<String, usize> = HashMap::new();
    let mut buses: Vec<BusTopology> = Vec::new();
    let mut bus_at: HashMap<String, usize> = HashMap::new();

    let mut components = Vec::with_capacity(model.components.len());
    for (i, c) in model.components.iter().enumerate() {
        let path = format!("components[{i}]");
        let terminals: Vec<Terminal> = c.terminals();
        for t in &terminals {
            if let Some(b) = &t.bus {
                let k = *bus_at.entry(b.clone()).or_insert_with(|| {
                    buses.push(BusTopology {
                        name: b.clone(),
                        nodes: PHASES.map(|p| format!("{b}.{p}")),
                        components: Vec::new(),
                    });
                    buses.len() - 1
                });
                push_once(&mut buses[k].components, c.name());
            }
            for n in &t.nodes {
                let k = *node_at.entry(n.clone()).or_insert_with(|| {
                    nodes.push(NodeTopology {
                        name: n.clone(),
                        ground: n == GROUND,
                        bus: None,
                        phase: None,
                        components: Vec::new(),
                    });
                    nodes.len() - 1
                });
                push_once(&mut nodes[k].components, c.name());
            }
        }
        components.push(ComponentTopology {
            name: c.name().to_string(),
            type_name: c.type_name(),
            terminals: terminals
                .into_iter()
                .map(|t| TerminalTopology {
                    path: format!("{path}.{}", t.name),
                    name: t.name,
                    bus: t.bus,
                    phases: t.phases,
                    nodes: t.nodes,
                })
                .collect(),
            signals: c
                .signals()
                .into_iter()
                .map(|(field, signal)| SignalUse {
                    field: field.to_string(),
                    path: format!("{path}.{field}"),
                    signal: signal.to_string(),
                })
                .collect(),
            offers: c.signal_names(),
            path,
        });
    }
    // A node "<bus>.a" is a phase of the bus, also for a single-phase
    // component that names it.
    for n in &mut nodes {
        let Some((bus, phase)) = n.name.rsplit_once('.') else {
            continue;
        };
        if let (true, Some(p)) = (
            bus_at.contains_key(bus),
            PHASES.iter().find(|p| **p == phase),
        ) {
            n.bus = Some(bus.to_string());
            n.phase = Some(p);
        }
    }

    // The blocks of the model, then those of its converters.
    let with_owned = expanded(model);
    let own = model.controls.len();
    let owner_of = |name: &str| {
        model.components.iter().enumerate().find(|(_, c)| {
            matches!(c, Component::GridFollowingConverter(_))
                && name
                    .strip_prefix(c.name())
                    .is_some_and(|rest| rest.starts_with('.'))
        })
    };
    let mut signals: Vec<SignalTopology> = Vec::new();
    let mut signal_at: HashMap<String, usize> = HashMap::new();
    let mut controls = Vec::with_capacity(with_owned.controls.len());
    for (i, c) in with_owned.controls.iter().enumerate() {
        let owner = if i < own { None } else { owner_of(c.name()) };
        let path = match owner {
            Some((k, _)) => format!("components[{k}]"),
            None => format!("controls[{i}]"),
        };
        let outputs = c.signal_names();
        for s in &outputs {
            signal_at.entry(s.clone()).or_insert_with(|| {
                signals.push(SignalTopology {
                    name: s.clone(),
                    from: Some(c.name().to_string()),
                    to: Vec::new(),
                });
                signals.len() - 1
            });
        }
        let measures = match c {
            Control::Measurement(m) => serde_json::to_value(&m.quantity).ok(),
            Control::ThreePhaseMeasurement(m) => serde_json::to_value(&m.quantity).ok(),
            _ => None,
        };
        controls.push(ControlTopology {
            name: c.name().to_string(),
            type_name: c.type_name(),
            inputs: c
                .inputs()
                .into_iter()
                .map(|(field, signal)| SignalUse {
                    path: match owner {
                        Some(_) => path.clone(),
                        None => format!("{path}.{field}"),
                    },
                    field,
                    signal: signal.to_string(),
                })
                .collect(),
            owner: owner.map(|(_, c)| c.name().to_string()),
            outputs,
            measures,
            path,
        });
    }
    // What reads the signals. A name that no block gives is listed too.
    let mut read = |signal: &str, end: SignalEnd| {
        let k = *signal_at.entry(signal.to_string()).or_insert_with(|| {
            signals.push(SignalTopology {
                name: signal.to_string(),
                from: None,
                to: Vec::new(),
            });
            signals.len() - 1
        });
        signals[k].to.push(end);
    };
    for c in &controls {
        for input in &c.inputs {
            read(
                &input.signal,
                SignalEnd {
                    control: Some(c.name.clone()),
                    component: None,
                    field: Some(input.field.clone()),
                    output: None,
                },
            );
        }
    }
    for c in &components {
        for s in &c.signals {
            read(
                &s.signal,
                SignalEnd {
                    control: None,
                    component: Some(c.name.clone()),
                    field: Some(s.field.clone()),
                    output: None,
                },
            );
        }
    }
    for (i, o) in model.outputs.iter().flatten().enumerate() {
        if let Output::Signal { signal } = o {
            read(
                signal,
                SignalEnd {
                    control: None,
                    component: None,
                    field: None,
                    output: Some(format!("outputs[{i}]")),
                },
            );
        }
    }

    let mut t = Topology {
        components,
        nodes,
        buses,
        controls,
        signals,
        diagnostics: Vec::new(),
    };
    let by = |severity| diagnostics.iter().filter(move |d| d.severity == severity);
    t.diagnostics = by(Severity::Error)
        .chain(by(Severity::Warning))
        .map(|d| mark(model, &t, d))
        .collect();
    t
}

/// What the diagnostic `d` is about: what its path names, and what it says
/// itself.
fn mark(model: &Model, t: &Topology, d: &Diagnostic) -> Mark {
    let mut components: Vec<String> = Vec::new();
    let mut controls: Vec<String> = Vec::new();
    let mut nodes: Vec<String> = Vec::new();
    let mut buses: Vec<String> = Vec::new();
    if let Some((i, rest)) = indexed(&d.path, "components") {
        if let Some(c) = t.components.get(i) {
            components.push(c.name.clone());
            // A terminal: "nodes[0]", or "windings[0]" for both of its ends.
            for terminal in &c.terminals {
                let field = terminal.name.as_str();
                let named = !rest.is_empty()
                    && (rest == field
                        || field.strip_prefix(rest).is_some_and(|r| r.starts_with('['))
                        || rest.strip_prefix(field).is_some_and(|r| r.starts_with('[')));
                if named {
                    nodes.extend(terminal.nodes.iter().cloned());
                    buses.extend(terminal.bus.iter().cloned());
                }
            }
        }
    } else if let Some((i, _)) = indexed(&d.path, "controls") {
        controls.extend(model.controls.get(i).map(|c| c.name().to_string()));
    } else if let Some((i, _)) = indexed(&d.path, "outputs") {
        match model.outputs.as_ref().and_then(|o| o.get(i)) {
            Some(Output::NodeVoltage { node }) => nodes.push(node.clone()),
            Some(Output::BranchCurrent { component } | Output::BranchVoltage { component }) => {
                components.push(component.clone())
            }
            Some(Output::Signal { signal }) => {
                let from = t.signals.iter().find(|s| &s.name == signal);
                controls.extend(from.and_then(|s| s.from.clone()));
            }
            Some(Output::BusVoltage { bus }) => buses.push(bus.clone()),
            None => {}
        }
    } else if let Some(scan) = &model.frequency_scan {
        if let Some((i, _)) = indexed(&d.path, "frequency_scan.nodes") {
            nodes.extend(scan.nodes.get(i).cloned());
        } else if let Some((i, _)) = indexed(&d.path, "frequency_scan.buses") {
            buses.extend(scan.buses.get(i).cloned());
        }
    }
    for subject in &d.about {
        match subject {
            Subject::Component(name) => components.push(name.clone()),
            Subject::Node(name) => nodes.push(name.clone()),
        }
    }
    // A block of a component is marked at the component.
    for name in std::mem::take(&mut controls) {
        if let Some(c) = t.controls.iter().find(|c| c.name == name) {
            match &c.owner {
                Some(owner) => components.push(owner.clone()),
                None => controls.push(name),
            }
        }
    }
    // Only what the document has, each once, in the order found.
    let known = |list: Vec<String>, has: &dyn Fn(&str) -> bool| {
        let mut seen = BTreeSet::new();
        list.into_iter()
            .filter(|n| has(n) && seen.insert(n.clone()))
            .collect::<Vec<_>>()
    };
    Mark {
        severity: d.severity,
        code: d.code,
        path: d.path.clone(),
        components: known(components, &|n| t.components.iter().any(|c| c.name == n)),
        controls: known(controls, &|n| t.controls.iter().any(|c| c.name == n)),
        nodes: known(nodes, &|n| t.nodes.iter().any(|x| x.name == n)),
        buses: known(buses, &|n| t.buses.iter().any(|b| b.name == n)),
    }
}
