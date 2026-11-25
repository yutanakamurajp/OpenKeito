//! The parts of a circuit that are solved on their own.
//!
//! An element with decoupled ends (a travelling-wave line, [Dommel 1969])
//! has no matrix entry between its ends, and what one end reads of the other
//! is at least one time step old. Where only such elements join two sets of
//! nodes, the system matrix is block diagonal: every block is a part with
//! its own unknowns, matrix, factors and solution. A part is smaller than
//! the circuit, a switching action factorizes only the part it happens in,
//! and within a stage the parts do not depend on each other, so they can be
//! solved side by side ([Marti and Linares 1994]).
//!
//! The elements of a part number their unknowns from the first unknown of
//! the part (`netlist`): they are given the slice of the right-hand side and
//! of the solution that belongs to the part, so parts never share memory
//! that one of them writes. An element that joins parts (a link) numbers the
//! nodes of its ends from 0; the solver copies their voltages into a buffer
//! of the link and takes the currents it injects from another (its ports).
//!
//! What makes the result independent of the order and of the number of
//! threads: within the stages of a step, a part reads only its own elements
//! and the port currents of its links, which were written before the parts
//! started; a link is advanced, and its port currents for the next stages
//! are written, only when every part has finished.

use std::ops::Range;

use openkeito_model::LinearSolver;

use super::operating_point::MAX_OPERATING_POINT_ITERATIONS;
use crate::element::{Element, OperatingPoint, Stage};
use crate::linalg::{Factors, Scalar, Solver, Stamp};
use crate::netlist::{Netlist, Term};

/// A matrix entry: row, column, value.
pub(super) type Entry = (usize, usize, f64);

/// A part of the circuit.
pub(super) struct Part {
    /// Its elements among the elements of the circuit.
    pub elements: Range<usize>,
    /// Its unknowns among the unknowns of the circuit.
    pub unknowns: Range<usize>,
    pub solver: Solver,
    /// Its elements with an operating point, numbered within the part.
    pub operating_points: Vec<usize>,
    /// The ports of links that lie in the part: the unknown within the part
    /// and the place of the port in the port buffers.
    pub ports: Vec<(usize, usize)>,
}

/// An element that joins parts.
pub(super) struct Link {
    pub element: usize,
    /// The unknown of the circuit behind every port.
    pub ports: Vec<usize>,
    /// Its ports in the port buffers.
    pub buffer: Range<usize>,
}

/// How a circuit is divided.
pub(super) struct Layout {
    pub parts: Vec<Part>,
    pub links: Vec<Link>,
    /// The part of every element; for a link, the number of parts plus its
    /// number among the links.
    home: Vec<u32>,
    /// The first of the links among the elements.
    pub links_from: usize,
    /// The part of every unknown, and the first unknown of every part.
    part_of_unknown: Vec<u32>,
    first: Vec<usize>,
    /// Length of the port buffers.
    pub port_count: usize,
    /// The circuit is one part without links.
    single: bool,
}

/// Collects the entries an element writes.
struct Collect<T>(Vec<(usize, usize, T)>);

impl<T: Scalar> Stamp<T> for Collect<T> {
    fn add(&mut self, row: usize, col: usize, value: T) {
        self.0.push((row, col, value));
    }
}

/// Passes the entries of an element of a part on to a matrix of the whole
/// circuit.
pub(super) struct Shifted<'a, T> {
    pub m: &'a mut dyn Stamp<T>,
    pub first: usize,
}

impl<T: Scalar> Stamp<T> for Shifted<'_, T> {
    #[inline]
    fn add(&mut self, row: usize, col: usize, value: T) {
        self.m.add(self.first + row, self.first + col, value);
    }
}

/// Passes the entries of a link on to a matrix of the whole circuit.
pub(super) struct Ported<'a, T> {
    pub m: &'a mut dyn Stamp<T>,
    pub ports: &'a [usize],
}

impl<T: Scalar> Stamp<T> for Ported<'_, T> {
    #[inline]
    fn add(&mut self, row: usize, col: usize, value: T) {
        self.m.add(self.ports[row], self.ports[col], value);
    }
}

/// Largest number of ports whose values are gathered without an allocation.
const INLINE_PORTS: usize = 12;

