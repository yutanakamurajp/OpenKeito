//! Circuit components, one module per family.
//!
//! A component type is a structure in the module of its family with an
//! implementation of [`ComponentSpec`]: its terminals, the checks of its
//! parameters, the format version that added it and its catalog entry. To
//! add a type, write both in the family module (or a new module) and add one
//! line to `component_types!` below. Validation, the catalog and the JSON
//! Schema follow from that line.

mod converters;
mod lines;
mod loads;
mod machines;
mod nonlinear;
mod passive;
mod sources;
mod switches;
mod three_phase;
mod transformers;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use converters::*;
pub use lines::*;
pub use loads::*;
pub use machines::*;
pub use nonlinear::*;
pub use passive::*;
pub use sources::*;
pub use switches::*;
pub use three_phase::*;
pub use transformers::*;

use crate::catalog::ComponentInfo;
use crate::check;
use crate::diagnostic::Diagnostic;
use crate::model::Simulation;

/// Whether a component participates in the t = 0 steady-state phasor solution
/// when `simulation.initialization` is `"steady_state"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComponentInitialization {
    /// Included in the phasor solution; initial states follow from it (default).
    #[default]
    Include,
    /// Excluded from the phasor solution (from version 0.7): open-circuited in
    /// the phasor solve, starts from zero state (or its own initial fields), and
    /// enters the circuit from the first time step.
    Exclude,
}

impl ComponentInitialization {
    pub fn is_include(&self) -> bool {
        *self == ComponentInitialization::Include
    }
}

/// What a component is to a sinusoidal steady state (`initialization`
/// "steady_state").
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SteadyState {
    /// No source, and linear: it has a phasor stamp.
    Linear,
    /// A sine source of this frequency in Hz.
    Sine(f64),
    /// A source that is not a sine without offset (E180); the field that
    /// holds its waveform.
    NotSine(&'static str),
    /// Its state follows from the solution, so it has no phasor (E182).
    NoPhasor,
}

/// What a component is to the equations of a time step between two or more
/// of its nodes, for the checks of loops of ideal branches (E142, W142) and
/// of nodes whose voltage nothing holds (E143, W143). A node that only the
/// component knows (the point behind an impedance) has a name with a ':',
/// which no node of a model has.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Branch {
    /// A conductance between the two nodes (a resistor, or the conductance
    /// an inductor, a capacitor or a line has within a time step): it holds
    /// the voltage of one node when the other is held, and is never part of
    /// a loop of ideal branches.
    Conductance(String, String),
    /// An ideal branch: an equation gives V(first) - V(second) and the
    /// branch current is an unknown (voltage source, closed switch).
    Voltage(String, String),
    /// An ideal transformer: one equation, the sum of coefficient * V(node)
    /// = 0, with a current as unknown.
    Constraint(Vec<(String, f64)>),
}

/// What validation and the catalog need to know about a component type.
///
/// The defaults are those of a component with `nodes` [from, to]: such a
/// type gives `two_nodes` and its parameters. Every other type gives
/// `node_names`, `check_terminals` and `connections` too.
pub(crate) trait ComponentSpec {
    fn name(&self) -> &str;

    /// Value of the `type` field.
    fn type_name(&self) -> &'static str;

    /// Format version this component needs: the version that added the type,
    /// or a later one when it uses a field that was added later.
    fn since(&self) -> &'static str;

    /// `nodes` of a single-phase component with two terminals.
    fn two_nodes(&self) -> Option<&[String; 2]> {
        None
    }

    /// Every node the component connects to, buses expanded to their phase
    /// nodes, in a fixed order (first appearance decides the numbering of
    /// the unknowns).
    fn node_names(&self) -> Vec<String> {
        self.two_nodes().expect("two-terminal component").to_vec()
    }

    /// The terminals as the model writes them, in the order of `node_names`
    /// and with the same nodes (a test compares the two for every type).
    fn terminals(&self) -> Vec<Terminal> {
        let nodes = self.two_nodes().expect("two-terminal component");
        vec![
            Terminal::node("nodes[0]", &nodes[0]),
            Terminal::node("nodes[1]", &nodes[1]),
        ]
    }

    /// Node names written in the component: valid names, buses that can be
    /// expanded, and no element connected to the same node at both ends.
    fn check_terminals(&self, p: &str, d: &mut Vec<Diagnostic>) {
        let nodes = self.two_nodes().expect("two-terminal component");
        check::two_terminals(self.name(), nodes, p, d);
    }

