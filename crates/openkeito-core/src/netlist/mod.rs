//! Expansion of the model's components into solver elements.
//!
//! Single-phase components become one element each. Three-phase components
//! and transformers become several elements with inner nodes, which are
//! numbered but have no name in the model (their labels use ':' so that they
//! cannot be confused with node names). Every recorded current or voltage of
//! a component is a linear combination of unknowns and element currents.
//!
//! The expansions live in one module per family of components. A new
//! component gets a function there (or in a new module) and one line in the
//! list of `build`.

mod converters;
mod dependent;
mod lines;
mod loads;
mod machines;
mod nonlinear;
mod passive;
mod sources;
mod switches;
mod transformers;

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;

use openkeito_model::{self as model, BusRef, Component, GROUND, PHASES};

use crate::element::{Element, Mat3, Node};
use crate::linalg::Solver;

/// One term of a recorded quantity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Term {
    /// coefficient * unknown.
    Unknown(usize, f64),
    /// coefficient * current k of element e.
    Current(usize, usize, f64),
}

/// A current or voltage of a component: the suffix of its column name (""
/// for a single-phase component, ".a", ".0.a", ".g" ...) and its terms.
pub(crate) struct Quantity {
    pub suffix: String,
    pub terms: Vec<Term>,
}

/// What a component is to the outputs.
pub(crate) struct Expansion {
    /// Its currents, in column order.
    pub currents: Vec<Quantity>,
    /// Its branch voltages; empty for a component without them.
    pub voltages: Vec<Quantity>,
}

impl Expansion {
    /// A component without branch voltages.
    fn currents(currents: Vec<Quantity>) -> Expansion {
        Expansion {
            currents,
            voltages: Vec::new(),
        }
    }

    /// The single element `e` between the nodes `a` and `b`: its current
    /// I(<name>) and its voltage U(<name>) = V(a) - V(b).
    fn two_terminal(e: usize, a: Node, b: Node) -> Expansion {
        Expansion {
            currents: vec![Quantity {
                suffix: String::new(),
                terms: vec![Term::Current(e, 0, 1.0)],
            }],
            voltages: vec![Quantity {
                suffix: String::new(),
                terms: Builder::difference(a, b),
            }],
        }
    }
}

/// A part of a circuit that is solved on its own: no element joins it to
/// another part but elements with decoupled ends (travelling-wave lines).
/// Its elements and its unknowns lie together, and its elements number
/// their unknowns from the first unknown of the part: an element of the
/// part is given `x[unknowns]` of a solution `x` of the circuit.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PartOf {
    pub elements: Range<usize>,
    pub unknowns: Range<usize>,
}

/// An element with decoupled ends that joins parts. It numbers the nodes of
/// its ends on its own, from 0 (its ports); `ports[k]` is the unknown of the
/// circuit behind port k. The links follow the elements of the parts.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LinkOf {
    pub element: usize,
    pub ports: Vec<usize>,
}

/// The solver's elements and unknowns of a model.
pub(crate) struct Netlist {
    /// The elements of every part, part after part, then the links.
    pub elements: Vec<Box<dyn Element>>,
    /// True for elements whose component has `"initialization": "exclude"`.
    pub excluded_from_steady_state: Vec<bool>,
    /// The parts the circuit is divided into: one, unless the model asks
    /// for the division and has lines that allow it.
    pub parts: Vec<PartOf>,
    pub links: Vec<LinkOf>,
    /// The elements in the order in which the components of the model made
    /// them (the order of `elements` when the circuit is one part).
    pub order: Vec<usize>,
    /// Name of every unknown, for error messages.
    pub labels: Vec<String>,
    /// True for the unknowns that are node voltages (named or inner).
    pub is_voltage: Vec<bool>,
    /// Unknown of every named node except gnd.
    pub node_index: HashMap<String, usize>,
    /// Named nodes in the order in which the components name them (the
    /// order of their unknowns when the circuit is one part).
    pub named_nodes: Vec<String>,
    /// Elements whose state follows from the solution (diodes).
    pub diodes: Vec<usize>,
    /// Every element with two states (diodes, controlled switches and
    /// thyristors).
    pub two_state: Vec<usize>,
    /// Branch currents of every component, in column order.
    pub currents: HashMap<String, Vec<Quantity>>,
    /// Branch voltages of the components that have them.
    pub voltages: HashMap<String, Vec<Quantity>>,
    /// Index of every control signal, in the order of `model::signal_names`.
    pub signal_index: HashMap<String, usize>,
    pub roles: Roles,
    /// For `component_nodes`: of every component, in the order of the model,
    /// the nodes its expansion asked for (gnd too), in the order it asked.
    /// Empty unless the circuit was built for that.
    pub asked: Vec<(String, Vec<String>)>,
}