impl Layout {
    pub fn new(net: &Netlist, choice: LinearSolver) -> Layout {
        let count = net.parts.len();
        let mut home = vec![0; net.elements.len()];
        let mut part_of_unknown = vec![0; net.labels.len()];
        for (p, part) in net.parts.iter().enumerate() {
            home[part.elements.clone()].fill(p as u32);
            part_of_unknown[part.unknowns.clone()].fill(p as u32);
        }
        let mut links = Vec::with_capacity(net.links.len());
        let mut port_count = 0;
        for (k, link) in net.links.iter().enumerate() {
            home[link.element] = (count + k) as u32;
            links.push(Link {
                element: link.element,
                ports: link.ports.clone(),
                buffer: port_count..port_count + link.ports.len(),
            });
            port_count += link.ports.len();
        }
        let mut layout = Layout {
            parts: Vec::new(),
            links_from: net.links.first().map_or(net.elements.len(), |l| l.element),
            links,
            home,
            part_of_unknown,
            first: net.parts.iter().map(|p| p.unknowns.start).collect(),
            port_count,
            single: count == 1 && net.links.is_empty(),
        };
        // Every entry a link can have, per part.
        let boundary = layout.boundary_with(&net.elements, |e, m| e.structure(m));
        for (p, part) in net.parts.iter().enumerate() {
            let n = part.unknowns.len();
            let elements = &net.elements[part.elements.clone()];
            let solver = Solver::new(choice.is_sparse(n), n, |s| {
                for e in elements {
                    e.structure(s);
                }
                for &(row, col, value) in &boundary[p] {
                    s.add(row, col, value);
                }
            });
            let mut ports = Vec::new();
            for link in &layout.links {
                for (k, &unknown) in link.ports.iter().enumerate() {
                    if part.unknowns.contains(&unknown) {
                        ports.push((unknown - part.unknowns.start, link.buffer.start + k));
                    }
                }
            }
            layout.parts.push(Part {
                elements: part.elements.clone(),
                unknowns: part.unknowns.clone(),
                solver,
                operating_points: Vec::new(),
                ports,
            });
        }
        layout.find_operating_points(&net.elements);
        layout
    }

    /// Finds the elements with an operating point of every part.
    pub fn find_operating_points(&mut self, elements: &[Box<dyn Element>]) {
        for part in &mut self.parts {
            part.operating_points = elements[part.elements.clone()]
                .iter()
                .enumerate()
                .filter(|(_, e)| e.has_operating_point())
                .map(|(i, _)| i)
                .collect();
        }
    }

    /// The part of element `e`; `None` for a link.
    #[inline]
    pub fn part_of(&self, e: usize) -> Option<usize> {
        let h = self.home[e] as usize;
        (h < self.parts.len()).then_some(h)
    }

    /// The unknown of the circuit that element `e` numbers `k`.
    pub fn unknown(&self, e: usize, k: usize) -> usize {
        match self.part_of(e) {
            Some(p) => self.parts[p].unknowns.start + k,
            None => self.links[self.home[e] as usize - self.parts.len()].ports[k],
        }
    }

    /// True when the circuit is one part without links: every element then
    /// numbers the unknowns of the circuit, and `with` hands on `x` as it
    /// is. Loops over elements ask this once instead of calling `with` for
    /// each.
    #[inline]
    pub fn is_whole(&self) -> bool {
        self.single
    }

    /// Calls `f` with what element `e` sees of the vector `x` over the
    /// unknowns of the circuit: the unknowns of its part, or for a link the
    /// values at its ports.
    #[inline(always)]
    pub fn with<T: Scalar, R>(&self, e: usize, x: &[T], f: impl FnOnce(&[T]) -> R) -> R {
        if self.single {
            f(x)
        } else {
            self.with_divided(e, x, f)
        }
    }

    /// `with` for a divided circuit.
    #[inline(never)]
    fn with_divided<T: Scalar, R>(&self, e: usize, x: &[T], f: impl FnOnce(&[T]) -> R) -> R {
        let h = self.home[e] as usize;
        if h < self.parts.len() {
            return f(&x[self.parts[h].unknowns.clone()]);
        }
        let ports = &self.links[h - self.parts.len()].ports;
        if ports.len() <= INLINE_PORTS {
            let mut values = [T::ZERO; INLINE_PORTS];
            for (v, &p) in values.iter_mut().zip(ports) {
                *v = x[p];
            }
            f(&values[..ports.len()])
        } else {
            let values: Vec<T> = ports.iter().map(|&p| x[p]).collect();
            f(&values)
        }
    }

    /// Lets every element write its entries into a matrix of the whole
    /// circuit through `stamp`, passing the element's index.
    pub fn assemble_indexed<T: Scalar>(
        &self,
        elements: &[Box<dyn Element>],
        m: &mut dyn Stamp<T>,
        mut stamp: impl FnMut(usize, &dyn Element, &mut dyn Stamp<T>),
    ) {
        for part in &self.parts {
            let mut shifted = Shifted {
                m: &mut *m,
                first: part.unknowns.start,
            };
            for idx in part.elements.clone() {
                stamp(idx, elements[idx].as_ref(), &mut shifted);
            }
        }
        for link in &self.links {
            let mut ported = Ported {
                m: &mut *m,
                ports: &link.ports,
            };
            stamp(link.element, elements[link.element].as_ref(), &mut ported);
        }
    }

