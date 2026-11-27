//! Dependent sources: the voltage-controlled voltage and current sources
//! and the current-controlled current and voltage sources (format 0.10).
//!
//! They are the controlled sources of modified nodal analysis (Ho, Ruehli
//! and Brennan 1975; Vlach and Singhal 1993): a constant gain relates the
//! output to a voltage or a current of the circuit within the same time
//! step. The controlled sources whose value is a control signal
//! (`controlled_voltage_source`, `controlled_current_source`) follow their
//! signal one time step late instead.
//!
//! Every one has two ports of two nodes each. The output current flows from
//! `output[0]` through the source to `output[1]`. The controlling voltage is
//! V(control[0]) - V(control[1]); the control port draws no current. The
//! controlling current is the current from `sense[0]` through the sense port
//! (an ideal branch of 0 V, an ammeter) to `sense[1]`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Branch, ComponentInitialization, ComponentSpec, Terminal};
use crate::catalog::{self, p, ComponentInfo};
use crate::check::{node_name, range_error, same_node};
use crate::diagnostic::Diagnostic;

/// The kind of a dependent source, for the checks of its equations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DependentKind {
    /// V(output) = gain V(control).
    Vcvs,
    /// I(output) = gain V(control).
    Vccs,
    /// I(output) = gain I(sense).
    Cccs,
    /// V(output) = gain I(sense).
    Ccvs,
}

/// Voltage-controlled voltage source:
/// V(output[0]) - V(output[1]) = gain_v_per_v (V(control[0]) - V(control[1])).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Vcvs {
    pub name: String,
    /// The nodes whose voltage V(control[0]) - V(control[1]) controls the
    /// source. The control port draws no current.
    pub control: [String; 2],
    /// [+, -] of the output. Its current I(<name>) flows from output[0]
    /// through the source to output[1].
    pub output: [String; 2],
    /// Voltage gain, V/V, finite (0 is a short circuit of the output).
    pub gain_v_per_v: f64,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

/// Voltage-controlled current source: the current
/// transconductance_s (V(control[0]) - V(control[1])) flows from output[0]
/// through the source to output[1], that is, into output[1].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Vccs {
    pub name: String,
    /// The nodes whose voltage V(control[0]) - V(control[1]) controls the
    /// source. The control port draws no current.
    pub control: [String; 2],
    /// [from, to] of the output current.
    pub output: [String; 2],
    /// Transconductance in siemens, finite (0 is an open output).
    pub transconductance_s: f64,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

/// Current-controlled current source: the current gain_a_per_a I(sense)
/// flows from output[0] through the source to output[1]. The sense port is
/// an ideal branch of 0 V between sense[0] and sense[1], and I(sense) is
/// its current from sense[0] to sense[1].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Cccs {
    pub name: String,
    /// [from, to] of the sense port (0 V), which measures the current that
    /// flows through it, I(<name>.sense).
    pub sense: [String; 2],
    /// [from, to] of the output current I(<name>).
    pub output: [String; 2],
    /// Current gain, A/A, finite (0 is an open output).
    pub gain_a_per_a: f64,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

/// Current-controlled voltage source:
/// V(output[0]) - V(output[1]) = transresistance_ohm I(sense), with the sense
/// port of a `cccs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Ccvs {
    pub name: String,
    /// [from, to] of the sense port (0 V), which measures the current that
    /// flows through it, I(<name>.sense).
    pub sense: [String; 2],
    /// [+, -] of the output. Its current I(<name>) flows from output[0]
    /// through the source to output[1].
    pub output: [String; 2],
    /// Transresistance in ohm, finite (0 is a short circuit of the output).
    pub transresistance_ohm: f64,
    /// Whether this component participates in `simulation.initialization = "steady_state"`.
    #[serde(default, skip_serializing_if = "ComponentInitialization::is_include")]
    pub initialization: ComponentInitialization,
}

/// The ports of a dependent source as the checks need them.
struct Ports<'a> {
    name: &'a str,
    /// "control" or "sense".
    input_field: &'static str,
    input: &'a [String; 2],
    output: &'a [String; 2],
    gain_field: &'static str,
    gain: f64,
    kind: DependentKind,
}