/// An internal quantity of an element offered as a signal.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ElementSignal {
    /// `<element name>.<signal name>`.
    pub name: String,
    pub element: usize,
    /// Index among the signals of the element.
    pub index: usize,
}

/// What the solver asks only some of the elements for. Found once, so that
/// a run without such elements does no work for them.
#[derive(Debug, Default)]
pub(crate) struct Roles {
    /// Elements with an operating point that follows from the solution of
    /// each stage.
    pub operating_points: Vec<usize>,
    /// Elements that read control signals at the start of a step.
    pub signal_readers: Vec<usize>,
    /// The internal quantities that elements offer as signals.
    pub signals: Vec<ElementSignal>,
}

impl Roles {
    pub fn of(elements: &[Box<dyn Element>]) -> Roles {
        let mut roles = Roles::default();
        for (i, e) in elements.iter().enumerate() {
            if e.has_operating_point() {
                roles.operating_points.push(i);
            }
            if e.reads_signals() {
                roles.signal_readers.push(i);
            }
            for (index, signal) in e.signal_names().into_iter().enumerate() {
                roles.signals.push(ElementSignal {
                    name: format!("{}.{signal}", e.name()),
                    element: i,
                    index,
                });
            }
        }
        roles
    }
}

impl Netlist {
    /// The linear solver for the equations of these elements, for a circuit
    /// that is one part.
    pub fn solver(&self, choice: model::LinearSolver) -> Solver {
        assert_eq!(self.parts.len(), 1, "one solver per part");
        let n = self.labels.len();
        Solver::new(choice.is_sparse(n), n, |s| {
            for e in &self.elements {
                e.structure(s);
            }
        })
    }
}

/// The unknowns of a part while it is built.
#[derive(Default)]
struct PartUnknowns {
    labels: Vec<String>,
    is_voltage: Vec<bool>,
}

/// What the expansions of the components build on.
///
/// The unknowns are numbered within the part of the component being
/// expanded (`current`). A component whose ends lie in different parts (a
/// travelling-wave line when the circuit is divided) is expanded with
/// `ports`: its nodes are then numbered from 0 in the order in which it asks
/// for them, and its element becomes a link.
struct Builder<'a> {
    elements: Vec<Box<dyn Element>>,
    /// The part of every element; `None` for a link.
    element_part: Vec<Option<usize>>,
    /// Whether each pushed element is excluded from steady-state initialization.
    element_excluded: Vec<bool>,
    current_excluded: bool,
    parts: Vec<PartUnknowns>,
    /// The part of the component being expanded.
    current: usize,
    /// Part and unknown within the part of every named node.
    node_index: HashMap<String, (usize, usize)>,
    /// For a component that joins parts: the part and unknown behind every
    /// port handed out so far.
    ports: RefCell<Option<Vec<(usize, usize)>>>,
    /// The links: element and the part and unknown behind each of its ports.
    links: Vec<(usize, Vec<(usize, usize)>)>,
    signal_index: &'a HashMap<String, usize>,
    diodes: Vec<usize>,
    two_state: Vec<usize>,
    /// The nodes the component being expanded has asked for, when they are
    /// recorded (`component_nodes`).
    asked: Option<RefCell<Vec<String>>>,
}