    /// The entries the links write into the matrix of every part.
    fn boundary_with(
        &self,
        elements: &[Box<dyn Element>],
        stamp: impl Fn(&dyn Element, &mut dyn Stamp),
    ) -> Vec<Vec<Entry>> {
        let mut entries = vec![Vec::new(); self.first.len()];
        let mut collect = Collect(Vec::new());
        for link in &self.links {
            collect.0.clear();
            stamp(elements[link.element].as_ref(), &mut collect);
            for &(row, col, value) in &collect.0 {
                let (row, col) = (link.ports[row], link.ports[col]);
                let p = self.part_of_unknown[row] as usize;
                assert_eq!(
                    p, self.part_of_unknown[col] as usize,
                    "an element that joins parts has no matrix entry between them"
                );
                let first = self.first[p];
                entries[p].push((row - first, col - first, value));
            }
        }
        entries
    }

    /// The entries the links write into the matrix of every part for the
    /// stage coefficient `alpha`.
    pub fn boundary(&self, elements: &[Box<dyn Element>], alpha: f64) -> Vec<Vec<Entry>> {
        self.boundary_with(elements, |e, m| e.stamp(alpha, m))
    }
}

/// The factors of the matrix of a part for one stage coefficient.
pub(super) struct Slot {
    alpha: f64,
    /// The version of the part (`Simulator::versions`) they were computed
    /// for: they hold while it stays.
    version: u64,
    factors: Factors<f64>,
}

/// The factors of a part: those of the regular stages and those of one
/// other stage coefficient (the half steps after a switching action under
/// the trapezoidal rule, the part of a split step). A part in which nothing
/// changed keeps both when another part switches.
#[derive(Default)]
pub(super) struct PartLu {
    regular: Option<Slot>,
    other: Option<Slot>,
    /// `other` is the one in use.
    use_other: bool,
}

impl PartLu {
    /// The factors in use.
    #[inline]
    pub fn active(&self) -> &Factors<f64> {
        let slot = if self.use_other {
            &self.other
        } else {
            &self.regular
        };
        &slot.as_ref().expect("factorized before it is used").factors
    }

    /// True when the factors for `alpha` are those of `version`.
    fn holds(&self, other: bool, alpha: f64, version: u64) -> bool {
        let slot = if other { &self.other } else { &self.regular };
        slot.as_ref()
            .is_some_and(|s| s.alpha == alpha && s.version == version)
    }

    fn set(&mut self, other: bool, slot: Slot) {
        if other {
            self.other = Some(slot);
        } else {
            self.regular = Some(slot);
        }
    }
}

/// The factors of every part.
pub(super) struct Lu {
    pub parts: Vec<PartLu>,
}

impl Lu {
    pub fn new(parts: usize) -> Lu {
        Lu {
            parts: (0..parts).map(|_| PartLu::default()).collect(),
        }
    }

    /// The largest condition estimate of the factors in use.
    pub fn condition_estimate(&self) -> f64 {
        self.parts
            .iter()
            .map(|p| p.active().condition_estimate())
            .fold(1.0, f64::max)
    }
}

/// What the solution of parts counted.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Counts {
    /// Solutions beyond the first of each stage.
    pub solves: u64,
    pub factorizations: u64,
    /// Largest condition estimate of the factorizations, and of those with
    /// the stage coefficient of the regular stages.
    pub condition: f64,
    pub regular_condition: f64,
    /// Additional solutions for operating points: in all, and the most in
    /// one stage of one part.
    pub iterations: u64,
    pub max_iterations: u64,
    /// Largest node voltage and current of the solutions recorded.
    pub peaks: (f64, f64),
}

impl Counts {
    pub fn merge(&mut self, other: &Counts) {
        self.solves += other.solves;
        self.factorizations += other.factorizations;
        self.condition = self.condition.max(other.condition);
        self.regular_condition = self.regular_condition.max(other.regular_condition);
        self.iterations += other.iterations;
        self.max_iterations = self.max_iterations.max(other.max_iterations);
        self.peaks = (
            self.peaks.0.max(other.peaks.0),
            self.peaks.1.max(other.peaks.1),
        );
    }

