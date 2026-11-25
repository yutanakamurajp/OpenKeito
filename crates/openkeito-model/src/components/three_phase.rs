//! What the three-phase components share: buses, phases, phase matrices and
//! the checks of their terminals.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::check::{self, same_node as same};
use crate::diagnostic::Diagnostic;
use crate::model::GROUND;

/// A 3x3 phase matrix, rows and columns in the order a, b, c.
pub type PhaseMatrix = [[f64; 3]; 3];

/// Names of the phases, in the order of the phase nodes of a bus.
pub const PHASES: [&str; 3] = ["a", "b", "c"];

/// One terminal of a three-phase component: a bus name, which stands for the
/// nodes "<bus>.a", "<bus>.b" and "<bus>.c" ("gnd" stands for gnd three
/// times), or the three nodes [a, b, c] written out, for example to connect
/// the phases to a star point ["n", "n", "n"] or in another order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    untagged,
    expecting = "a bus name string or an array of three node names [a, b, c]"
)]
pub enum BusRef {
    Bus(String),
    Nodes([String; 3]),
}

impl BusRef {
    /// The three phase nodes.
    pub fn nodes(&self) -> [String; 3] {
        match self {
            BusRef::Bus(b) if b == GROUND => [GROUND.into(), GROUND.into(), GROUND.into()],
            BusRef::Bus(b) => PHASES.map(|p| format!("{b}.{p}")),
            BusRef::Nodes(n) => n.clone(),
        }
    }
}

/// A phase of a bus.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    A,
    B,
    C,
}

impl Phase {
    /// 0, 1 or 2.
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn as_str(self) -> &'static str {
        PHASES[self.index()]
    }
}

/// Series impedance of three coupled phases: v = R i + L di/dt with 3x3
/// matrices R and L. Written either with the positive- and zero-sequence
/// values of a balanced (transposed) branch, which give the self values
/// (Z0 + 2 Z1)/3 and the mutual values (Z0 - Z1)/3, or with the matrices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Impedance {
    /// Positive- and zero-sequence values. Resistances >= 0, inductances > 0.
    Sequence {
        r1_ohm: f64,
        l1_h: f64,
        r0_ohm: f64,
        l0_h: f64,
    },
    /// Phase matrices, rows and columns in the order a, b, c. Both must be
    /// symmetric; L positive definite, R positive semidefinite.
    Matrix {
        resistance_ohm: [[f64; 3]; 3],
        inductance_h: [[f64; 3]; 3],
    },
}

/// Matrix [[s, m, m], [m, s, m], [m, m, s]] of a balanced branch with
/// positive-sequence value `x1` and zero-sequence value `x0`.
pub fn sequence_matrix(x1: f64, x0: f64) -> PhaseMatrix {
    let s = (x0 + 2.0 * x1) / 3.0;
    let m = (x0 - x1) / 3.0;
    [[s, m, m], [m, s, m], [m, m, s]]
}

impl Impedance {
    /// (R, L) phase matrices in ohm and henry.
    pub fn matrices(&self) -> (PhaseMatrix, PhaseMatrix) {
        match self {
            Impedance::Sequence {
                r1_ohm,
                l1_h,
                r0_ohm,
                l0_h,
            } => (
                sequence_matrix(*r1_ohm, *r0_ohm),
                sequence_matrix(*l1_h, *l0_h),
            ),
            Impedance::Matrix {
                resistance_ohm,
                inductance_h,
            } => (*resistance_ohm, *inductance_h),
        }
    }
}

/// Default of a `neutral` field.
pub(crate) fn ground() -> String {
    GROUND.to_string()
}

/// One terminal of a component as the model writes it: a node, or the three
/// phases of a bus.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Terminal {
    /// The field of the component that names it: "nodes[0]", "bus",
    /// "buses[1]", "windings[0][1]", "neutral".
    pub name: String,
    /// The bus of a three-phase terminal: the name written, or the bus whose
    /// phase nodes "<bus>.a", "<bus>.b" and "<bus>.c" are written out.
    /// Absent for a single node and for three nodes that are not one bus.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bus: Option<String>,
    /// The phases of `nodes`, for a three-phase terminal: "a", "b", "c", or
    /// those of a fault.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phases: Option<Vec<&'static str>>,
    /// The nodes: one, or one per phase.
    pub nodes: Vec<String>,
}

