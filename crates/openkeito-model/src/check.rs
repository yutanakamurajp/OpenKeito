//! Checks shared by the component families, the control blocks and the
//! validation of the whole model.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::components::{BusRef, Impedance};
use crate::diagnostic::Diagnostic;
use crate::model::GROUND;

pub(crate) fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

pub(crate) fn positive(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

/// E122 for a node name that is not valid.
pub(crate) fn node_name(d: &mut Vec<Diagnostic>, n: &str, path: String) {
    if !valid_name(n) {
        d.push(Diagnostic::error(
            "E122",
            path,
            format!("node name \"{n}\" is not valid"),
            "Use letters, digits, '_', '-' and '.' only. The reference node is \"gnd\".",
        ));
    }
}

/// E124 for a bus name that is not valid; the nodes of a bus written out are
/// checked as node names.
pub(crate) fn bus_ref(d: &mut Vec<Diagnostic>, b: &BusRef, path: String) {
    let node = node_name;
    match b {
        BusRef::Bus(name) => {
            if !valid_name(name) {
                d.push(Diagnostic::error(
                    "E124",
                    path,
                    format!("bus name \"{name}\" is not valid"),
                    "A bus name stands for the nodes <bus>.a, <bus>.b and <bus>.c. Use letters, \
                 digits, '_', '-' and '.' only, or write the three nodes as [\"a\", \"b\", \"c\"].",
                ));
            }
        }
        BusRef::Nodes(nodes) => {
            for (k, n) in nodes.iter().enumerate() {
                node(d, n, format!("{path}[{k}]"));
            }
        }
    }
}

/// E123: terminals of one element at the same node.
pub(crate) fn same_node(d: &mut Vec<Diagnostic>, path: String, what: String) {
    d.push(Diagnostic::error(
        "E123",
        path,
        what,
        "Connect the terminals to different nodes, or remove the component.",
    ));
}

/// The terminals of a component with `nodes` [from, to]: valid names and two
/// different nodes.
pub(crate) fn two_terminals(name: &str, nodes: &[String; 2], p: &str, d: &mut Vec<Diagnostic>) {
    let [a, b] = nodes;
    for (k, n) in [a, b].into_iter().enumerate() {
        node_name(d, n, format!("{p}.nodes[{k}]"));
    }
    if a == b {
        same_node(
            d,
            format!("{p}.nodes"),
            format!("both terminals of \"{name}\" are node \"{a}\""),
        );
    }
}

pub(crate) fn check_bus(bus: &str, path: &str, nodes: &BTreeSet<&str>, d: &mut Vec<Diagnostic>) {
    let phases = BusRef::Bus(bus.to_string()).nodes();
    if bus == GROUND {
        d.push(Diagnostic::error(
            "E152",
            path.to_string(),
            "the voltage of \"gnd\" is always 0",
            "Remove this output or name another bus.",
        ));
    } else if !phases.iter().all(|n| nodes.contains(n.as_str())) {
        let buses: BTreeSet<&str> = nodes
            .iter()
            .filter_map(|n| n.strip_suffix(".a"))
            .filter(|b| {
                nodes.contains(format!("{b}.b").as_str())
                    && nodes.contains(format!("{b}.c").as_str())
            })
            .collect();
        d.push(Diagnostic::error(
            "E154",
            path.to_string(),
            format!(
                "bus \"{bus}\" does not exist: the nodes {} are not all used by components",
                phases.join(", ")
            ),
            if buses.is_empty() {
                "No bus has all three phase nodes. Connect three-phase components to the bus, or \
                 record single nodes with node_voltage."
                    .to_string()
            } else {
                format!("Existing buses: {}.", join(buses.into_iter()))
            },
        ));
    }
}

pub(crate) fn join<'a>(items: impl Iterator<Item = &'a str>) -> String {
    items.collect::<Vec<_>>().join(", ")
}

pub(crate) fn check_node(
    node: &str,
    path: String,
    nodes: &BTreeSet<&str>,
    d: &mut Vec<Diagnostic>,
) {
    if node == GROUND {
        d.push(Diagnostic::error(
            "E152",
            path,
            "the voltage of \"gnd\" is always 0",
            "Remove this output or name another node.",
        ));
    } else if !nodes.contains(node) {
        d.push(Diagnostic::error(
            "E150",
            path,
            format!("node \"{node}\" is not used by any component"),
            format!("Existing nodes: {}.", join(nodes.iter().copied())),
        ));
    }
}