    fn factorized(&mut self, factors: &Factors<f64>, regular: bool) {
        let condition = factors.condition_estimate();
        self.condition = self.condition.max(condition);
        if regular {
            self.regular_condition = self.regular_condition.max(condition);
        }
    }

    /// True when nothing was counted.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.solves == 0 && self.factorizations == 0
    }
}

/// Why the solution of a part failed. Unknowns are numbered within the part.
#[derive(Debug)]
pub(super) enum PartError {
    Singular(usize),
    NonFinite(usize),
    NotConverged {
        components: Vec<String>,
        iterations: u64,
    },
}

/// Factorizes the matrix of a part for `alpha`; `boundary` holds the entries
/// of the links.
pub(super) fn factor_part(
    part: &Part,
    elements: &[Box<dyn Element>],
    boundary: &[Entry],
    alpha: f64,
) -> Result<Factors<f64>, usize> {
    part.solver.factor(|m| {
        for e in elements {
            e.stamp(alpha, m);
        }
        for &(row, col, value) in boundary {
            m.add(row, col, value);
        }
    })
}

/// A part with what the solution of its stages reads and writes. Nothing in
/// it belongs to another part.
pub(super) struct PartRun<'a> {
    pub part: &'a Part,
    /// The elements of the part.
    pub elements: &'a mut [Box<dyn Element>],
    pub lu: &'a mut PartLu,
    pub version: &'a mut u64,
    /// The entries of the links in the matrix of the part, for the stage
    /// coefficient of the stages to be solved.
    pub boundary: &'a [Entry],
    /// The stage coefficient of the regular stages.
    pub alpha_regular: f64,
}

impl PartRun<'_> {
    /// Makes the factors for `alpha` those of the present states of the
    /// part, and the ones in use.
    pub fn refresh(&mut self, alpha: f64, counts: &mut Counts) -> Result<(), usize> {
        let other = alpha != self.alpha_regular;
        if !self.lu.holds(other, alpha, *self.version) {
            let factors = factor_part(self.part, self.elements, self.boundary, alpha);
            counts.factorizations += 1;
            if let Ok(f) = &factors {
                counts.factorized(f, !other);
            }
            self.lu.set(
                other,
                Slot {
                    alpha,
                    version: *self.version,
                    factors: factors?,
                },
            );
        }
        self.lu.use_other = other;
        Ok(())
    }

    /// The right-hand side of `stage` and its solution with the factors in
    /// use. `port_b` holds the currents of the links at their ports.
    #[inline]
    fn solve(
        &self,
        stage: &Stage,
        t: f64,
        port_b: &[f64],
        b: &mut [f64],
        x: &mut [f64],
    ) -> Result<(), PartError> {
        b.fill(0.0);
        for &(unknown, port) in &self.part.ports {
            b[unknown] += port_b[port];
        }
        solve_and_check(self.lu.active(), self.elements, stage, t, b, x)
            .map_err(PartError::NonFinite)
    }

    /// Solves `stage` at time `t` into `x`, again and again until the
    /// operating point of every element of the part agrees with the
    /// solution. When a stamp changed, the factors in use are replaced by
    /// those of the present operating points; when only right-hand sides
    /// changed, they are used again.
    #[inline]
    pub fn stage(
        &mut self,
        stage: &Stage,
        t: f64,
        port_b: &[f64],
        b: &mut [f64],
        x: &mut [f64],
        counts: &mut Counts,
    ) -> Result<(), PartError> {
        self.solve(stage, t, port_b, b, x)?;
        if self.part.operating_points.is_empty() {
            return Ok(());
        }
        let mut iterations = 0;
        let mut moved = Vec::new();
        loop {
            let mut matrix_changed = false;
            moved.clear();
            for &i in &self.part.operating_points {
                match self.elements[i].update_operating_point(t, stage, x) {
                    OperatingPoint::Consistent => {}
                    OperatingPoint::RhsChanged => moved.push(i),
                    OperatingPoint::MatrixChanged => {
                        matrix_changed = true;
                        moved.push(i);
                    }
                }
            }
            if moved.is_empty() {
                break;
            }
            if iterations >= MAX_OPERATING_POINT_ITERATIONS {
                return Err(PartError::NotConverged {
                    components: moved
                        .iter()
                        .map(|&i| self.elements[i].name().to_string())
                        .collect(),
                    iterations,
                });
            }
            iterations += 1;
            if matrix_changed {
                // The factors kept for the other stage coefficient are
                // those of the operating points before.
                *self.version += 1;
                self.refresh(stage.alpha, counts)
                    .map_err(PartError::Singular)?;
            }
            self.solve(stage, t, port_b, b, x)?;
            counts.solves += 1;
        }
        counts.iterations += iterations;
        counts.max_iterations = counts.max_iterations.max(iterations);
        Ok(())
    }

    /// Updates the states of the elements of the part from the solution `x`
    /// of `stage`.
    #[inline]
    pub fn advance(&mut self, stage: &Stage, x: &[f64]) {
        for e in self.elements.iter_mut() {
            e.advance(stage, x);
        }
    }
}