impl Terminal {
    /// A terminal at one node.
    pub(crate) fn node(field: impl Into<String>, node: &str) -> Terminal {
        Terminal {
            name: field.into(),
            bus: None,
            phases: None,
            nodes: vec![node.to_string()],
        }
    }

    /// A three-phase terminal.
    pub(crate) fn bus(field: impl Into<String>, bus: &BusRef) -> Terminal {
        let nodes = bus.nodes();
        let name = match bus {
            BusRef::Bus(b) if b != GROUND => Some(b.clone()),
            BusRef::Bus(_) => None,
            // The phase nodes of one bus, written out.
            BusRef::Nodes(n) => n[0]
                .strip_suffix(".a")
                .filter(|b| !b.is_empty() && n[1] == format!("{b}.b") && n[2] == format!("{b}.c"))
                .map(str::to_owned),
        };
        Terminal {
            name: field.into(),
            bus: name,
            phases: Some(PHASES.to_vec()),
            nodes: nodes.to_vec(),
        }
    }

    /// The same terminal with only the phases `phases` (0, 1, 2), in their
    /// order.
    pub(crate) fn only(mut self, phases: &[usize]) -> Terminal {
        self.nodes = phases.iter().map(|&p| self.nodes[p].clone()).collect();
        self.phases = Some(phases.iter().map(|&p| PHASES[p]).collect());
        self
    }
}

/// The terminals of a component between `bus` and a star point `neutral`.
pub(crate) fn bus_and_neutral_terminals(bus: &BusRef, neutral: &str) -> Vec<Terminal> {
    vec![
        Terminal::bus("bus", bus),
        Terminal::node("neutral", neutral),
    ]
}

/// The terminals of a component from buses[0] to buses[1].
pub(crate) fn series_terminals(buses: &[BusRef; 2]) -> Vec<Terminal> {
    (0..2)
        .map(|k| Terminal::bus(format!("buses[{k}]"), &buses[k]))
        .collect()
}

/// The pairs (buses[0].p, buses[1].p) of the three phases.
pub(crate) fn phase_pairs(buses: &[BusRef; 2]) -> Vec<(String, String)> {
    buses[0].nodes().into_iter().zip(buses[1].nodes()).collect()
}

/// The pairs (bus.p, neutral) of the three phases.
pub(crate) fn neutral_pairs(bus: &BusRef, neutral: &str) -> Vec<(String, String)> {
    bus.nodes()
        .into_iter()
        .map(|n| (n, neutral.to_string()))
        .collect()
}

/// `bus` and the star point `neutral` of every phase.
pub(crate) fn bus_and_neutral(bus: &BusRef, neutral: &str) -> Vec<String> {
    let mut v = Vec::from(bus.nodes());
    v.push(neutral.to_string());
    v
}

/// Terminals of a component between `bus` and a star point `neutral`.
pub(crate) fn check_bus_and_neutral(
    name: &str,
    bus: &BusRef,
    neutral: &String,
    p: &str,
    d: &mut Vec<Diagnostic>,
) {
    check::bus_ref(d, bus, format!("{p}.bus"));
    check::node_name(d, neutral, format!("{p}.neutral"));
    if bus.nodes().contains(neutral) {
        check::same_node(
            d,
            format!("{p}.neutral"),
            format!("a phase of \"{name}\" is connected to its neutral \"{neutral}\""),
        );
    }
}

/// Terminals of a component with one series branch per phase from buses[0]
/// to buses[1].
pub(crate) fn check_series_buses(
    name: &str,
    buses: &[BusRef; 2],
    p: &str,
    d: &mut Vec<Diagnostic>,
) {
    for (k, b) in buses.iter().enumerate() {
        check::bus_ref(d, b, format!("{p}.buses[{k}]"));
    }
    let [from, to] = [buses[0].nodes(), buses[1].nodes()];
    let shorted: Vec<String> = (0..3)
        .filter(|&k| from[k] == to[k])
        .map(|k| format!("{} at \"{}\"", PHASES[k], from[k]))
        .collect();
    if !shorted.is_empty() {
        same(
            d,
            format!("{p}.buses"),
            format!(
                "phases of \"{name}\" with both ends at one node: {}",
                shorted.join(", ")
            ),
        );
    }
}