impl Builder<'_> {
    fn node(&self, n: &str) -> Node {
        if let Some(asked) = &self.asked {
            asked.borrow_mut().push(n.to_string());
        }
        if n == GROUND {
            return None;
        }
        let place = self.node_index[n];
        match self.ports.borrow_mut().as_mut() {
            None => {
                debug_assert_eq!(place.0, self.current, "node {n} lies in another part");
                Some(place.1)
            }
            Some(ports) => Some(ports.iter().position(|p| *p == place).unwrap_or_else(|| {
                ports.push(place);
                ports.len() - 1
            })),
        }
    }

    /// The nodes of a component with `nodes` [from, to].
    fn two_nodes(&self, nodes: &[String; 2]) -> (Node, Node) {
        (self.node(&nodes[0]), self.node(&nodes[1]))
    }

    fn bus(&self, b: &BusRef) -> [Node; 3] {
        let n = b.nodes();
        [self.node(&n[0]), self.node(&n[1]), self.node(&n[2])]
    }

    /// A new inner node.
    fn inner(&mut self, label: String) -> Node {
        assert!(
            self.ports.borrow().is_none(),
            "a component that joins parts has no inner nodes"
        );
        let part = &mut self.parts[self.current];
        part.labels.push(label);
        part.is_voltage.push(true);
        Some(part.labels.len() - 1)
    }

    /// Adds an element and its extra unknowns; returns its index.
    fn push(&mut self, mut e: Box<dyn Element>) -> usize {
        let n = e.aux_count();
        let index = self.elements.len();
        if let Some(ports) = self.ports.borrow().as_ref() {
            assert!(
                n == 0 && !e.has_operating_point() && e.conducting().is_none(),
                "an element that joins parts has no unknowns and no states of its own"
            );
            self.links.push((index, ports.clone()));
            self.element_part.push(None);
        } else {
            let part = &mut self.parts[self.current];
            if n > 0 {
                e.set_aux(part.labels.len());
                for k in 0..n {
                    part.labels.push(e.aux_label(k));
                    part.is_voltage.push(false);
                }
            }
            self.element_part.push(Some(self.current));
        }
        self.elements.push(e);
        self.element_excluded.push(self.current_excluded);
        index
    }

    /// Adds an element with two states that is driven by a gate signal.
    fn push_two_state(&mut self, e: Box<dyn Element>) -> usize {
        let index = self.push(e);
        self.two_state.push(index);
        index
    }

    /// Adds an element with two states whose state follows from the
    /// solution.
    fn push_diode(&mut self, e: Box<dyn Element>) -> usize {
        let index = self.push_two_state(e);
        self.diodes.push(index);
        index
    }

    /// Index of the control signal `name`.
    fn signal(&self, name: &str) -> usize {
        self.signal_index[name]
    }

    /// V(a) - V(b), without the terms of gnd.
    fn difference(a: Node, b: Node) -> Vec<Term> {
        a.map(|i| Term::Unknown(i, 1.0))
            .into_iter()
            .chain(b.map(|i| Term::Unknown(i, -1.0)))
            .collect()
    }
}

fn scaled(m: &Mat3, f: f64) -> Mat3 {
    m.map(|row| row.map(|v| v * f))
}

fn one(v: f64) -> Mat3 {
    [[v, 0.0, 0.0], [0.0; 3], [0.0; 3]]
}

fn phase_quantities(e: usize, prefix: &str, sign: f64) -> impl Iterator<Item = Quantity> + '_ {
    PHASES.iter().enumerate().map(move |(p, ph)| Quantity {
        suffix: format!("{prefix}.{ph}"),
        terms: vec![Term::Current(e, p, sign)],
    })
}

/// Phase angles of the phases a, b and c of a balanced set against phase a,
/// in degrees: b lags by 120, c leads by 120.
const PHASE_SHIFT_DEG: [f64; 3] = [0.0, -120.0, 120.0];

/// U(<name>.p) = V(from.p) - V(to.p).
fn phase_differences(from: [Node; 3], to: [Node; 3]) -> Vec<Quantity> {
    PHASES
        .iter()
        .enumerate()
        .map(|(p, ph)| Quantity {
            suffix: format!(".{ph}"),
            terms: Builder::difference(from[p], to[p]),
        })
        .collect()
}