/// A recorded or measured quantity that one part or one link computes, with
/// the place of its value in the staged values of a step.
pub(super) struct Owned {
    /// Its terms; unknowns and elements are numbered within the part. For a
    /// link, unknowns are those of the circuit and every current is one of
    /// the link.
    pub terms: Vec<Term>,
    pub staged: usize,
}

/// Where the outputs and the measured quantities of a step are computed
/// when the parts are solved on their own: a quantity is computed by the
/// part (or the link) whose elements carry its currents, right after its
/// solution and before its elements are advanced.
#[derive(Default)]
pub(super) struct Outputs {
    /// Per part and per link.
    pub of_parts: Vec<Vec<Owned>>,
    pub of_links: Vec<Vec<Owned>>,
    /// The destination of every staged value: an output column, or the
    /// number of output columns plus a measured quantity.
    pub destinations: Vec<usize>,
    /// The staged values of every part and of every link.
    pub staged_of_parts: Vec<Range<usize>>,
    pub staged_of_links: Vec<Range<usize>>,
    /// Quantities of node voltages of several parts: destination and terms.
    pub of_circuit: Vec<(usize, Vec<Term>)>,
}

impl Outputs {
    /// Assigns every quantity (`None`: not a combination of terms) to its
    /// owner. Every component keeps its currents within one part or one link.
    pub fn new(layout: &Layout, quantities: &[Option<&[Term]>]) -> Outputs {
        let parts = layout.parts.len();
        let mut of_parts: Vec<Vec<(usize, Vec<Term>)>> = vec![Vec::new(); parts];
        let mut of_links: Vec<Vec<(usize, Vec<Term>)>> = vec![Vec::new(); layout.links.len()];
        let mut of_circuit = Vec::new();
        for (destination, terms) in quantities.iter().enumerate() {
            let Some(terms) = terms else { continue };
            // The owners of its currents and the parts of its unknowns.
            let mut carriers: Vec<usize> = Vec::new();
            let mut homes: Vec<usize> = Vec::new();
            for term in terms.iter() {
                let (list, owner) = match *term {
                    Term::Unknown(i, _) => (&mut homes, layout.part_of_unknown[i] as usize),
                    Term::Current(e, _, _) => (&mut carriers, layout.home[e] as usize),
                };
                if !list.contains(&owner) {
                    list.push(owner);
                }
            }
            let quantity = (destination, terms.to_vec());
            match (carriers.as_slice(), homes.as_slice()) {
                ([], [p]) => of_parts[*p].push(quantity),
                ([], _) => of_circuit.push(quantity),
                ([p], homes) if *p < parts && homes.iter().all(|h| h == p) => {
                    of_parts[*p].push(quantity)
                }
                ([l], _) if *l >= parts => of_links[*l - parts].push(quantity),
                _ => unreachable!("every component keeps its currents in one part or link"),
            }
        }
        let mut out = Outputs {
            of_circuit,
            ..Outputs::default()
        };
        for (p, owned) in of_parts.into_iter().enumerate() {
            let part = &layout.parts[p];
            let first = out.destinations.len();
            let owned = owned
                .into_iter()
                .map(|(destination, terms)| {
                    out.destinations.push(destination);
                    let terms = terms
                        .into_iter()
                        .map(|t| match t {
                            Term::Unknown(i, c) => Term::Unknown(i - part.unknowns.start, c),
                            Term::Current(e, k, c) => Term::Current(e - part.elements.start, k, c),
                        })
                        .collect();
                    Owned {
                        terms,
                        staged: out.destinations.len() - 1 - first,
                    }
                })
                .collect();
            out.of_parts.push(owned);
            out.staged_of_parts.push(first..out.destinations.len());
        }
        for owned in of_links {
            let first = out.destinations.len();
            let owned = owned
                .into_iter()
                .map(|(destination, terms)| {
                    out.destinations.push(destination);
                    Owned {
                        terms,
                        staged: out.destinations.len() - 1 - first,
                    }
                })
                .collect();
            out.of_links.push(owned);
            out.staged_of_links.push(first..out.destinations.len());
        }
        out
    }
}