pub(crate) fn check_component(
    component: &str,
    path: String,
    components: &BTreeSet<&str>,
    d: &mut Vec<Diagnostic>,
) {
    if !components.contains(component) {
        d.push(Diagnostic::error(
            "E151",
            path,
            format!("component \"{component}\" does not exist"),
            format!("Existing components: {}.", join(components.iter().copied())),
        ));
    }
}

pub(crate) fn check_signal(
    signal: &str,
    path: String,
    signals: &BTreeSet<&str>,
    d: &mut Vec<Diagnostic>,
) {
    if !signals.contains(signal) {
        // The outputs of a block with several, when its name is written.
        let prefix = format!("{signal}.");
        let outputs: Vec<&str> = signals
            .iter()
            .copied()
            .filter(|s| s.starts_with(&prefix))
            .collect();
        let hint = if signals.is_empty() {
            "No control blocks are defined. Add a block named like the signal under \"controls\"."
                .to_string()
        } else if !outputs.is_empty() {
            format!(
                "The block \"{signal}\" has several outputs; name one of its signals: {}.",
                outputs.join(", ")
            )
        } else {
            format!(
                "A signal is the name of a control block, or <block>.<output> for a block with \
                 several outputs. Existing signals: {}.",
                join(signals.iter().copied())
            )
        };
        d.push(Diagnostic::error(
            "E170",
            path,
            format!("signal \"{signal}\" does not exist"),
            hint,
        ));
    }
}

pub(crate) fn range_error(
    d: &mut Vec<Diagnostic>,
    p: &str,
    name: &str,
    field: &str,
    value: f64,
    hint: &str,
) {
    d.push(Diagnostic::error(
        "E130",
        format!("{p}.{field}"),
        format!("{field} of \"{name}\" is {value}"),
        hint.to_string(),
    ));
}

/// E130 for a value of a control block that is not a finite number.
pub(crate) fn finite(d: &mut Vec<Diagnostic>, p: &str, name: &str, field: &str, v: f64) {
    if !v.is_finite() {
        range_error(d, p, name, field, v, "Use a finite number.");
    }
}

/// An error at `{p}.{field}` saying that `field` must be smaller than another.
pub(crate) fn order_error(d: &mut Vec<Diagnostic>, path: String, message: String, hint: &str) {
    d.push(Diagnostic::error("E132", path, message, hint.to_string()));
}

pub(crate) fn check_impedance(z: &Impedance, path: &str, name: &str, d: &mut Vec<Diagnostic>) {
    match z {
        Impedance::Sequence {
            r1_ohm,
            l1_h,
            r0_ohm,
            l0_h,
        } => {
            sequence_values(
                path,
                name,
                &[("r1_ohm", *r1_ohm), ("r0_ohm", *r0_ohm)],
                false,
                d,
            );
            sequence_values(path, name, &[("l1_h", *l1_h), ("l0_h", *l0_h)], true, d);
        }
        Impedance::Matrix {
            resistance_ohm,
            inductance_h,
        } => {
            check_matrix(path, name, "resistance_ohm", resistance_ohm, false, d);
            check_matrix(path, name, "inductance_h", inductance_h, true, d);
        }
    }
}

/// Positive- and zero-sequence values: > 0 when `strict`, otherwise >= 0.
pub(crate) fn sequence_values(
    path: &str,
    name: &str,
    values: &[(&str, f64)],
    strict: bool,
    d: &mut Vec<Diagnostic>,
) {
    for (field, v) in values {
        let ok = v.is_finite() && (*v > 0.0 || (!strict && *v == 0.0));
        if !ok {
            d.push(Diagnostic::error(
                "E130",
                format!("{path}.{field}"),
                format!("{field} of \"{name}\" is {v}"),
                if strict {
                    "Inductances and capacitances of both sequences must be > 0."
                } else {
                    "Resistances must be >= 0."
                },
            ));
        }
    }
}

/// Relative tolerance of the symmetry and definiteness checks.
pub(crate) const MATRIX_TOLERANCE: f64 = 1e-9;