/// The largest number of named nodes of a part that "auto" makes by joining
/// two parts across a line: 16 three-phase buses. A ring of buses with one
/// line each way is solved 3 to 30 % slower on one thread when every bus is
/// a part than when it is one part, and within -8 to +3 % with parts of this
/// size; the gain from threads is the same (docs/performance.md).
const MERGED_PART_NODES: usize = 48;

/// The parts of a model. When the circuit is divided, they are the sets of
/// nodes that components join, where a travelling-wave line joins the nodes
/// of each of its ends but not one end with the other. Islands of a circuit
/// that nothing joins are parts as well. With `merge`, parts that a line
/// joins are put together again while they are small.
struct Division {
    /// Number of parts, at least 1.
    parts: usize,
    /// Part of every named node, in the order in which the components name
    /// them. The parts are numbered in the order of their first nodes.
    nodes: Vec<(String, usize)>,
    /// Part of every component (of its first end for one that joins parts).
    component_part: Vec<usize>,
    /// True for a component whose ends lie in different parts.
    joins: Vec<bool>,
}

impl Division {
    /// `divide`: at the lines; `merge`: small parts are joined again.
    fn of(m: &model::Model, divide: bool, merge: bool) -> Division {
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut names = Vec::new();
        for c in &m.components {
            for n in c.node_names() {
                if n != GROUND && !index.contains_key(&n) {
                    index.insert(n.clone(), names.len());
                    names.push(n);
                }
            }
        }
        // Union-find over the nodes.
        let mut parent: Vec<usize> = (0..names.len()).collect();
        fn root(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        if !divide {
            // One part, also when the circuit has islands.
            return Division {
                parts: 1,
                nodes: names.into_iter().map(|n| (n, 0)).collect(),
                component_part: vec![0; m.components.len()],
                joins: vec![false; m.components.len()],
            };
        }
        let mut join = |nodes: &[String]| {
            let mut first = None;
            for n in nodes.iter().filter(|n| *n != GROUND) {
                let r = root(&mut parent, index[n]);
                match first {
                    None => first = Some(r),
                    Some(f) => parent[r] = f,
                }
            }
        };
        // The nodes of each end of the components with decoupled ends.
        let ends = |c: &Component| match c {
            Component::BergeronLine(l) => {
                Some([l.buses[0].nodes().to_vec(), l.buses[1].nodes().to_vec()])
            }
            _ => None,
        };
        for c in &m.components {
            match ends(c) {
                Some(ends) => ends.iter().for_each(|end| join(end)),
                None => join(&c.node_names()),
            }
        }
        // Small parts that a line joins are solved together: the line then
        // lies within a part like any other element. Every part and every
        // line between parts costs a little in each step, which only pays
        // when the part has some work of its own (docs/performance.md).
        let limit = if merge { MERGED_PART_NODES } else { 0 };
        let mut size = vec![0; names.len()];
        for i in 0..names.len() {
            size[root(&mut parent, i)] += 1;
        }
        for c in &m.components {
            let Some(ends) = ends(c) else { continue };
            let mut roots = ends.iter().map(|end| {
                end.iter()
                    .find(|n| *n != GROUND)
                    .map(|n| root(&mut parent, index[n]))
            });
            if let (Some(Some(a)), Some(Some(b))) = (roots.next(), roots.next()) {
                if a != b && size[a] + size[b] <= limit {
                    parent[b] = a;
                    size[a] += size[b];
                }
            }
        }
        let mut number: HashMap<usize, usize> = HashMap::new();
        let mut nodes = Vec::with_capacity(names.len());
        for (i, n) in names.iter().enumerate() {
            let r = root(&mut parent, i);
            let next = number.len();
            nodes.push((n.clone(), *number.entry(r).or_insert(next)));
        }
        let part_of = |n: &String| nodes[index[n]].1;
        let mut component_part = Vec::with_capacity(m.components.len());
        let mut joins = Vec::with_capacity(m.components.len());
        for c in &m.components {
            let own: Vec<usize> = c
                .node_names()
                .iter()
                .filter(|n| *n != GROUND)
                .map(part_of)
                .collect();
            component_part.push(own.first().copied().unwrap_or(0));
            joins.push(own.iter().any(|p| *p != own[0]));
        }
        Division {
            parts: number.len().max(1),
            nodes,
            component_part,
            joins,
        }
    }
}

/// Builds the elements of a validated model as one part, whatever the model
/// says about its division.
pub(crate) fn build(m: &model::Model) -> Netlist {
    build_parts(m, model::Partitioning::Off, false)
}

/// Builds the elements of a validated model, divided into parts as its
/// `simulation.partitioning` says.
pub(crate) fn build_divided(m: &model::Model) -> Netlist {
    build_parts(m, m.simulation.partitioning(), false)
}

/// What the solver connects a component to: the nodes its expansion into
/// elements asked for, and its branch voltages as differences of node
/// voltages. For checks of what describes the connections of a model
/// (`openkeito topology`) against the circuit that is solved.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentNodes {
    pub name: String,
    /// The nodes in the order in which the expansion asked for them, each
    /// once; "gnd" where it was asked for by name (a component that is
    /// grounded inside does not ask).
    pub nodes: Vec<String>,
    /// Its branch voltages `U(<name><suffix>)`: the suffix and the terms
    /// (node, coefficient); a term of gnd is left out.
    pub voltages: Vec<(String, Vec<(String, f64)>)>,
}