/// Evaluates `elements`' right-hand sides (added onto `b`), solves `lu`, and
/// returns the index of any non-finite entry in `x`.
#[inline(always)]
pub(super) fn solve_and_check(
    lu: &Factors<f64>,
    elements: &[Box<dyn Element>],
    stage: &Stage,
    t: f64,
    b: &mut [f64],
    x: &mut [f64],
) -> Result<(), usize> {
    for e in elements {
        e.rhs(t, stage, b);
    }
    lu.solve(b, x);
    match x.iter().position(|v| !v.is_finite()) {
        Some(k) => Err(k),
        None => Ok(()),
    }
}

/// Value of a linear combination of unknowns and element currents.
#[inline(always)]
pub(super) fn eval_terms(terms: &[Term], x: &[f64], current: impl Fn(usize, usize) -> f64) -> f64 {
    terms
        .iter()
        .map(|term| match *term {
            Term::Unknown(i, c) => c * x[i],
            Term::Current(e, k, c) => c * current(e, k),
        })
        .sum()
}

/// Largest node voltage and largest branch/conduction current of `x`.
#[inline(always)]
pub(super) fn peak_magnitudes(
    voltages: &[usize],
    currents: &[usize],
    two_state: &[usize],
    x: &[f64],
    conduction: impl Fn(usize) -> f64,
) -> (f64, f64) {
    let largest = |v: &[usize]| v.iter().fold(0.0_f64, |a, &i| a.max(x[i].abs()));
    let mut current = largest(currents);
    for &e in two_state {
        current = current.max(conduction(e).abs());
    }
    (largest(voltages), current)
}

/// The unknowns and elements of a part whose magnitudes set the thresholds
/// of the diode states, numbered within the part.
#[derive(Default)]
pub(super) struct Magnitudes {
    pub voltages: Vec<usize>,
    pub currents: Vec<usize>,
    pub two_state: Vec<usize>,
}

/// What the parts of a step that is solved without a switching action share:
/// they only read it.
pub(super) struct Plain<'a> {
    pub stages: &'a [Stage],
    /// The time of every stage.
    pub times: [f64; 2],
    /// The currents of the links at their ports for every stage.
    pub port_b: [&'a [f64]; 2],
    pub boundary: &'a [Vec<Entry>],
    pub alpha_regular: f64,
    pub outputs: &'a Outputs,
    /// For the largest voltage and current of the solutions: the unknowns
    /// and elements to look at in every part. `None` in a circuit without
    /// diodes.
    pub magnitudes: Option<&'a [Magnitudes]>,
}

/// Some parts that follow each other, with everything their step writes.
/// The groups of a step share no memory that any of them writes.
pub(super) struct PartGroup<'a> {
    pub first_part: usize,
    pub parts: &'a [Part],
    pub elements: &'a mut [Box<dyn Element>],
    pub lu: &'a mut [PartLu],
    pub versions: &'a mut [u64],
    /// Right-hand side, solution of the first of two stages and solution of
    /// the last stage, over the unknowns of the group.
    pub b: &'a mut [f64],
    pub x_first: &'a mut [f64],
    pub x: &'a mut [f64],
    pub staged: &'a mut [f64],
    pub counts: Counts,
    /// The part that failed, why, and the time of the stage.
    pub error: Option<(usize, PartError, f64)>,
}

impl PartGroup<'_> {
    /// Solves every stage of the step for every part of the group, and
    /// advances its elements. Stops at the first part that fails.
    pub fn run(&mut self, s: &Plain) {
        let Some(first) = self.parts.first() else {
            return;
        };
        let (first_element, first_unknown) = (first.elements.start, first.unknowns.start);
        let first_staged = s.outputs.staged_of_parts[self.first_part].start;
        let count = s.stages.len();
        for (i, part) in self.parts.iter().enumerate() {
            let p = self.first_part + i;
            let unknowns = part.unknowns.start - first_unknown..part.unknowns.end - first_unknown;
            let mut run = PartRun {
                part,
                elements: &mut self.elements
                    [part.elements.start - first_element..part.elements.end - first_element],
                lu: &mut self.lu[i],
                version: &mut self.versions[i],
                boundary: &s.boundary[p],
                alpha_regular: s.alpha_regular,
            };
            let b = &mut self.b[unknowns.clone()];
            for (k, stage) in s.stages.iter().enumerate() {
                let last = k + 1 == count;
                let x = if last {
                    &mut self.x[unknowns.clone()]
                } else {
                    &mut self.x_first[unknowns.clone()]
                };
                let t = s.times[k];
                if let Err(e) = run.stage(stage, t, s.port_b[k], b, x, &mut self.counts) {
                    self.error = Some((p, e, t));
                    return;
                }
                if last {
                    let staged = &s.outputs.staged_of_parts[p];
                    let staged =
                        &mut self.staged[staged.start - first_staged..staged.end - first_staged];
                    for owned in &s.outputs.of_parts[p] {
                        staged[owned.staged] = eval_terms(&owned.terms, x, |e, k| {
                            run.elements[e].current(k, t, stage, x)
                        });
                    }
                    if let Some(magnitudes) = s.magnitudes {
                        let m = &magnitudes[p];
                        let (v, i) =
                            peak_magnitudes(&m.voltages, &m.currents, &m.two_state, x, |e| {
                                run.elements[e].conduction_current(x)
                            });
                        let peaks = &mut self.counts.peaks;
                        *peaks = (peaks.0.max(v), peaks.1.max(i));
                    }
                }
                run.advance(stage, x);
            }
        }
    }
}