    /// Ranges and consistency of the parameters. `p` is the path of the
    /// component, such as "components[2]".
    fn check_parameters(&self, p: &str, d: &mut Vec<Diagnostic>);

    /// Checks that need the conditions of the run (the time step). Called
    /// after `check_parameters` of every component, when `time_step_s` is
    /// valid.
    fn check_simulation(&self, _sim: &Simulation, _p: &str, _d: &mut Vec<Diagnostic>) {}

    /// Pairs of nodes that the component connects through a conducting
    /// path, for the check that every node reaches gnd (E141).
    fn connections(&self) -> Vec<(String, String)> {
        let [a, b] = self.two_nodes().expect("two-terminal component");
        vec![(a.clone(), b.clone())]
    }

    /// What the component is between its nodes at time `t` (the contacts
    /// that act by time in the state they are commanded to at `t`), as the
    /// solver builds it. The default holds every pair of `connections`
    /// through a conductance: a type that does not say is never part of a
    /// loop of ideal branches and holds its nodes, so that it is never the
    /// cause of E142 or E143.
    fn branches(&self, _t: f64) -> Vec<Branch> {
        let held = |(a, b)| Branch::Conductance(a, b);
        self.connections().into_iter().map(held).collect()
    }

    /// The times at which `branches` changes.
    fn switching_times(&self) -> Vec<f64> {
        Vec::new()
    }

    /// The suffixes of the columns of its currents, in column order: "" for
    /// `I(<name>)`, ".a" for `I(<name>.a)`, ".0.a" for `I(<name>.0.a)`.
    /// They are what `terminal` and `phase` of a measurement choose from, and
    /// must be the currents the solver builds for the component (a test of
    /// openkeito-core compares them for every type). `None` when the type
    /// does not say: a choice that names no current is then found only when
    /// the circuit is built.
    fn current_suffixes(&self) -> Option<Vec<String>> {
        self.two_nodes().map(|_| vec![String::new()])
    }

    /// True for a component with one branch voltage per phase:
    /// V(buses[0].p) - V(buses[1].p), or V(nodes[0]) - V(nodes[1]).
    fn has_branch_voltage(&self) -> bool {
        true
    }

    /// True when the component connects its nodes to gnd without naming it
    /// (the shunt capacitances of a line).
    fn grounds_implicitly(&self) -> bool {
        false
    }

    /// True when the component has windings that are not connected to each
    /// other (for the hint of E141).
    fn has_separate_windings(&self) -> bool {
        false
    }

    /// Series resistance and inductance matrices that the solver inverts as
    /// R + alpha L, with the field they are written in (for E139).
    fn series_impedance(&self) -> Option<(PhaseMatrix, PhaseMatrix, &'static str)> {
        None
    }

    /// All series resistance and inductance matrices that the solver inverts
    /// (including scheduled changes), each with its field path.
    fn series_impedances(&self) -> Vec<(PhaseMatrix, PhaseMatrix, String)> {
        self.series_impedance()
            .into_iter()
            .map(|(r, l, f)| (r, l, f.to_string()))
            .collect()
    }

    /// Control signals the component reads, each with the field it is
    /// written in.
    fn signals(&self) -> Vec<(&'static str, &str)> {
        Vec::new()
    }

    /// Names of the internal quantities the component offers as signals
    /// (the speed of a machine); a measurement block reads them as
    /// `component_signal`. They are the `signal_names` of its element in the
    /// solver.
    fn signal_names(&self) -> Vec<&'static str> {
        Vec::new()
    }

    fn steady_state(&self) -> SteadyState {
        SteadyState::Linear
    }

    /// Frequency of a source with a sine waveform (also one with an offset).
    fn sine_frequency(&self) -> Option<f64> {
        None
    }

    /// True when a controlled source has no `steady_state` phasor (W240).
    fn lacks_steady_state_phasor(&self) -> bool {
        false
    }

    /// The catalog entry of the type.
    fn info() -> ComponentInfo
    where
        Self: Sized;
}