impl Ports<'_> {
    fn node_names(&self) -> Vec<String> {
        self.input.iter().chain(self.output).cloned().collect()
    }

    fn terminals(&self) -> Vec<Terminal> {
        let mut terminals = Vec::with_capacity(4);
        for (field, nodes) in [(self.input_field, self.input), ("output", self.output)] {
            for (k, n) in nodes.iter().enumerate() {
                terminals.push(Terminal::node(format!("{field}[{k}]"), n));
            }
        }
        terminals
    }

    fn check_terminals(&self, p: &str, d: &mut Vec<Diagnostic>) {
        for (field, nodes) in [(self.input_field, self.input), ("output", self.output)] {
            for (k, n) in nodes.iter().enumerate() {
                node_name(d, n, format!("{p}.{field}[{k}]"));
            }
            if nodes[0] == nodes[1] {
                same_node(
                    d,
                    format!("{p}.{field}"),
                    format!(
                        "both nodes of the {field} port of \"{}\" are node \"{}\"",
                        self.name, nodes[0]
                    ),
                );
            }
        }
    }

    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
        if !self.gain.is_finite() {
            range_error(
                d,
                p,
                self.name,
                self.gain_field,
                self.gain,
                "Use a finite gain. 0 is allowed: a source of voltage is then 0 V, a source of \
                 current an open circuit.",
            );
        }
    }

    /// The output always connects its nodes (a voltage holds them, a current
    /// flows between them); a sense port too. A control port draws no
    /// current and connects nothing.
    fn connections(&self) -> Vec<(String, String)> {
        let pair = |n: &[String; 2]| (n[0].clone(), n[1].clone());
        match self.kind {
            DependentKind::Vcvs | DependentKind::Vccs => vec![pair(self.output)],
            DependentKind::Cccs | DependentKind::Ccvs => {
                vec![pair(self.input), pair(self.output)]
            }
        }
    }

    fn branches(&self) -> Vec<Branch> {
        vec![Branch::Dependent {
            kind: self.kind,
            input: self.input.clone(),
            output: self.output.clone(),
            gain: self.gain,
        }]
    }
}

/// The common parts of the four implementations of `ComponentSpec`.
macro_rules! dependent_spec {
    ($type:ident, $type_name:literal, $input:ident, $gain:ident, $kind:ident) => {
        impl $type {
            fn ports(&self) -> Ports<'_> {
                Ports {
                    name: &self.name,
                    input_field: stringify!($input),
                    input: &self.$input,
                    output: &self.output,
                    gain_field: stringify!($gain),
                    gain: self.$gain,
                    kind: DependentKind::$kind,
                }
            }
        }

        impl ComponentSpec for $type {
            fn name(&self) -> &str {
                &self.name
            }
            fn type_name(&self) -> &'static str {
                $type_name
            }
            fn since(&self) -> &'static str {
                "0.10"
            }
            fn node_names(&self) -> Vec<String> {
                self.ports().node_names()
            }
            fn terminals(&self) -> Vec<Terminal> {
                self.ports().terminals()
            }
            fn check_terminals(&self, p: &str, d: &mut Vec<Diagnostic>) {
                self.ports().check_terminals(p, d);
            }
            fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>) {
                self.ports().check_parameters(p, d);
            }
            fn connections(&self) -> Vec<(String, String)> {
                self.ports().connections()
            }
            fn branches(&self, _t: f64) -> Vec<Branch> {
                self.ports().branches()
            }
            fn current_suffixes(&self) -> Option<Vec<String>> {
                Some(current_suffixes(DependentKind::$kind))
            }
            fn has_branch_voltage(&self) -> bool {
                false
            }
            // The ports are not connected to each other.
            fn has_separate_windings(&self) -> bool {
                true
            }
            fn info() -> ComponentInfo {
                info(DependentKind::$kind)
            }
        }
    };
}

dependent_spec!(Vcvs, "vcvs", control, gain_v_per_v, Vcvs);
dependent_spec!(Vccs, "vccs", control, transconductance_s, Vccs);
dependent_spec!(Cccs, "cccs", sense, gain_a_per_a, Cccs);
dependent_spec!(Ccvs, "ccvs", sense, transresistance_ohm, Ccvs);

/// I(<name>), and I(<name>.sense) of the sources with a sense port.
fn current_suffixes(kind: DependentKind) -> Vec<String> {
    match kind {
        DependentKind::Vcvs | DependentKind::Vccs => vec![String::new()],
        DependentKind::Cccs | DependentKind::Ccvs => vec![String::new(), ".sense".into()],
    }
}