/// What the links of a step read after its parts are solved.
pub(super) struct AfterParts<'a> {
    pub stages: &'a [Stage],
    /// The time of every stage of this step and of the next one.
    pub times: [f64; 2],
    pub next_times: [f64; 2],
    /// The solutions of the stages over the unknowns of the circuit.
    pub x: [&'a [f64]; 2],
    pub outputs: &'a Outputs,
}

/// Some links that follow each other, with everything their step writes.
pub(super) struct LinkGroup<'a> {
    pub first_link: usize,
    pub links: &'a [Link],
    /// The elements of the links of the group.
    pub elements: &'a mut [Box<dyn Element>],
    /// The voltages at the ports of the group, and for every stage of the
    /// next step the currents there.
    pub port_x: &'a mut [f64],
    pub port_b: [&'a mut [f64]; 2],
    pub staged: &'a mut [f64],
}

impl LinkGroup<'_> {
    /// Advances every link of the group over the stages of the step, and
    /// writes the currents at its ports for the stages of the next step,
    /// which follow from the solutions up to the end of this one.
    pub fn run(&mut self, s: &AfterParts) {
        let Some(first) = self.links.first() else {
            return;
        };
        let first_port = first.buffer.start;
        let first_staged = s.outputs.staged_of_links[self.first_link].start;
        let count = s.stages.len();
        for (i, link) in self.links.iter().enumerate() {
            let l = self.first_link + i;
            let element = &mut self.elements[i];
            let ports = link.buffer.start - first_port..link.buffer.end - first_port;
            let x = &mut self.port_x[ports.clone()];
            for (k, stage) in s.stages.iter().enumerate() {
                for (v, &unknown) in x.iter_mut().zip(&link.ports) {
                    *v = s.x[k][unknown];
                }
                if k + 1 == count {
                    let staged = &s.outputs.staged_of_links[l];
                    let staged =
                        &mut self.staged[staged.start - first_staged..staged.end - first_staged];
                    for owned in &s.outputs.of_links[l] {
                        staged[owned.staged] = eval_terms(&owned.terms, s.x[k], |_, n| {
                            element.current(n, s.times[k], stage, x)
                        });
                    }
                }
                element.advance(stage, x);
            }
            for (k, stage) in s.stages.iter().enumerate() {
                let b = &mut self.port_b[k][ports.clone()];
                b.fill(0.0);
                element.rhs(s.next_times[k], stage, b);
            }
        }
    }
}

/// Divides items with the given weights into at most `groups` groups of
/// items that follow each other, of about the same weight: the first item of
/// every group, and the number of items at the end.
pub(super) fn balanced(weights: impl Iterator<Item = usize>, groups: usize) -> Vec<usize> {
    let weights: Vec<usize> = weights.map(|w| w.max(1)).collect();
    let total: usize = weights.iter().sum();
    let groups = groups.clamp(1, weights.len().max(1));
    let mut bounds = vec![0];
    let mut sum = 0;
    for (i, w) in weights.iter().enumerate() {
        // A group starts where the weight before it has reached the share
        // of the groups before it.
        if i > 0 && bounds.len() < groups && sum * groups >= bounds.len() * total {
            bounds.push(i);
        }
        sum += w;
    }
    bounds.push(weights.len());
    bounds
}

/// The work of a step in a part, in the unit of `THREAD_WORK`: its unknowns
/// and its elements.
pub(super) fn work(part: &Part) -> usize {
    part.unknowns.len() + part.elements.len()
}

/// The work of a step (`work` over all parts) that every thread must have
/// for a further thread to gain: below it, handing the parts to the threads
/// and waiting for them costs more than it saves (docs/performance.md).
pub(super) const THREAD_WORK: usize = 100;