/// The nodes of every component of a validated model as the solver builds
/// the circuit, in the order of the model. Nothing is solved.
pub fn component_nodes(m: &model::Model) -> Vec<ComponentNodes> {
    let netlist = build_parts(m, model::Partitioning::Off, true);
    let mut name_of: HashMap<usize, &str> = HashMap::new();
    for (n, i) in &netlist.node_index {
        name_of.insert(*i, n.as_str());
    }
    netlist
        .asked
        .iter()
        .map(|(name, asked)| {
            let mut nodes: Vec<String> = Vec::new();
            for n in asked {
                if !nodes.contains(n) {
                    nodes.push(n.clone());
                }
            }
            let voltages = netlist.voltages.get(name).map(|quantities| {
                let terms = |q: &Quantity| {
                    q.terms
                        .iter()
                        .map(|t| match t {
                            Term::Unknown(i, c) => {
                                // An unknown that is no named node: an inner
                                // node or a current, by its label.
                                let node = name_of
                                    .get(i)
                                    .map_or_else(|| netlist.labels[*i].clone(), |n| n.to_string());
                                (node, *c)
                            }
                            Term::Current(e, k, c) => (format!("current {k} of element {e}"), *c),
                        })
                        .collect()
                };
                quantities
                    .iter()
                    .map(|q| (q.suffix.clone(), terms(q)))
                    .collect()
            });
            ComponentNodes {
                name: name.clone(),
                nodes,
                voltages: voltages.unwrap_or_default(),
            }
        })
        .collect()
}