/// A 3x3 phase matrix: finite, symmetric and positive definite (`definite`)
/// or positive semidefinite.
pub(crate) fn check_matrix(
    path: &str,
    name: &str,
    field: &str,
    m: &[[f64; 3]; 3],
    definite: bool,
    d: &mut Vec<Diagnostic>,
) {
    let at = format!("{path}.{field}");
    if m.iter().flatten().any(|v| !v.is_finite()) {
        d.push(Diagnostic::error(
            "E130",
            at,
            format!("{field} of \"{name}\" has an entry that is not a finite number"),
            "Use finite numbers.",
        ));
        return;
    }
    let scale = m.iter().flatten().fold(0.0_f64, |a, v| a.max(v.abs()));
    for i in 0..3 {
        for j in i + 1..3 {
            if (m[i][j] - m[j][i]).abs() > MATRIX_TOLERANCE * scale {
                d.push(Diagnostic::error(
                    "E133",
                    format!("{at}[{i}][{j}]"),
                    format!(
                        "{field} of \"{name}\" is not symmetric: [{i}][{j}] = {} but [{j}][{i}] = {}",
                        m[i][j], m[j][i]
                    ),
                    "Mutual values between two phases are the same in both directions; write the \
                     same number at [i][j] and [j][i].",
                ));
                return;
            }
        }
    }
    if !definiteness_ok(m, scale, definite) {
        d.push(Diagnostic::error(
            "E134",
            at,
            format!(
                "{field} of \"{name}\" is not positive {}definite",
                if definite { "" } else { "semi" }
            ),
            if definite {
                "Each phase must have a positive self value larger than its mutual values \
                 (the sequence values (Z0 + 2 Z1)/3 and (Z0 - Z1)/3 must give positive Z1 and Z0). \
                 For a capacitance matrix, the diagonal is positive and the off-diagonal entries \
                 are negative."
            } else {
                "Resistances must not supply energy: the self values must be >= 0 and large enough \
                 against the mutual values."
            },
        ));
    }
}

/// Sylvester's criterion: leading principal minors > 0 for a positive
/// definite matrix; every principal minor >= 0 for a semidefinite one. The
/// tolerance is relative to the largest entry.
pub(crate) fn definiteness_ok(m: &[[f64; 3]; 3], scale: f64, definite: bool) -> bool {
    if scale == 0.0 {
        return !definite;
    }
    let minor2 = |i: usize, j: usize| m[i][i] * m[j][j] - m[i][j] * m[j][i];
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let tol = |k: i32| MATRIX_TOLERANCE * scale.powi(k);
    if definite {
        m[0][0] > tol(1) && minor2(0, 1) > tol(2) && det > tol(3)
    } else {
        (0..3).all(|i| m[i][i] >= -tol(1))
            && [(0, 1), (0, 2), (1, 2)]
                .iter()
                .all(|&(i, j)| minor2(i, j) >= -tol(2))
            && det >= -tol(3)
    }
}

// ---------------------------------------------------------------------------
// Loops of ideal branches and nodes that nothing holds.

/// A `Branch` of a component with its nodes as numbers; 0 is gnd.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Edge {
    Conductance(usize, usize),
    Voltage(usize, usize),
    Constraint(Vec<(usize, f64)>),
}

/// What `topology` finds in one state of the switches.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Topology {
    /// Loops of ideal branches, each as the components (by number, in
    /// increasing order) whose branches form it.
    pub loops: Vec<Vec<usize>>,
    /// Loops beyond the `MAX_LOOPS` that are listed.
    pub more_loops: usize,
    /// Nodes whose voltage the equations do not determine.
    pub floating: Vec<usize>,
}

/// As many loops are traced to their components.
const MAX_LOOPS: usize = 8;

/// A combination of equations smaller than this, relative to their largest
/// coefficient, is 0: the equations depend on each other. It is the pivot
/// below which the solver calls a matrix singular; equations that depend on
/// each other exactly leave rounding errors near 1e-16.
const DEPENDENT: f64 = 1e-13;

/// Above this many entries the equations of the ideal transformers at loose
/// parts of the circuit are not solved; no node is then called floating.
const MAX_ENTRIES: usize = 4_000_000;

/// Disjoint sets of nodes.
pub(crate) struct Sets(Vec<usize>);