/// The column suffixes of the three phases at each of `prefixes`: ".a" ...
/// with [""], ".0.a" ... ".1.c" with [".0", ".1"].
pub(crate) fn phase_suffixes(prefixes: &[&str]) -> Vec<String> {
    let phases = |t| PHASES.iter().map(move |ph| format!("{t}.{ph}"));
    prefixes.iter().copied().flat_map(phases).collect()
}

/// Declares the component types: the variants of `Component` (the structure
/// of each has the name of its variant), in the order of the catalog.
macro_rules! component_types {
    ($($variant:ident),* $(,)?) => {
/// A circuit component.
///
/// Single-phase components have `nodes` [from, to]; a positive branch
/// current flows through the component from `nodes[0]` to `nodes[1]`, and the
/// branch voltage is V(nodes[0]) - V(nodes[1]). Three-phase components connect
/// to buses: bus "b1" stands for the nodes "b1.a", "b1.b" and "b1.c".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Component {
            $($variant($variant)),*
        }

        impl Component {
            /// The component as an implementation of `ComponentSpec`.
            pub(crate) fn spec(&self) -> &dyn ComponentSpec {
                match self {
                    $(Component::$variant(c) => c),*
                }
            }

            /// Whether this component participates in the steady-state phasor solution.
            pub fn initialization(&self) -> ComponentInitialization {
                match self {
                    $(Component::$variant(c) => c.initialization),*
                }
            }
        }

        /// The catalog entries of every component type.
        pub(crate) fn catalog() -> Vec<ComponentInfo> {
            let mut entries = vec![$(<$variant as ComponentSpec>::info()),*];
            for info in &mut entries {
                info.parameters.push(crate::catalog::p(
                    "initialization",
                    "",
                    false,
                    "Participation in simulation.initialization=\"steady_state\": \"include\" (default) or \"exclude\" (version 0.7: omitted from phasor solution, starts from rest at t = 0).",
                ));
            }
            entries
        }
    };
}

component_types! {
    Resistor,
    Inductor,
    Capacitor,
    VoltageSource,
    CurrentSource,
    Switch,
    Diode,
    ControlledSwitch,
    Thyristor,
    ThreePhaseSource,
    CoupledRl,
    PiLine,
    Transformer,
    ThreePhaseTransformer,
    Fault,
    ThreePhaseLoad,
    ThreePhaseCurrentSource,
    ThreePhaseIdealTransformer,
    NonlinearResistor,
    NonlinearInductor,
    BergeronLine,
    ControlledVoltageSource,
    ControlledCurrentSource,
    ThreePhaseControlledSource,
    ThreePhaseControlledCurrentSource,
    SynchronousMachine,
    GridFollowingConverter,
}

impl Component {
    pub fn name(&self) -> &str {
        self.spec().name()
    }

    /// `nodes` of a single-phase component with two terminals; `None` for
    /// the transformer and the three-phase components.
    pub fn two_nodes(&self) -> Option<&[String; 2]> {
        self.spec().two_nodes()
    }

    /// Every node the component connects to, buses expanded to their phase
    /// nodes, in a fixed order (first appearance decides the numbering of
    /// the unknowns).
    pub fn node_names(&self) -> Vec<String> {
        self.spec().node_names()
    }

    pub fn type_name(&self) -> &'static str {
        self.spec().type_name()
    }

    /// The terminals as the model writes them: each with the field that
    /// names it and its nodes, in the order of `node_names`.
    pub fn terminals(&self) -> Vec<Terminal> {
        self.spec().terminals()
    }

    /// Control signals the component reads, each with the field it is
    /// written in.
    pub fn signals(&self) -> Vec<(&'static str, &str)> {
        self.spec().signals()
    }

    /// The suffixes of the columns of its currents (`I(<name><suffix>)`), in
    /// column order; `None` when the type does not say.
    pub fn current_suffixes(&self) -> Option<Vec<String>> {
        self.spec().current_suffixes()
    }

    /// The suffixes of the columns of its branch voltages
    /// (`U(<name><suffix>)`); empty for a component without them.
    pub fn voltage_suffixes(&self) -> Vec<String> {
        if !self.spec().has_branch_voltage() {
            Vec::new()
        } else if self.two_nodes().is_some() {
            vec![String::new()]
        } else {
            phase_suffixes(&[""])
        }
    }

    /// Names of the internal quantities the component offers as signals.
    pub fn signal_names(&self) -> Vec<&'static str> {
        self.spec().signal_names()
    }
}