#[cfg(test)]
mod tests {
    use super::balanced;
    use crate::sim::Simulator;

    /// Six buses joined into a ring by Bergeron lines, a source on bus 0, a
    /// load on every bus and a fault on bus 3.
    fn ring(method: &str) -> openkeito_model::Model {
        let mut c = vec![
            r#"{"type": "three_phase_source", "name": "G", "bus": "b0", "line_voltage_rms_v": 66e3,
            "frequency_hz": 50.0, "impedance": {"type": "sequence", "r1_ohm": 0.5, "l1_h": 0.016,
            "r0_ohm": 1.0, "l0_h": 0.048}}"#
                .to_string(),
            r#"{"type": "fault", "name": "F1", "bus": "b3", "phases": ["a", "b"], "ground": true,
            "start_time_s": 1.5e-3, "clear_time_s": 4e-3}"#
                .to_string(),
        ];
        for k in 0..6 {
            let next = (k + 1) % 6;
            c.push(format!(
                r#"{{"type": "bergeron_line", "name": "L{k}", "buses": ["b{k}", "b{next}"], "length_km": 10.0,
                "per_km": {{"type": "sequence", "r1_ohm_per_km": 0.03, "l1_h_per_km": 0.9e-3, "c1_f_per_km": 12.7e-9,
                "r0_ohm_per_km": 0.2, "l0_h_per_km": 3e-3, "c0_f_per_km": 7e-9}}}}"#
            ));
            c.push(format!(
                r#"{{"type": "coupled_rl", "name": "LD{k}", "buses": ["b{k}", "gnd"],
                "impedance": {{"type": "sequence", "r1_ohm": 400.0, "l1_h": 0.4, "r0_ohm": 400.0, "l0_h": 0.4}}}}"#
            ));
        }
        let text = format!(
            r#"{{"format": "openkeito-model", "version": "0.5",
            "simulation": {{"time_step_s": 1e-5, "end_time_s": 0.01, "method": "{method}",
                            "initialization": "steady_state", "partitioning": "every_line"}},
            "components": [{}],
            "outputs": [{{"type": "bus_voltage", "bus": "b3"}}, {{"type": "branch_current", "component": "L2"}},
                        {{"type": "branch_voltage", "component": "L4"}}, {{"type": "branch_current", "component": "LD5"}},
                        {{"type": "branch_current", "component": "F1"}}]}}"#,
            c.join(",\n")
        );
        openkeito_model::load_str(&text).unwrap().model
    }

    fn rows(sim: Simulator) -> (Vec<Vec<u64>>, u64, u64) {
        let mut rows = Vec::new();
        let stats = sim
            .run(|_, row| {
                rows.push(row.iter().map(|v| v.to_bits()).collect());
                Ok(())
            })
            .unwrap();
        assert_eq!(stats.parts, 6);
        (rows, stats.solves, stats.factorizations)
    }

    #[test]
    fn the_parts_on_their_own_give_what_the_stages_over_the_circuit_give() {
        // A step without an action is solved part by part, all stages of a
        // part at once (`plain_step`); a step with one, stage by stage over
        // the circuit (`attempt`). Both are the same arithmetic in another
        // order of the parts: the same rows to the last bit, also when every
        // step is solved the second way.
        for method in ["2s-dirk", "trapezoidal"] {
            let model = ring(method);
            let plain = rows(Simulator::from_model(&model).unwrap());
            let staged = rows(Simulator::from_model(&model).unwrap().without_plain_steps());
            assert!(plain == staged, "{method}");
        }
    }

    #[test]
    fn groups_follow_each_other_and_have_about_the_same_weight() {
        assert_eq!(balanced([1, 1, 1, 1].into_iter(), 2), [0, 2, 4]);
        assert_eq!(balanced([1, 1, 1, 1].into_iter(), 1), [0, 4]);
        // Not more groups than items.
        assert_eq!(balanced([5, 5].into_iter(), 8), [0, 1, 2]);
        // A heavy item is a group of its own.
        assert_eq!(balanced([10, 1, 1, 1, 1].into_iter(), 2), [0, 1, 5]);
        assert_eq!(balanced(std::iter::empty(), 3), [0, 0]);
        // Every item is in one group, whatever the weights.
        for groups in 1..9 {
            let bounds = balanced((0..37).map(|i| i * i % 11), groups);
            assert_eq!((bounds[0], bounds[bounds.len() - 1]), (0, 37));
            assert!(bounds.windows(2).all(|b| b[0] < b[1]));
            assert!(bounds.len() - 1 <= groups);
        }
    }
}