impl Sets {
    pub(crate) fn new(n: usize) -> Self {
        Sets((0..n).collect())
    }
    pub(crate) fn find(&mut self, mut i: usize) -> usize {
        let p = &mut self.0;
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    /// Joins the sets of `a` and `b`; false when they were one set.
    pub(crate) fn join(&mut self, a: usize, b: usize) -> bool {
        let (ra, rb) = (self.find(a), self.find(b));
        self.0[ra] = rb;
        ra != rb
    }
}

/// The components on the way from `from` to `to` through the branches of
/// `tree` (per node: the node at the other end and the component).
fn path(tree: &[Vec<(usize, usize)>], from: usize, to: usize) -> Vec<usize> {
    let mut came_from: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
    let mut queue = VecDeque::from([from]);
    while let Some(n) = queue.pop_front() {
        if n == to {
            break;
        }
        for &(next, owner) in &tree[n] {
            if next != from && !came_from.contains_key(&next) {
                came_from.insert(next, (n, owner));
                queue.push_back(next);
            }
        }
    }
    let mut owners = Vec::new();
    let mut n = to;
    while let Some(&(previous, owner)) = came_from.get(&n) {
        owners.push(owner);
        n = previous;
    }
    owners
}

/// An equation of ideal transformers in terms of sets of nodes, reduced by
/// the equations before it.
struct Reduced {
    /// (set, coefficient), in the order of the sets.
    terms: Vec<(usize, f64)>,
    /// The set this equation determines, and its coefficient.
    pivot: (usize, f64),
    /// The components whose equations are combined in it, and their nodes.
    owners: Vec<usize>,
    nodes: Vec<usize>,
}

/// a + factor * b, both in the order of their sets.
fn combined(a: &[(usize, f64)], b: &[(usize, f64)], factor: f64) -> Vec<(usize, f64)> {
    let mut sum = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        let next_a = a.get(i).map_or(usize::MAX, |t| t.0);
        let next_b = b.get(j).map_or(usize::MAX, |t| t.0);
        if next_a < next_b {
            sum.push(a[i]);
            i += 1;
        } else if next_b < next_a {
            sum.push((next_b, factor * b[j].1));
            j += 1;
        } else {
            sum.push((next_a, a[i].1 + factor * b[j].1));
            i += 1;
            j += 1;
        }
    }
    sum
}

/// Lists a loop once, by its components.
fn add_loop(found: &mut Topology, mut members: Vec<usize>) {
    members.sort_unstable();
    members.dedup();
    if found.loops.contains(&members) {
        return;
    }
    if found.loops.len() < MAX_LOOPS {
        found.loops.push(members);
    } else {
        found.more_loops += 1;
    }
}