/// The sentence on the time step that ends every summary.
macro_rules! same_step {
    () => {
        " The relation holds within each time step (it is part of the circuit equations), unlike controlled_voltage_source and controlled_current_source, whose value is a control signal of one time step before. The gain is a constant."
    };
}

const CONTROL: &str = "[+, -]: the controlling voltage is V(control[0]) - V(control[1]). The control port draws no current, so it does not connect its nodes to anything.";

const SENSE: &str = "[from, to] of the sense port, an ideal branch of 0 V (an ammeter) placed in the circuit: the controlling current I(<name>.sense) flows from sense[0] through it to sense[1].";

const VOLTAGE_SIGN: &str = "The output current I(<name>) is positive from output[0] through the source to output[1], so a source that delivers power with output[0] positive has a negative current (as a voltage_source).";

const CURRENT_SIGN: &str = "The output current I(<name>) flows from output[0] through the source to output[1], that is, into output[1] (as a current_source).";

fn info(kind: DependentKind) -> ComponentInfo {
    let gain_p = |field, unit, text| p(field, unit, true, text);
    let (type_name, summary, sign_convention, input, gain, example) = match kind {
        DependentKind::Vcvs => (
            "vcvs",
            concat!("Voltage-controlled voltage source (modified nodal analysis): V(output[0]) - V(output[1]) = gain_v_per_v (V(control[0]) - V(control[1])) at every instant.", same_step!()),
            VOLTAGE_SIGN,
            p("control", "", true, CONTROL),
            gain_p("gain_v_per_v", "V/V", "Voltage gain, finite. 0: the output is 0 V (a short circuit)."),
            json!({"type": "vcvs", "name": "E1", "control": ["n1", "gnd"], "output": ["n2", "gnd"], "gain_v_per_v": 10.0}),
        ),
        DependentKind::Vccs => (
            "vccs",
            concat!("Voltage-controlled current source (modified nodal analysis): the current transconductance_s (V(control[0]) - V(control[1])) flows from output[0] through the source to output[1] at every instant.", same_step!()),
            CURRENT_SIGN,
            p("control", "", true, CONTROL),
            gain_p("transconductance_s", "S", "Transconductance, finite. 0: the output is open."),
            json!({"type": "vccs", "name": "G1", "control": ["n1", "gnd"], "output": ["gnd", "n2"], "transconductance_s": 0.01}),
        ),
        DependentKind::Cccs => (
            "cccs",
            concat!("Current-controlled current source (modified nodal analysis): the current gain_a_per_a I(<name>.sense) flows from output[0] through the source to output[1] at every instant; I(<name>.sense) is the current through its own sense port of 0 V.", same_step!()),
            "The output current I(<name>) flows from output[0] through the source to output[1], that is, into output[1] (as a current_source). The sense current I(<name>.sense) flows from sense[0] through the sense port to sense[1].",
            p("sense", "", true, SENSE),
            gain_p("gain_a_per_a", "A/A", "Current gain, finite. 0: the output is open."),
            json!({"type": "cccs", "name": "F1", "sense": ["n1", "n2"], "output": ["gnd", "n3"], "gain_a_per_a": 2.0}),
        ),
        DependentKind::Ccvs => (
            "ccvs",
            concat!("Current-controlled voltage source (modified nodal analysis): V(output[0]) - V(output[1]) = transresistance_ohm I(<name>.sense) at every instant; I(<name>.sense) is the current through its own sense port of 0 V.", same_step!()),
            "The output current I(<name>) is positive from output[0] through the source to output[1] (as a voltage_source). The sense current I(<name>.sense) flows from sense[0] through the sense port to sense[1].",
            p("sense", "", true, SENSE),
            gain_p("transresistance_ohm", "ohm", "Transresistance, finite. 0: the output is 0 V (a short circuit)."),
            json!({"type": "ccvs", "name": "H1", "sense": ["n1", "n2"], "output": ["n3", "gnd"], "transresistance_ohm": 50.0}),
        ),
    };
    ComponentInfo {
        type_name,
        category: "circuit",
        summary,
        sign_convention,
        parameters: vec![
            catalog::name(),
            input,
            p("output", "", true, "[first, second] nodes of the output."),
            gain,
        ],
        example,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summaries_end_with_the_sentence_on_the_time_step() {
        for kind in [
            DependentKind::Vcvs,
            DependentKind::Vccs,
            DependentKind::Cccs,
            DependentKind::Ccvs,
        ] {
            assert!(info(kind).summary.ends_with(same_step!()));
        }
    }
}