fn build_parts(m: &model::Model, partitioning: model::Partitioning, record: bool) -> Netlist {
    // With the control blocks of the converters, whose signals their
    // sources follow.
    let m = &*model::expanded(m);
    let signal_index: HashMap<String, usize> = model::signal_names(&m.controls)
        .into_iter()
        .enumerate()
        .map(|(i, name)| (name, i))
        .collect();
    let division = Division::of(
        m,
        partitioning != model::Partitioning::Off,
        partitioning == model::Partitioning::Auto,
    );
    let mut b = Builder {
        elements: Vec::with_capacity(m.components.len()),
        element_part: Vec::with_capacity(m.components.len()),
        element_excluded: Vec::with_capacity(m.components.len()),
        current_excluded: false,
        parts: (0..division.parts)
            .map(|_| PartUnknowns::default())
            .collect(),
        current: 0,
        node_index: HashMap::new(),
        ports: RefCell::new(None),
        links: Vec::new(),
        signal_index: &signal_index,
        diodes: Vec::new(),
        two_state: Vec::new(),
        asked: record.then(|| RefCell::new(Vec::new())),
    };
    let mut asked = Vec::new();
    let mut named_nodes = Vec::new();
    for (n, part) in &division.nodes {
        let unknowns = &mut b.parts[*part];
        b.node_index
            .insert(n.clone(), (*part, unknowns.labels.len()));
        unknowns.labels.push(format!("V({n})"));
        unknowns.is_voltage.push(true);
        named_nodes.push(n.clone());
    }

    let mut raw_currents: Raw = Vec::new();
    let mut raw_voltages: Raw = Vec::new();
    for (index, c) in m.components.iter().enumerate() {
        b.current = division.component_part[index];
        b.current_excluded = c.initialization() == model::ComponentInitialization::Exclude;
        *b.ports.borrow_mut() = division.joins[index].then(Vec::new);
        // One line per component type.
        let x = match c {
            Component::Resistor(c) => passive::resistor(&mut b, c),
            Component::Inductor(c) => passive::inductor(&mut b, c),
            Component::Capacitor(c) => passive::capacitor(&mut b, c),
            Component::VoltageSource(c) => sources::voltage_source(&mut b, c),
            Component::CurrentSource(c) => sources::current_source(&mut b, c),
            Component::Switch(c) => switches::switch(&mut b, c),
            Component::Diode(c) => switches::diode(&mut b, c),
            Component::ControlledSwitch(c) => switches::controlled_switch(&mut b, c),
            Component::Thyristor(c) => switches::thyristor(&mut b, c),
            Component::ThreePhaseSource(c) => sources::three_phase_source(&mut b, c),
            Component::CoupledRl(c) => passive::coupled_rl(&mut b, c),
            Component::PiLine(c) => lines::pi_line(&mut b, c),
            Component::Transformer(c) => transformers::transformer(&mut b, c),
            Component::ThreePhaseTransformer(c) => transformers::three_phase_transformer(&mut b, c),
            Component::Fault(c) => switches::fault(&mut b, c),
            Component::ThreePhaseLoad(c) => loads::three_phase_load(&mut b, c),
            Component::ThreePhaseCurrentSource(c) => loads::three_phase_current_source(&mut b, c),
            Component::ThreePhaseIdealTransformer(c) => {
                transformers::three_phase_ideal_transformer(&mut b, c)
            }
            Component::NonlinearResistor(c) => nonlinear::nonlinear_resistor(&mut b, c),
            Component::NonlinearInductor(c) => nonlinear::nonlinear_inductor(&mut b, c),
            Component::BergeronLine(c) => lines::bergeron_line(&mut b, c),
            Component::ControlledVoltageSource(c) => sources::controlled_voltage_source(&mut b, c),
            Component::ControlledCurrentSource(c) => sources::controlled_current_source(&mut b, c),
            Component::ThreePhaseControlledSource(c) => {
                sources::three_phase_controlled_source(&mut b, c)
            }
            Component::ThreePhaseControlledCurrentSource(c) => {
                sources::three_phase_controlled_current_source(&mut b, c)
            }
            Component::SynchronousMachine(c) => machines::synchronous_machine(&mut b, c),
            Component::GridFollowingConverter(c) => converters::grid_following_converter(&mut b, c),
            Component::Vcvs(c) => dependent::vcvs(&mut b, c),
            Component::Vccs(c) => dependent::vccs(&mut b, c),
            Component::Cccs(c) => dependent::cccs(&mut b, c),
            Component::Ccvs(c) => dependent::ccvs(&mut b, c),
        };
        let name = c.name().to_string();
        if let Some(a) = &b.asked {
            asked.push((name.clone(), std::mem::take(&mut *a.borrow_mut())));
        }
        // The unknowns of the terms as the component numbered them.
        let ports = b.ports.borrow_mut().take();
        let part = b.current;
        let place = move |i: usize| match &ports {
            Some(ports) => ports[i],
            None => (part, i),
        };
        let placed = |q: Vec<Quantity>| -> Vec<RawQuantity> {
            q.into_iter()
                .map(|q| {
                    let places = q
                        .terms
                        .iter()
                        .filter_map(|t| match t {
                            Term::Unknown(i, _) => Some(place(*i)),
                            Term::Current(..) => None,
                        })
                        .collect();
                    (q.suffix, q.terms, places)
                })
                .collect()
        };
        raw_currents.push((name.clone(), placed(x.currents)));
        if !x.voltages.is_empty() {
            raw_voltages.push((name, placed(x.voltages)));
        }
    }

    // The unknowns of the parts one after the other.
    let mut first = Vec::with_capacity(b.parts.len());
    let mut labels = Vec::new();
    let mut is_voltage = Vec::new();
    for part in &mut b.parts {
        first.push(labels.len());
        labels.append(&mut part.labels);
        is_voltage.append(&mut part.is_voltage);
    }
    let unknown = |(part, i): (usize, usize)| first[part] + i;
    // The elements of the parts one after the other, then the links; within
    // a part and among the links in the order in which they were made.
    let count = b.elements.len();
    let part_count = b.parts.len();
    let element_part = b.element_part;
    let key = |e: usize| element_part[e].unwrap_or(part_count);
    let mut made: Vec<usize> = (0..count).collect();
    made.sort_by_key(|&e| key(e));
    let mut moved = vec![0; count];
    for (new, &old) in made.iter().enumerate() {
        moved[old] = new;
    }
    let mut slots: Vec<Option<Box<dyn Element>>> = b.elements.into_iter().map(Some).collect();
    let elements: Vec<Box<dyn Element>> = made
        .iter()
        .map(|&old| slots[old].take().expect("each element once"))
        .collect();
    let excluded_from_steady_state: Vec<bool> =
        made.iter().map(|&old| b.element_excluded[old]).collect();
    let mut parts = Vec::with_capacity(part_count);
    let mut from = 0;
    for p in 0..part_count {
        let to = from + made[from..].iter().take_while(|&&e| key(e) == p).count();
        let end = first.get(p + 1).copied().unwrap_or(labels.len());
        parts.push(PartOf {
            elements: from..to,
            unknowns: first[p]..end,
        });
        from = to;
    }
    let links = b
        .links
        .iter()
        .map(|(e, ports)| LinkOf {
            element: moved[*e],
            ports: ports.iter().map(|p| unknown(*p)).collect(),
        })
        .collect();
    let finish = |raw: Raw| {
        raw.into_iter()
            .map(|(name, quantities)| {
                let quantities = quantities
                    .into_iter()
                    .map(|(suffix, terms, places)| {
                        let mut places = places.into_iter();
                        let terms = terms
                            .into_iter()
                            .map(|t| match t {
                                Term::Unknown(_, c) => {
                                    Term::Unknown(unknown(places.next().expect("placed")), c)
                                }
                                Term::Current(e, k, c) => Term::Current(moved[e], k, c),
                            })
                            .collect();
                        Quantity { suffix, terms }
                    })
                    .collect();
                (name, quantities)
            })
            .collect::<HashMap<String, Vec<Quantity>>>()
    };

    Netlist {
        roles: Roles::of(&elements),
        elements,
        excluded_from_steady_state,
        parts,
        links,
        labels,
        is_voltage,
        node_index: b
            .node_index
            .iter()
            .map(|(n, p)| (n.clone(), unknown(*p)))
            .collect(),
        named_nodes,
        diodes: b.diodes.iter().map(|&e| moved[e]).collect(),
        two_state: b.two_state.iter().map(|&e| moved[e]).collect(),
        currents: finish(raw_currents),
        voltages: finish(raw_voltages),
        order: moved,
        signal_index,
        asked,
    }
}

/// A quantity of a component while the circuit is built: its suffix, its
/// terms, and for every term that is an unknown its part and number there.
type RawQuantity = (String, Vec<Term>, Vec<(usize, usize)>);
type Raw = Vec<(String, Vec<RawQuantity>)>;

#[cfg(test)]
mod columns_tests;