/// The loops of ideal branches and the nodes that nothing holds, for the
/// branches `fixed` and `switched` (each with the number of its component)
/// between `nodes` nodes.
///
/// The equations of a time step are [[G, B^T], [B, 0]] in the node voltages
/// and the currents of the ideal branches: G the conductances between the
/// nodes, B one row per ideal branch. They have one solution exactly when
/// the rows of B are independent (no loop of ideal branches: otherwise a
/// current can circulate) and no voltage pattern other than 0 is in the
/// null space of both G and B (every node is held). The null space of G is
/// the patterns that are constant on every set of nodes joined by
/// conductances and 0 on the set of gnd.
pub(crate) fn topology(
    nodes: usize,
    fixed: &[(usize, Edge)],
    switched: &[(usize, Edge)],
) -> Topology {
    let edges = || fixed.iter().chain(switched);
    let mut found = Topology::default();

    // Ideal branches between two nodes: a branch whose ends are already
    // joined by others closes a loop.
    let mut ideal = Sets::new(nodes);
    let mut tree: Vec<Vec<(usize, usize)>> = vec![Vec::new(); nodes];
    for (owner, e) in edges() {
        let Edge::Voltage(a, b) = *e else { continue };
        if a == b {
            // The same node at both ends is an error of its own (E123).
        } else if ideal.join(a, b) {
            tree[a].push((b, *owner));
            tree[b].push((a, *owner));
        } else {
            let mut members = if found.loops.len() < MAX_LOOPS {
                path(&tree, a, b)
            } else {
                Vec::new()
            };
            members.push(*owner);
            add_loop(&mut found, members);
        }
    }
    // Ideal transformers: their equations in the voltages of the sets of
    // nodes that the ideal branches join (the set of gnd is at 0). An
    // equation that the ones before it reduce to nothing closes a loop.
    let ground = ideal.find(0);
    let mut reduced: Vec<Reduced> = Vec::new();
    for (owner, e) in edges() {
        let Edge::Constraint(terms) = e else { continue };
        let scale = terms.iter().fold(0.0_f64, |m, t| m.max(t.1.abs()));
        let mut row: Vec<(usize, f64)> = Vec::new();
        for &(node, coefficient) in terms {
            let set = ideal.find(node);
            if set != ground {
                row = combined(&row, &[(set, coefficient)], 1.0);
            }
        }
        let mut owners = vec![*owner];
        let mut of_nodes: Vec<usize> = terms.iter().map(|t| t.0).collect();
        for r in &reduced {
            if let Ok(k) = row.binary_search_by_key(&r.pivot.0, |t| t.0) {
                row = combined(&row, &r.terms, -row[k].1 / r.pivot.1);
                owners.extend(&r.owners);
                of_nodes.extend(&r.nodes);
            }
        }
        let pivot = row
            .iter()
            .copied()
            .max_by(|x, y| x.1.abs().total_cmp(&y.1.abs()));
        match pivot {
            Some(pivot) if pivot.1.abs() > DEPENDENT * scale => reduced.push(Reduced {
                terms: row,
                pivot,
                owners,
                nodes: of_nodes,
            }),
            _ => {
                // With the ideal branches that join the nodes of the
                // equations.
                if found.loops.len() < MAX_LOOPS {
                    let mut first_of: BTreeMap<usize, usize> = BTreeMap::from([(ground, 0)]);
                    for node in of_nodes {
                        let first = *first_of.entry(ideal.find(node)).or_insert(node);
                        owners.extend(path(&tree, first, node));
                    }
                }
                add_loop(&mut found, owners);
            }
        }
    }

    // The sets of nodes that conductances and ideal branches join: a set
    // without gnd is held only by the equations of ideal transformers.
    let mut held = Sets::new(nodes);
    for (_, e) in edges() {
        if let Edge::Conductance(a, b) | Edge::Voltage(a, b) = *e {
            held.join(a, b);
        }
    }
    let ground = held.find(0);
    let mut column: BTreeMap<usize, usize> = BTreeMap::new();
    for node in 0..nodes {
        let set = held.find(node);
        if set != ground {
            let next = column.len();
            column.entry(set).or_insert(next);
        }
    }
    if column.is_empty() {
        return found;
    }
    // The equations of the ideal transformers in the voltages of those
    // sets, each scaled to a largest coefficient of 1.
    let width = column.len();
    let mut rows: Vec<Vec<f64>> = Vec::new();
    for (_, e) in edges() {
        let Edge::Constraint(terms) = e else { continue };
        let mut row = vec![0.0; width];
        let mut loose = false;
        for &(node, coefficient) in terms {
            if let Some(&c) = column.get(&held.find(node)) {
                row[c] += coefficient;
                loose = true;
            }
        }
        let scale = terms.iter().fold(0.0_f64, |m, t| m.max(t.1.abs()));
        if loose && scale > 0.0 {
            row.iter_mut().for_each(|x| *x /= scale);
            rows.push(row);
        }
        if rows.len() * width > MAX_ENTRIES {
            return found;
        }
    }
    // Gauss-Jordan elimination: a set is held when its column has a pivot
    // whose row has no entry in a column without one.
    let mut pivot_row: Vec<Option<usize>> = vec![None; width];
    let mut rank = 0;
    for c in 0..width {
        let best =
            (rank..rows.len()).max_by(|&i, &j| rows[i][c].abs().total_cmp(&rows[j][c].abs()));
        let Some(best) = best.filter(|&i| rows[i][c].abs() > DEPENDENT) else {
            continue;
        };
        rows.swap(rank, best);
        let pivot = rows[rank][c];
        rows[rank].iter_mut().for_each(|x| *x /= pivot);
        let pivot = rows[rank].clone();
        for (i, other) in rows.iter_mut().enumerate() {
            let factor = other[c];
            if i != rank && factor != 0.0 {
                for (x, p) in other.iter_mut().zip(&pivot) {
                    *x -= factor * p;
                }
                other[c] = 0.0;
            }
        }
        pivot_row[c] = Some(rank);
        rank += 1;
    }
    let free: Vec<usize> = (0..width).filter(|&c| pivot_row[c].is_none()).collect();
    let loose = |c: usize| match pivot_row[c] {
        None => true,
        Some(r) => free.iter().any(|&f| rows[r][f].abs() > DEPENDENT),
    };
    let loose: Vec<bool> = (0..width).map(loose).collect();
    for node in 0..nodes {
        if column.get(&held.find(node)).is_some_and(|&c| loose[c]) {
            found.floating.push(node);
        }
    }
    found
}
