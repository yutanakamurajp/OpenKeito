//! The time loop: builds the circuit of a model and runs it.

mod error;
mod operating_point;
mod parts;
mod pool;
mod report;
mod split;
mod stages;
mod steady;
mod switching;

use crate::clock::Stopwatch;
use std::collections::HashMap;

use openkeito_model::{self as model, Diagnostic, Method, Model, Severity};

pub use error::SimError;
pub use operating_point::MAX_OPERATING_POINT_ITERATIONS;
use parts::{
    eval_terms, solve_and_check, AfterParts, Counts, Entry, Layout, LinkGroup, Lu, Magnitudes,
    Outputs, PartError, PartGroup, PartRun, Plain,
};
use pool::Pool;
use report::push_event;
pub use report::{Column, PhasorValue, RunStats, SteadyStateReport, SwitchEvent, Timing};
pub use stages::Stages;
use steady::Steady;

use crate::control::{ControlError, Controls, MAX_LOOP_ITERATIONS};
use crate::element::{DefaultStress, Element, Stage, SwitchAction};
use crate::netlist::{self, ElementSignal, Term};

enum Probe {
    /// A linear combination of unknowns and element currents.
    Terms(Vec<Term>),
    /// A control signal, filled in when the row is written.
    Signal(usize),
}

/// The voltage above which a diode or controlled switch cannot be one
/// device: published data sheets of single thyristors, diodes and IGBTs end
/// at blocking voltages of 8.5 kV. Above it the element stands for a series
/// connection, for which the default resistances of one device are not meant
/// (W271).
pub const SINGLE_DEVICE_BLOCKING_VOLTAGE_V: f64 = 10e3;

/// Number of output rows buffered before flushing to the sink and reading the
/// output stopwatch once for the batch.
const OUTPUT_BATCH: usize = 256;

/// Working vectors of a run.
pub(in crate::sim) struct Work {
    b: Vec<f64>,
    x: Vec<f64>,
    row: Vec<f64>,
    measured: Vec<f64>,
    /// Buffered output rows handed to the sink in batches of `OUTPUT_BATCH`.
    out_times: Vec<f64>,
    out_values: Vec<f64>,
    /// The switching actions of the grid point being handled.
    actions: Vec<(String, SwitchAction)>,
    /// The control signals previewed at a time within a step.
    preview: Vec<f64>,
    /// The vertices of the carriers within a part (`Controls::vertices`).
    vertices: Vec<f64>,
    /// The currents of the links at their ports for the stage being solved,
    /// and the voltages at their ports in its solution.
    port_b: Vec<f64>,
    port_x: Vec<f64>,
    /// The solution of the first of two stages of a step whose parts are
    /// solved on their own (`plain_step`).
    x_first: Vec<f64>,
    /// The currents of the links at their ports for every stage of such a
    /// step, written when the step before it ended, and whether they are
    /// those of the step to be solved next.
    ahead: [Vec<f64>; 2],
    ahead_ready: bool,
    /// The outputs and measured quantities that the parts and links of such
    /// a step computed, before they are put in their places.
    staged: Vec<f64>,
    /// The states and the currents of the diodes at the grid point being
    /// handled, before its actions. Kept from step to step: a grid point
    /// asks for them in every step, and allocating the two vectors there
    /// took a fifth of the step of a circuit of four unknowns (214 against
    /// 172 ns for the buck converter of `tests/power_electronics.rs`).
    diode_states: Vec<bool>,
    diode_currents: Vec<f64>,
    /// Current zero-order-held controller overrides `(signal_index, value)`.
    overrides: Vec<(usize, f64)>,
    /// Scratch buffer for updates returned by one controller invocation.
    controller_updates: Vec<(usize, f64)>,
}

pub(in crate::sim) trait ControllerHook {
    const ENABLED: bool;
    fn call(
        &mut self,
        t: f64,
        columns: &[f64],
        signals: &[f64],
        updates: &mut Vec<(usize, f64)>,
    ) -> Result<(), String>;
}

struct NoController;

impl ControllerHook for NoController {
    const ENABLED: bool = false;
    #[inline(always)]
    fn call(
        &mut self,
        _t: f64,
        _columns: &[f64],
        _signals: &[f64],
        _updates: &mut Vec<(usize, f64)>,
    ) -> Result<(), String> {
        Ok(())
    }
}

struct FnController<C>(C);

impl<C> ControllerHook for FnController<C>
where
    C: FnMut(f64, &[f64], &[f64], &mut Vec<(usize, f64)>) -> Result<(), String>,
{
    const ENABLED: bool = true;
    #[inline]
    fn call(
        &mut self,
        t: f64,
        columns: &[f64],
        signals: &[f64],
        updates: &mut Vec<(usize, f64)>,
    ) -> Result<(), String> {
        (self.0)(t, columns, signals, updates)
    }
}

/// The factors of the system matrix, part by part.
struct Matrix {
    lu: Lu,
    /// The stage coefficient `lu` was computed for. When it is not the one
    /// of the regular stages, `lu` belongs to another matrix than theirs.
    alpha: f64,
}

/// What the switching actions at a grid point left.
#[derive(Default, Clone, Copy)]
struct GridPoint {
    /// A state changed.
    changed: bool,
    /// A diode stopped conducting because its current had crossed zero.
    opened_at_zero: bool,
    /// The factors already belong to the matrix of the new states (with the
    /// stage coefficient of the regular stages).
    fresh: bool,
}

/// A circuit ready to run once.
pub struct Simulator {
    elements: Vec<Box<dyn Element>>,
    steady_state_excluded: Vec<bool>,
    /// Name of every unknown, for error messages.
    labels: Vec<String>,
    /// Unknowns that are node voltages (named or inner nodes).
    voltage_unknowns: Vec<usize>,
    /// Unknowns that are branch currents.
    current_unknowns: Vec<usize>,
    probes: Vec<Probe>,
    columns: Vec<Column>,
    controls: Controls,
    /// Circuit quantities read by the measurement blocks. The terms of a
    /// quantity that is a signal of an element are empty.
    measurements: Vec<Vec<Term>>,
    /// The measured quantities that are signals of elements: their index
    /// among the measurements and among the element signals.
    measured_signals: Vec<(usize, usize)>,
    /// Elements whose state follows from the solution (diodes).
    diodes: Vec<usize>,
    /// Every element with two states (diodes, controlled switches and
    /// thyristors).
    two_state: Vec<usize>,
    /// Elements whose operating point follows from the solution of each
    /// stage.
    operating_points: Vec<usize>,
    /// Elements that read control signals at the start of a step.
    signal_readers: Vec<usize>,
    /// The internal quantities that elements offer as signals.
    element_signals: Vec<ElementSignal>,
    /// Where switching instants lie.
    switching: model::Switching,
    /// The zero-crossing functions of the elements, whose changes of sign
    /// are placed within a step.
    crossings: split::Crossings,
    /// The zero crossings found in the part of a step just solved: index of
    /// the function and fraction of the part.
    located: Vec<(usize, f64)>,
    /// An element keeps what every stage leaves (a travelling-wave line):
    /// a part that can be discarded needs `save` and `restore`.
    stage_history: bool,
    /// The stage coefficient of the regular stages, and the largest
    /// condition estimate of the matrices factorized with it so far.
    alpha_regular: f64,
    regular_condition: std::cell::Cell<f64>,
    /// The times at which elements act on their own, in ascending order,
    /// and the first of them that is still to come.
    action_times: Vec<f64>,
    next_action: usize,
    method: Method,
    h: f64,
    steps: u64,
    interval: u64,
    /// Largest node voltage and current of the solutions so far.
    peaks: (f64, f64),
    /// Frequency of the steady state to start from.
    init_frequency: Option<f64>,
    /// Named nodes and their unknowns.
    named_nodes: Vec<(String, usize)>,
    /// The parts of the circuit that are solved on their own, and the
    /// elements that join them.
    layout: Layout,
    /// Counts the changes of the matrix of every part (a switching action,
    /// a diode that changes its state, an operating point that moves a
    /// stamp): factors are those of the version they were computed for.
    versions: Vec<u64>,
    /// The elements in the order in which the components of the model made
    /// them, when that is not the order of `elements`.
    order: Option<Vec<usize>>,
    /// The entries of the links in the matrix of every part, for the stage
    /// coefficient of the regular stages, and cached for one other stage
    /// coefficient (half steps or a split part).
    boundary_regular: Vec<Vec<Entry>>,
    boundary_other: (f64, Vec<Vec<Entry>>),
    linear_solver: model::LinearSolver,
    /// Largest number of threads of the run; with `exact_threads`, the
    /// number to use whatever the size of the parts.
    threads: usize,
    exact_threads: bool,
    /// For a divided circuit: which part or link computes every output and
    /// measured quantity when the parts are solved on their own. `None` for
    /// a circuit that is one part.
    outputs: Option<Outputs>,
    /// Per part: what the thresholds of the diode states look at.
    magnitudes: Vec<Magnitudes>,
    /// The threads that solve parts side by side, and the first part (and
    /// the first link) of every group of them that is one task.
    pool: Option<Pool>,
    part_groups: Vec<usize>,
    link_groups: Vec<usize>,
    /// Steps without a switching action are solved by `plain_step`.
    plain: bool,
    /// The model asks for the division of the circuit.
    divide: bool,
    /// One part and no element with an operating point: a stage is one
    /// solution of one matrix.
    simple: bool,
    /// Measure control evaluation time (`Timing::control_s`).
    detail_timing: bool,
}

impl std::fmt::Debug for Simulator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Simulator")
            .field("unknowns", &self.labels)
            .field("columns", &self.columns)
            .field("method", &self.method)
            .field("time_step_s", &self.h)
            .field("steps", &self.steps)
            .finish()
    }
}

impl Simulator {
    /// Builds the circuit. The model is validated first; errors are returned
    /// as diagnostics.
    pub fn from_model(m: &Model) -> Result<Simulator, Vec<Diagnostic>> {
        let diagnostics = model::validate(m);
        if diagnostics.iter().any(|d| d.severity == Severity::Error) {
            return Err(diagnostics);
        }

        // The blocks written in the model, whose signals are recorded when
        // no outputs are given, and the model with the blocks of its
        // converters, which the solver runs (`netlist::build_divided` adds
        // them on its own).
        let written = model::signal_names(&m.controls);
        let net = netlist::build_divided(m);
        let expanded = model::expanded(m);
        let m = &*expanded;
        let h = m.simulation.time_step_s;
        let node_terms = |n: &str| vec![Term::Unknown(net.node_index[n], 1.0)];

        let mut measurements = Vec::new();
        let mut measured_signals = Vec::new();
        let mut measurement_slot = vec![0; m.controls.len()];
        let mut unknown = Vec::new();
        for (i, c) in m.controls.iter().enumerate() {
            measurement_slot[i] = measurements.len();
            let path = format!("controls[{i}].quantity");
            // The currents and voltages of a component are known here, where
            // the circuit is built: a choice of terminal and phase that
            // names none of them is reported now.
            let mut chosen = |measurements: &mut Vec<Vec<Term>>,
                              map: &HashMap<String, Vec<netlist::Quantity>>,
                              letter: &str,
                              component: &str,
                              suffix: String| {
                match measured_terms(map, letter, component, &suffix, &path) {
                    Ok(terms) => measurements.push(terms),
                    Err(d) => {
                        unknown.push(*d);
                        measurements.push(Vec::new());
                    }
                }
            };
            match c {
                model::Control::Measurement(meas) => match &meas.quantity {
                    model::Quantity::NodeVoltage { node } => measurements.push(node_terms(node)),
                    model::Quantity::BranchCurrent {
                        component,
                        terminal,
                        phase,
                    } => {
                        let suffix = model::Quantity::column_suffix(*terminal, phase.as_deref());
                        chosen(&mut measurements, &net.currents, "I", component, suffix);
                    }
                    model::Quantity::BranchVoltage { component, phase } => {
                        let suffix = model::Quantity::column_suffix(None, phase.as_deref());
                        chosen(&mut measurements, &net.voltages, "U", component, suffix);
                    }
                    model::Quantity::ComponentSignal { component, signal } => {
                        let name = format!("{component}.{signal}");
                        // Validation has compared the signal with those the
                        // component type declares.
                        let k = net
                            .roles
                            .signals
                            .iter()
                            .position(|s| s.name == name)
                            .expect("validated: the element offers the signal");
                        measured_signals.push((measurements.len(), k));
                        measurements.push(Vec::new());
                    }
                },
                model::Control::ThreePhaseMeasurement(meas) => match &meas.quantity {
                    model::ThreePhaseQuantity::BusVoltage { bus } => {
                        for node in model::BusRef::Bus(bus.clone()).nodes() {
                            measurements.push(node_terms(&node));
                        }
                    }
                    model::ThreePhaseQuantity::BranchCurrent {
                        component,
                        terminal,
                    } => {
                        for ph in model::PHASES {
                            let suffix = model::Quantity::column_suffix(*terminal, Some(ph));
                            chosen(&mut measurements, &net.currents, "I", component, suffix);
                        }
                    }
                },
                _ => {}
            }
        }
        if !unknown.is_empty() {
            // One diagnostic per block is enough.
            unknown.dedup_by(|a, b| a.path == b.path);
            return Err(unknown);
        }
        let controls = Controls::new(&m.controls, h, |i| measurement_slot[i]);
        let signal_names = written;

        let outputs: Vec<model::Output> = match &m.outputs {
            Some(o) => o.clone(),
            None => net
                .named_nodes
                .iter()
                .map(|n| model::Output::NodeVoltage { node: n.clone() })
                .chain(m.components.iter().map(|c| model::Output::BranchCurrent {
                    component: c.name().to_string(),
                }))
                .chain(
                    signal_names
                        .iter()
                        .map(|s| model::Output::Signal { signal: s.clone() }),
                )
                .collect(),
        };
        let mut probes = Vec::with_capacity(outputs.len());
        let mut columns = Vec::with_capacity(outputs.len());
        let node_column = |node: &str, probes: &mut Vec<Probe>, columns: &mut Vec<Column>| {
            probes.push(Probe::Terms(node_terms(node)));
            columns.push(Column {
                name: format!("V({node})"),
                kind: "node_voltage",
                unit: "V",
                target: node.to_string(),
            });
        };
        for o in &outputs {
            match o {
                model::Output::NodeVoltage { node } => node_column(node, &mut probes, &mut columns),
                model::Output::BusVoltage { bus } => {
                    for node in model::BusRef::Bus(bus.clone()).nodes() {
                        node_column(&node, &mut probes, &mut columns);
                    }
                }
                model::Output::BranchCurrent { component }
                | model::Output::BranchVoltage { component } => {
                    let (map, letter, kind, unit) = match o {
                        model::Output::BranchCurrent { .. } => {
                            (&net.currents, "I", "branch_current", "A")
                        }
                        _ => (&net.voltages, "U", "branch_voltage", "V"),
                    };
                    for q in &map[component.as_str()] {
                        probes.push(Probe::Terms(q.terms.clone()));
                        columns.push(Column {
                            name: format!("{letter}({component}{})", q.suffix),
                            kind,
                            unit,
                            target: component.clone(),
                        });
                    }
                }
                model::Output::Signal { signal } => {
                    probes.push(Probe::Signal(net.signal_index[signal.as_str()]));
                    columns.push(Column {
                        name: format!("S({signal})"),
                        kind: "signal",
                        unit: "",
                        target: signal.clone(),
                    });
                }
            }
        }

        let named_nodes = net
            .named_nodes
            .iter()
            .map(|n| (n.clone(), net.node_index[n.as_str()]))
            .collect();
        let init_frequency = match m.simulation.initialization {
            model::Initialization::SteadyState => model::source_frequency(m),
            model::Initialization::None => None,
        };
        let voltage_unknowns = (0..net.labels.len())
            .filter(|&i| net.is_voltage[i])
            .collect();
        let current_unknowns = (0..net.labels.len())
            .filter(|&i| !net.is_voltage[i])
            .collect();
        let layout = Layout::new(&net, m.simulation.linear_solver);
        let divided = !layout.is_whole();
        let order = divided.then(|| net.order.clone());
        let outputs = divided.then(|| {
            let quantities: Vec<Option<&[Term]>> = probes
                .iter()
                .map(|p| match p {
                    Probe::Terms(terms) => Some(terms.as_slice()),
                    Probe::Signal(_) => None,
                })
                .chain(measurements.iter().map(|terms| Some(terms.as_slice())))
                .collect();
            Outputs::new(&layout, &quantities)
        });
        debug_assert_eq!(layout.is_whole(), outputs.is_none());
        let magnitudes = layout
            .parts
            .iter()
            .map(|part| {
                let local = |voltage: bool| {
                    part.unknowns
                        .clone()
                        .filter(|&i| net.is_voltage[i] == voltage)
                        .map(|i| i - part.unknowns.start)
                        .collect()
                };
                Magnitudes {
                    voltages: local(true),
                    currents: local(false),
                    two_state: net
                        .two_state
                        .iter()
                        .filter(|e| part.elements.contains(e))
                        .map(|e| e - part.elements.start)
                        .collect(),
                }
            })
            .collect();
        let crossings = split::Crossings::of(&net.elements);
        let mut action_times: Vec<f64> =
            net.elements.iter().flat_map(|e| e.action_times()).collect();
        action_times.sort_by(f64::total_cmp);
        Ok(Simulator {
            versions: vec![0; layout.parts.len()],
            layout,
            order,
            boundary_regular: Vec::new(),
            boundary_other: (f64::NAN, Vec::new()),
            linear_solver: m.simulation.linear_solver,
            threads: m.simulation.threads(),
            exact_threads: false,
            outputs,
            magnitudes,
            pool: None,
            part_groups: Vec::new(),
            link_groups: Vec::new(),
            plain: true,
            divide: m.simulation.partitioning() != model::Partitioning::Off,
            simple: !divided && net.roles.operating_points.is_empty(),
            detail_timing: false,
            switching: m.simulation.switching(),
            crossings,
            stage_history: net.elements.iter().any(|e| e.decoupled_ends().is_some()),
            located: Vec::new(),
            alpha_regular: Stages::new(m.simulation.method, h).alpha(),
            regular_condition: std::cell::Cell::new(1.0),
            action_times,
            next_action: 0,
            elements: net.elements,
            steady_state_excluded: net.excluded_from_steady_state,
            labels: net.labels,
            voltage_unknowns,
            current_unknowns,
            probes,
            columns,
            controls,
            measurements,
            measured_signals,
            diodes: net.diodes,
            two_state: net.two_state,
            operating_points: net.roles.operating_points,
            signal_readers: net.roles.signal_readers,
            element_signals: net.roles.signals,
            method: m.simulation.method,
            h,
            steps: model::step_count(&m.simulation),
            interval: m.simulation.output_interval(),
            peaks: (0.0, 0.0),
            init_frequency,
            named_nodes,
        })
    }

    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    /// Names of all control signals in the order indexed by `updates`.
    pub fn signal_names(&self) -> &[String] {
        self.controls.names()
    }

    /// Number of unknowns of the circuit equations.
    pub fn unknowns(&self) -> usize {
        self.labels.len()
    }

    /// Number of unknowns of every part the circuit is divided into (one
    /// part: the whole circuit).
    pub fn parts(&self) -> Vec<usize> {
        let parts = &self.layout.parts;
        parts.iter().map(|p| p.unknowns.len()).collect()
    }

    /// Sets the largest number of threads of the run, instead of
    /// `simulation.threads` of the model: a caller that runs several cases
    /// side by side gives each of them one.
    pub fn set_threads(&mut self, threads: usize) {
        self.threads = threads.max(1);
        self.exact_threads = false;
    }

    /// Makes the run use `threads` threads (or one per part, when the parts
    /// are fewer) whatever the size of the parts and the number of cores of
    /// the machine: for measurements of where threads pay off, and for
    /// tests.
    pub fn use_threads(&mut self, threads: usize) {
        self.threads = threads.max(1);
        self.exact_threads = true;
    }

    /// Measures the time spent evaluating control blocks (`Timing::control_s`).
    pub fn with_detail_timing(mut self, enabled: bool) -> Self {
        self.detail_timing = enabled;
        self
    }

    /// Solves every step stage by stage over the whole circuit, as the
    /// steps with switching actions are solved.
    #[cfg(test)]
    pub(crate) fn without_plain_steps(mut self) -> Self {
        self.plain = false;
        self
    }

    /// The number of threads that solve the parts of this circuit: at most
    /// `threads` and what the machine offers, and one when the parts are too
    /// few or too small for more to gain (`THREAD_WORK`).
    fn threads_of_run(&self) -> usize {
        if self.layout.is_whole() || !self.plain {
            return 1;
        }
        if self.exact_threads {
            return self.threads.min(self.layout.parts.len());
        }
        let work: usize = self.layout.parts.iter().map(parts::work).sum();
        self.threads
            .min(pool::available_threads())
            .min(self.layout.parts.len())
            .min(work / parts::THREAD_WORK)
            .max(1)
    }

    /// Names of the internal quantities that elements offer as signals,
    /// each `<element name>.<signal name>`.
    pub fn element_signals(&self) -> impl Iterator<Item = &str> {
        self.element_signals.iter().map(|s| s.name.as_str())
    }

    /// Value of element signal `k` (in the order of `element_signals`) at
    /// the grid point the run has reached.
    fn element_signal(&self, k: usize) -> f64 {
        let s = &self.element_signals[k];
        self.elements[s.element].signal(s.index)
    }

    /// The `k`-th element in the order in which the components of the model
    /// made them: the order in which elements act and are named.
    #[inline]
    fn made(&self, k: usize) -> usize {
        match &self.order {
            Some(order) => order[k],
            None => k,
        }
    }

    /// Notes that the matrix of the part of element `e` changed.
    #[inline]
    fn touch(&mut self, e: usize) {
        if let Some(p) = self.layout.part_of(e) {
            self.versions[p] += 1;
        }
    }

    /// Asks every element for the switching actions it takes at time `t`
    /// (`Element::pre_step_changed`), in the order of the model.
    fn pre_step(&mut self, t: f64, tol: f64, actions: &mut Vec<(String, SwitchAction)>) -> bool {
        if self.layout.is_whole() {
            // One part: every action changes its matrix.
            let mut any_changed = false;
            for e in self.elements.iter_mut() {
                if e.pre_step_changed(t, tol, actions) {
                    any_changed = true;
                }
            }
            if any_changed {
                self.versions[0] += 1;
            }
            return any_changed;
        }
        let mut any_changed = false;
        for k in 0..self.elements.len() {
            let e = self.made(k);
            if self.elements[e].pre_step_changed(t, tol, actions) {
                self.touch(e);
                any_changed = true;
            }
        }
        any_changed
    }

    /// True when the solution `x` contradicts the state of element `e`.
    #[inline]
    fn inconsistent(&self, e: usize, x: &[f64], tol: &crate::element::Tolerance) -> bool {
        self.layout
            .with(e, x, |x| self.elements[e].inconsistent(x, tol))
    }

    /// Finds again what the solver asks only some elements for, after a
    /// test has replaced elements.
    #[cfg(test)]
    fn refresh_roles(&mut self) {
        let roles = netlist::Roles::of(&self.elements);
        self.operating_points = roles.operating_points;
        self.layout.find_operating_points(&self.elements);
        self.simple = self.layout.is_whole() && self.operating_points.is_empty();
        self.signal_readers = roles.signal_readers;
        self.element_signals = roles.signals;
    }

    /// Adds what the solution of parts counted to the statistics.
    fn count(&self, counts: &Counts, stats: &mut RunStats) {
        stats.factorizations += counts.factorizations;
        stats.solves += counts.solves;
        stats.max_condition_estimate = stats.max_condition_estimate.max(counts.condition);
        self.regular_condition
            .set(self.regular_condition.get().max(counts.regular_condition));
        stats.operating_point_iterations += counts.iterations;
        stats.max_operating_point_iterations = stats
            .max_operating_point_iterations
            .max(counts.max_iterations);
    }

    /// The error of part `p` as an error of the run at time `t`.
    fn part_error(&self, p: usize, error: PartError, t: f64) -> SimError {
        let label = |k: usize| self.labels[self.layout.parts[p].unknowns.start + k].clone();
        match error {
            PartError::Singular(k) => SimError::Singular {
                time_s: t,
                unknown: label(k),
            },
            PartError::NonFinite(k) => SimError::NonFinite {
                time_s: t,
                unknown: label(k),
            },
            PartError::NotConverged {
                components,
                iterations,
            } => SimError::OperatingPointNotConverged {
                time_s: t,
                components,
                iterations,
            },
        }
    }

    /// Ensures `boundary_other` holds the link boundary entries for `alpha`
    /// when `alpha` differs from `alpha_regular` and there are links.
    fn ensure_boundary(&mut self, alpha: f64) -> bool {
        let use_other = alpha != self.alpha_regular && !self.layout.links.is_empty();
        if use_other && self.boundary_other.0 != alpha {
            self.boundary_other = (alpha, self.layout.boundary(&self.elements, alpha));
        }
        use_other
    }

    /// Makes `lu` hold, and use, the factors for `alpha` of the present
    /// states of every part. Parts whose factors are still those of their
    /// states are not factorized again.
    fn refresh(
        &mut self,
        lu: &mut Lu,
        alpha: f64,
        t: f64,
        stats: &mut RunStats,
    ) -> Result<(), SimError> {
        let started = Stopwatch::start();
        let use_other = self.ensure_boundary(alpha);
        let boundary = if use_other {
            &self.boundary_other.1
        } else {
            &self.boundary_regular
        };
        let mut counts = Counts::default();
        let mut failed = None;
        for (p, part) in self.layout.parts.iter().enumerate() {
            let mut run = PartRun {
                part,
                elements: &mut self.elements[part.elements.clone()],
                lu: &mut lu.parts[p],
                version: &mut self.versions[p],
                boundary: &boundary[p],
                alpha_regular: self.alpha_regular,
            };
            if let Err(k) = run.refresh(alpha, &mut counts) {
                failed = Some((p, PartError::Singular(k)));
                break;
            }
        }
        self.count(&counts, stats);
        stats.timing.factorize_s += started.elapsed_s();
        match failed {
            Some((p, e)) => Err(self.part_error(p, e, t)),
            None => Ok(()),
        }
    }

    /// Writes the currents of the links at their ports for `stage` at time
    /// `t` into `port_b`.
    fn port_currents(&self, stage: &Stage, t: f64, port_b: &mut [f64]) {
        for link in &self.layout.links {
            let b = &mut port_b[link.buffer.clone()];
            b.fill(0.0);
            self.elements[link.element].rhs(t, stage, b);
        }
    }

    /// Solves `stage` at time `t` into `w.x`: every part until the
    /// operating points of its elements agree with its solution.
    #[inline]
    fn solve(
        &mut self,
        lu: &mut Lu,
        stage: &Stage,
        t: f64,
        w: &mut Work,
        stats: &mut RunStats,
    ) -> Result<(), SimError> {
        if !self.simple {
            return self.solve_parts(lu, stage, t, w, stats);
        }
        self.solve_whole(lu.parts[0].active(), stage, t, &mut w.b, &mut w.x, stats)
    }

    /// `solve` for a circuit that is one part without elements that
    /// iterate: the right-hand side and one solution, without the
    /// bookkeeping of the parts.
    fn solve_whole(
        &self,
        lu: &crate::linalg::Factors<f64>,
        stage: &Stage,
        t: f64,
        b: &mut [f64],
        x: &mut [f64],
        stats: &mut RunStats,
    ) -> Result<(), SimError> {
        b.fill(0.0);
        let res = solve_and_check(lu, &self.elements, stage, t, b, x);
        stats.solves += 1;
        res.map_err(|k| SimError::NonFinite {
            time_s: t,
            unknown: self.labels[k].clone(),
        })
    }

    /// `solve` for a circuit of several parts or with elements that have an
    /// operating point: part by part (`PartRun::stage`).
    #[inline(never)]
    fn solve_parts(
        &mut self,
        lu: &mut Lu,
        stage: &Stage,
        t: f64,
        w: &mut Work,
        stats: &mut RunStats,
    ) -> Result<(), SimError> {
        self.port_currents(stage, t, &mut w.port_b);
        w.ahead_ready = false;
        let use_other = !self.operating_points.is_empty() && self.ensure_boundary(stage.alpha);
        let boundary = if use_other {
            &self.boundary_other.1
        } else {
            &self.boundary_regular
        };
        let mut counts = Counts::default();
        let mut failed = None;
        for (p, part) in self.layout.parts.iter().enumerate() {
            let unknowns = part.unknowns.clone();
            let mut run = PartRun {
                part,
                elements: &mut self.elements[part.elements.clone()],
                lu: &mut lu.parts[p],
                version: &mut self.versions[p],
                boundary: &boundary[p],
                alpha_regular: self.alpha_regular,
            };
            let (b, x) = (&mut w.b[unknowns.clone()], &mut w.x[unknowns]);
            if let Err(e) = run.stage(stage, t, &w.port_b, b, x, &mut counts) {
                failed = Some((p, e));
                break;
            }
        }
        stats.solves += 1;
        if !counts.is_empty() {
            self.count(&counts, stats);
        }
        match failed {
            Some((p, e)) => Err(self.part_error(p, e, t)),
            None => Ok(()),
        }
    }

    /// One time step from t0 to t1 without a switching action at its start,
    /// in a divided circuit: every part solves all `stages` of the step and
    /// advances its elements, on its own; then every link is advanced over
    /// the stages and writes the currents at its ports for the stages of the
    /// step that follows (to `t2`). The parts, and then the links, are run
    /// in groups by the threads of the pool.
    ///
    /// This rests on what a link reads: in a step that no action precedes,
    /// its currents of every stage follow from solutions up to the start of
    /// the step (`Element::decoupled_ends`). A part therefore reads nothing
    /// that another part writes during the step, and the result is the same
    /// in any order and on any number of threads. Steps with a switching
    /// action, and parts of split steps, are solved stage by stage over the
    /// whole circuit (`attempt`).
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn plain_step(
        &mut self,
        lu: &mut Lu,
        stages: &[Stage],
        t0: f64,
        t1: f64,
        t2: f64,
        w: &mut Work,
        stats: &mut RunStats,
    ) -> Result<(), SimError> {
        /// Takes the first `n` of `slice`.
        fn take<'a, T>(slice: &mut &'a mut [T], n: usize) -> &'a mut [T] {
            let (head, tail) = std::mem::take(slice).split_at_mut(n);
            *slice = tail;
            head
        }
        let count = stages.len();
        let time = |from: f64, to: f64, k: usize| {
            if k + 1 == count {
                to
            } else {
                from + stages[k].time_fraction * (to - from)
            }
        };
        stats.steps_solved_by_parts += 1;
        let (mut times, mut next_times) = ([t1; 2], [t2; 2]);
        for k in 0..count {
            times[k] = time(t0, t1, k);
            next_times[k] = time(t1, t2, k);
        }
        if !w.ahead_ready {
            for k in 0..count {
                self.port_currents(&stages[k], times[k], &mut w.ahead[k]);
            }
        }
        let layout = &self.layout;
        let outputs = self.outputs.as_ref().expect("a divided circuit");
        let (part_elements, link_elements) = self.elements.split_at_mut(layout.links_from);
        let staged_of_parts = outputs.staged_of_parts.last().map_or(0, |r| r.end);
        let (staged_parts, staged_links) = w.staged.split_at_mut(staged_of_parts);

        // The parts.
        let plain = Plain {
            stages,
            times,
            port_b: [&w.ahead[0], &w.ahead[1]],
            boundary: &self.boundary_regular,
            alpha_regular: self.alpha_regular,
            outputs,
            magnitudes: (!self.diodes.is_empty()).then_some(&self.magnitudes[..]),
        };
        let mut counts = Counts::default();
        let mut failed = None;
        if self.part_groups.len() == 2 {
            let mut group = PartGroup {
                first_part: 0,
                parts: &layout.parts,
                elements: part_elements,
                lu: &mut lu.parts,
                versions: &mut self.versions,
                b: &mut w.b,
                x_first: &mut w.x_first,
                x: &mut w.x,
                staged: staged_parts,
                counts: Counts::default(),
                error: None,
            };
            group.run(&plain);
            counts = group.counts;
            failed = group.error;
        } else {
            let mut groups = Vec::with_capacity(self.part_groups.len() - 1);
            {
                let mut elements = part_elements;
                let mut lus = &mut lu.parts[..];
                let mut versions = &mut self.versions[..];
                let (mut b, mut x_first, mut x) = (&mut w.b[..], &mut w.x_first[..], &mut w.x[..]);
                let mut staged = staged_parts;
                for bounds in self.part_groups.windows(2) {
                    let parts = &layout.parts[bounds[0]..bounds[1]];
                    let (first, last) = (&parts[0], &parts[parts.len() - 1]);
                    let unknowns = last.unknowns.end - first.unknowns.start;
                    let values = outputs.staged_of_parts[bounds[1] - 1].end
                        - outputs.staged_of_parts[bounds[0]].start;
                    groups.push(std::sync::Mutex::new(PartGroup {
                        first_part: bounds[0],
                        parts,
                        elements: take(&mut elements, last.elements.end - first.elements.start),
                        lu: take(&mut lus, parts.len()),
                        versions: take(&mut versions, parts.len()),
                        b: take(&mut b, unknowns),
                        x_first: take(&mut x_first, unknowns),
                        x: take(&mut x, unknowns),
                        staged: take(&mut staged, values),
                        counts: Counts::default(),
                        error: None,
                    }));
                }
            }
            let run = |g: usize| groups[g].lock().expect("no other thread").run(&plain);
            match &self.pool {
                Some(pool) if groups.len() > 1 => pool.run(groups.len(), &run),
                _ => (0..groups.len()).for_each(run),
            }
            for group in groups {
                let group = group.into_inner().expect("no other thread");
                counts.merge(&group.counts);
                if failed.is_none() {
                    failed = group.error;
                }
            }
        }
        if let Some((p, error, t)) = failed {
            // What the other parts counted in this step depends on how far
            // they came: it is left out.
            return Err(self.part_error(p, error, t));
        }

        // The links.
        if !layout.links.is_empty() {
            let after = AfterParts {
                stages,
                times,
                next_times,
                x: [if count == 1 { &w.x } else { &w.x_first }, &w.x],
                outputs,
            };
            let [ahead_first, ahead_last] = &mut w.ahead;
            if self.link_groups.len() == 2 {
                let mut group = LinkGroup {
                    first_link: 0,
                    links: &layout.links,
                    elements: link_elements,
                    port_x: &mut w.port_x,
                    port_b: [&mut ahead_first[..], &mut ahead_last[..]],
                    staged: staged_links,
                };
                group.run(&after);
            } else {
                let mut groups = Vec::with_capacity(self.link_groups.len() - 1);
                let mut elements = link_elements;
                let mut port_x = &mut w.port_x[..];
                let (mut ahead_first, mut ahead_last) = (&mut ahead_first[..], &mut ahead_last[..]);
                let mut staged = staged_links;
                for bounds in self.link_groups.windows(2) {
                    let links = &layout.links[bounds[0]..bounds[1]];
                    let ports = links[links.len() - 1].buffer.end - links[0].buffer.start;
                    let values = outputs.staged_of_links[bounds[1] - 1].end
                        - outputs.staged_of_links[bounds[0]].start;
                    groups.push(std::sync::Mutex::new(LinkGroup {
                        first_link: bounds[0],
                        links,
                        elements: take(&mut elements, links.len()),
                        port_x: take(&mut port_x, ports),
                        port_b: [take(&mut ahead_first, ports), take(&mut ahead_last, ports)],
                        staged: take(&mut staged, values),
                    }));
                }
                let run = |g: usize| groups[g].lock().expect("no other thread").run(&after);
                match &self.pool {
                    Some(pool) if groups.len() > 1 => pool.run(groups.len(), &run),
                    _ => (0..groups.len()).for_each(run),
                }
            }
        }
        w.ahead_ready = true;

        // The outputs and measured quantities, in their places.
        let rows = self.probes.len();
        for (&v, &destination) in w.staged.iter().zip(&outputs.destinations) {
            if destination < rows {
                w.row[destination] = v;
            } else {
                w.measured[destination - rows] = v;
            }
        }
        for (destination, terms) in &outputs.of_circuit {
            let v = eval_terms(terms, &w.x, |_, _| unreachable!("currents have an owner"));
            if *destination < rows {
                w.row[*destination] = v;
            } else {
                w.measured[*destination - rows] = v;
            }
        }
        stats.solves += count as u64;
        if !counts.is_empty() {
            self.count(&counts, stats);
        }
        if !self.diodes.is_empty() {
            self.peaks = (
                self.peaks.0.max(counts.peaks.0),
                self.peaks.1.max(counts.peaks.1),
            );
        }
        self.measure_signals(w);
        Ok(())
    }

    /// Updates the states of every element from the solution `w.x` of
    /// `stage`: the parts, then the links.
    #[inline]
    fn advance(&mut self, stage: &Stage, w: &mut Work) {
        if self.layout.is_whole() {
            for e in self.elements.iter_mut() {
                e.advance(stage, &w.x);
            }
            return;
        }
        self.advance_parts(stage, w);
    }

    /// `advance` for a divided circuit.
    #[inline(never)]
    fn advance_parts(&mut self, stage: &Stage, w: &mut Work) {
        for part in &self.layout.parts {
            let x = &w.x[part.unknowns.clone()];
            for e in &mut self.elements[part.elements.clone()] {
                e.advance(stage, x);
            }
        }
        for link in &self.layout.links {
            let x = &mut w.port_x[link.buffer.clone()];
            for (v, &unknown) in x.iter_mut().zip(&link.ports) {
                *v = w.x[unknown];
            }
            self.elements[link.element].advance(stage, x);
        }
    }

    /// Records the outputs and the measured quantities of a stage solution.
    /// Call before `advance`. The signal columns are filled by `emit`.
    ///
    /// Whether the circuit is divided is asked once per row, not once per
    /// current (`Layout::with`): asked in the innermost loop, it cost 2 % of
    /// the step of a circuit of four unknowns (219 against 215 ns for the
    /// buck converter of `tests/power_electronics.rs`).
    fn record(&self, t: f64, stage: &Stage, w: &mut Work) {
        if self.layout.is_whole() {
            self.record_by(w, |e, k, x| self.elements[e].current(k, t, stage, x));
        } else {
            self.record_by(w, |e, k, x| {
                self.layout
                    .with(e, x, |x| self.elements[e].current(k, t, stage, x))
            });
        }
    }

    /// `record` with `current(e, k, x)`, the `k`-th current of element `e`
    /// in the solution `x` over the unknowns of the circuit.
    #[inline(always)]
    fn record_by(&self, w: &mut Work, current: impl Fn(usize, usize, &[f64]) -> f64) {
        let x = &w.x[..];
        let value = |terms: &[Term]| eval_terms(terms, x, |e, k| current(e, k, x));
        for (p, out) in self.probes.iter().zip(w.row.iter_mut()) {
            if let Probe::Terms(terms) = p {
                *out = value(terms);
            }
        }
        for (terms, out) in self.measurements.iter().zip(w.measured.iter_mut()) {
            *out = value(terms);
        }
    }

    /// Fills the signal columns, buffers the row, and flushes to the sink in
    /// batches of `OUTPUT_BATCH`.
    fn emit<F>(
        &self,
        t: f64,
        w: &mut Work,
        sink: &mut F,
        stats: &mut RunStats,
    ) -> Result<(), SimError>
    where
        F: FnMut(f64, &[f64]) -> std::io::Result<()>,
    {
        for (p, out) in self.probes.iter().zip(w.row.iter_mut()) {
            if let Probe::Signal(i) = *p {
                *out = self.controls.values[i];
            }
        }
        w.out_times.push(t);
        w.out_values.extend_from_slice(&w.row);
        if w.out_times.len() >= OUTPUT_BATCH {
            Self::flush_output(w, sink, stats)?;
        }
        Ok(())
    }

    /// Hands any buffered output rows to `sink`, timing the batch as a whole.
    fn flush_output<F>(w: &mut Work, sink: &mut F, stats: &mut RunStats) -> Result<(), SimError>
    where
        F: FnMut(f64, &[f64]) -> std::io::Result<()>,
    {
        if w.out_times.is_empty() {
            return Ok(());
        }
        let started = Stopwatch::start();
        let cols = w.row.len();
        let mut written = 0;
        let mut err = None;
        if cols == 0 {
            for &t in &w.out_times {
                if let Err(e) = sink(t, &[]) {
                    err = Some(e);
                    break;
                }
                written += 1;
            }
        } else {
            for (&t, row) in w.out_times.iter().zip(w.out_values.chunks_exact(cols)) {
                if let Err(e) = sink(t, row) {
                    err = Some(e);
                    break;
                }
                written += 1;
            }
        }
        w.out_times.clear();
        w.out_values.clear();
        stats.rows += written;
        stats.timing.output_s += started.elapsed_s();
        match err {
            Some(e) => Err(SimError::Output(e)),
            None => Ok(()),
        }
    }

    /// Solves the stages of one step from t0 to t1 with fixed switch states.
    /// Each stage is solved until the operating points agree with its
    /// solution, which replaces `lu` when a stamp changed.
    fn attempt(
        &mut self,
        lu: &mut Lu,
        stages: &[Stage],
        t0: f64,
        t1: f64,
        w: &mut Work,
        stats: &mut RunStats,
    ) -> Result<(), SimError> {
        self.attempt_located(lu, stages, t0, t1, false, false, None, w, stats)
            .map(|_| ())
    }

    /// Reads the measured quantities that are signals of elements. They
    /// belong to the point the elements have been advanced to: call after
    /// `advance` of the last stage.
    fn measure_signals(&self, w: &mut Work) {
        for &(slot, k) in &self.measured_signals {
            w.measured[slot] = self.element_signal(k);
        }
    }

    fn map_control_error(t: f64, e: ControlError) -> SimError {
        match e {
            ControlError::LoopNotConverged { blocks } => SimError::ControlLoopNotConverged {
                time_s: t,
                blocks,
                iterations: MAX_LOOP_ITERATIONS,
            },
            ControlError::NotFinite { signal, block_type } => SimError::SignalNotFinite {
                time_s: t,
                signal,
                block_type,
            },
        }
    }

    /// Evaluates the control blocks at the grid point `t`, on the measured
    /// quantities of its solution.
    fn evaluate_controls(
        &mut self,
        t: f64,
        measured: &[f64],
        stats: &mut RunStats,
    ) -> Result<(), SimError> {
        let started = self.detail_timing.then(Stopwatch::start);
        let result = self.controls.evaluate(t, measured);
        stats.control_loop_iterations = self.controls.loop_iterations;
        if let Some(started) = started {
            stats.timing.control_s += started.elapsed_s();
        }
        result.map_err(|e| Self::map_control_error(t, e))
    }

    /// Evaluates controls at grid point `t`, optionally invoking `controller`
    /// after previewing fresh measurements and upstream blocks, and holding
    /// the latest overrides across intermediate ZOH steps.
    fn evaluate_controls_with_controller<C: ControllerHook>(
        &mut self,
        t: f64,
        call_controller: bool,
        w: &mut Work,
        stats: &mut RunStats,
        controller: &mut C,
    ) -> Result<(), SimError> {
        let started = self.detail_timing.then(Stopwatch::start);
        let result = if call_controller {
            if !self.controls.is_empty() {
                self.controls
                    .evaluate_preview_state(t, &w.measured, &w.overrides)
                    .map_err(|e| Self::map_control_error(t, e))?;
            }
            for (p, out) in self.probes.iter().zip(w.row.iter_mut()) {
                if let Probe::Signal(i) = *p {
                    *out = self.controls.values[i];
                }
            }
            w.controller_updates.clear();
            controller
                .call(t, &w.row, &self.controls.values, &mut w.controller_updates)
                .map_err(|message| SimError::Controller { time_s: t, message })?;
            let n_signals = self.controls.names().len();
            for &(sig_idx, val) in &w.controller_updates {
                if sig_idx >= n_signals {
                    return Err(SimError::Controller {
                        time_s: t,
                        message: format!(
                            "signal index {sig_idx} is out of range (model has {n_signals} control signals)"
                        ),
                    });
                }
                if !val.is_finite() {
                    return Err(SimError::Controller {
                        time_s: t,
                        message: format!(
                            "controller returned non-finite value ({val}) for signal \"{}\"",
                            self.controls.names()[sig_idx]
                        ),
                    });
                }
                if let Some(existing) = w.overrides.iter_mut().find(|(idx, _)| *idx == sig_idx) {
                    existing.1 = val;
                } else {
                    w.overrides.push((sig_idx, val));
                }
            }
            if self.controls.is_empty() {
                Ok(())
            } else {
                self.controls
                    .evaluate_with_overrides(t, &w.measured, &w.overrides)
                    .map_err(|e| Self::map_control_error(t, e))
            }
        } else if !self.controls.is_empty() {
            self.controls
                .evaluate_with_overrides(t, &w.measured, &w.overrides)
                .map_err(|e| Self::map_control_error(t, e))
        } else {
            Ok(())
        };
        stats.control_loop_iterations = self.controls.loop_iterations;
        if let Some(started) = started {
            stats.timing.control_s += started.elapsed_s();
        }
        result
    }

    /// Like `attempt`, for a part of a step in which zero crossings are
    /// placed (`locate`). The zero-crossing functions are evaluated at the
    /// start of the part (when `start_known`: `w.x` is the solution there
    /// and no action followed it), after the first stage and at the end;
    /// the crossings found are left in `self.located` and the earliest
    /// fraction is returned. When `stop_before` is given and that fraction
    /// lies before it, the last stage is not advanced: the part is to be
    /// solved again up to the crossing.
    #[allow(clippy::too_many_arguments)]
    fn attempt_located(
        &mut self,
        lu: &mut Lu,
        stages: &[Stage],
        t0: f64,
        t1: f64,
        locate: bool,
        start_known: bool,
        stop_before: Option<f64>,
        w: &mut Work,
        stats: &mut RunStats,
    ) -> Result<Option<f64>, SimError> {
        let locate = locate && !self.crossings.is_empty();
        let mut found: Option<f64> = None;
        let mut stage_fraction = None;
        if locate {
            self.located.clear();
            if start_known && !self.crossings.start_valid {
                self.crossings
                    .evaluate(&self.elements, &self.layout, &w.x, None, split::At::Start);
            }
        } else {
            self.crossings.start_valid = false;
        }
        for (k, stage) in stages.iter().enumerate() {
            let last = k + 1 == stages.len();
            let t = if last {
                t1
            } else {
                t0 + stage.time_fraction * (t1 - t0)
            };
            self.solve(lu, stage, t, w, stats)?;
            if locate && k == 0 && !last {
                self.crossings
                    .evaluate(&self.elements, &self.layout, &w.x, None, split::At::Stage);
                stage_fraction = Some(stage.time_fraction);
            }
            if last {
                if locate {
                    let tol = self.tolerance(&w.x, lu);
                    self.crossings.evaluate(
                        &self.elements,
                        &self.layout,
                        &w.x,
                        Some(&tol),
                        split::At::End,
                    );
                    self.crossings
                        .locate(start_known, stage_fraction, &mut self.located);
                    found = self.located.iter().map(|l| l.1).reduce(f64::min);
                    if let (Some(theta), Some(limit)) = (found, stop_before) {
                        if theta < limit {
                            return Ok(found);
                        }
                    }
                    self.crossings.accept();
                }
                self.record(t, stage, w);
                if !self.diodes.is_empty() {
                    let (v, i) = self.magnitudes(&w.x);
                    self.peaks = (self.peaks.0.max(v), self.peaks.1.max(i));
                }
            }
            self.advance(stage, w);
            if last {
                self.measure_signals(w);
            }
        }
        Ok(found)
    }

    /// Runs the simulation. `sink` receives the time and the recorded values
    /// of every output row, starting with t = 0.
    pub fn run<F>(self, sink: F) -> Result<RunStats, SimError>
    where
        F: FnMut(f64, &[f64]) -> std::io::Result<()>,
    {
        let (stats, result) = self.run_with_stats(sink);
        result.map(|_| stats)
    }

    /// Like `run`, but a run that fails also returns what it counted up to
    /// the failure: the switching events before a singular matrix are often
    /// its cause.
    pub fn run_with_stats<F>(self, sink: F) -> (RunStats, Result<(), SimError>)
    where
        F: FnMut(f64, &[f64]) -> std::io::Result<()>,
    {
        self.run_with_stats_inner(sink, 1, NoController)
    }

    /// Runs the simulation with an external controller called at `t = 0` and
    /// every `interval_steps` steps, holding its latest signal overrides with
    /// zero-order hold in between.
    pub fn run_with_controller<F, C>(
        self,
        sink: F,
        interval_steps: u64,
        controller: C,
    ) -> Result<RunStats, SimError>
    where
        F: FnMut(f64, &[f64]) -> std::io::Result<()>,
        C: FnMut(f64, &[f64], &[f64], &mut Vec<(usize, f64)>) -> Result<(), String>,
    {
        let (stats, result) = self.run_with_stats_and_controller(sink, interval_steps, controller);
        result.map(|_| stats)
    }

    /// Like `run_with_controller`, returning statistics even if the run fails.
    pub fn run_with_stats_and_controller<F, C>(
        self,
        sink: F,
        interval_steps: u64,
        controller: C,
    ) -> (RunStats, Result<(), SimError>)
    where
        F: FnMut(f64, &[f64]) -> std::io::Result<()>,
        C: FnMut(f64, &[f64], &[f64], &mut Vec<(usize, f64)>) -> Result<(), String>,
    {
        if interval_steps == 0 {
            return (
                RunStats::default(),
                Err(SimError::Controller {
                    time_s: 0.0,
                    message: "control interval_steps must be at least 1".to_string(),
                }),
            );
        }
        self.run_with_stats_inner(sink, interval_steps, FnController(controller))
    }

    fn run_with_stats_inner<F, C>(
        mut self,
        mut sink: F,
        interval_steps: u64,
        mut controller: C,
    ) -> (RunStats, Result<(), SimError>)
    where
        F: FnMut(f64, &[f64]) -> std::io::Result<()>,
        C: ControllerHook,
    {
        let started = Stopwatch::start();
        let mut stats = RunStats::default();
        let result =
            self.run_loop_with_controller(&mut sink, interval_steps, &mut controller, &mut stats);
        let t = &mut stats.timing;
        t.total_s = started.elapsed_s();
        t.solve_s = (t.total_s - t.factorize_s - t.output_s).max(0.0);
        if self.detail_timing {
            LAST_DETAIL_CONTROL_S.with(|c| c.set(t.control_s));
        }
        (stats, result)
    }

    /// Rounding allowed when a time is compared with a grid point: a small
    /// part of the step, or the rounding of the grid time itself in a long
    /// run.
    fn time_tolerance(&self, t: f64) -> f64 {
        (1e-9 * self.h).max(4.0 * f64::EPSILON * t)
    }

    #[cfg(test)]
    fn run_loop<F>(&mut self, sink: &mut F, stats: &mut RunStats) -> Result<(), SimError>
    where
        F: FnMut(f64, &[f64]) -> std::io::Result<()>,
    {
        self.run_loop_with_controller(sink, 1, &mut NoController, stats)
    }

    fn run_loop_with_controller<F, C>(
        &mut self,
        sink: &mut F,
        interval_steps: u64,
        controller: &mut C,
        stats: &mut RunStats,
    ) -> Result<(), SimError>
    where
        F: FnMut(f64, &[f64]) -> std::io::Result<()>,
        C: ControllerHook,
    {
        if std::env::var_os("OPENKEITO_DETAIL_TIMING").is_some() {
            self.detail_timing = true;
        }
        let h = self.h;
        let n = self.labels.len();
        stats.unknowns = n;
        stats.linear_solver = self.layout.parts[0].solver.name();
        if self
            .layout
            .parts
            .iter()
            .any(|p| p.solver.name() != stats.linear_solver)
        {
            stats.linear_solver = "mixed";
        }

        let stages = Stages::new(self.method, h);
        for k in 0..self.elements.len() {
            let made = self.made(k);
            let e = &mut self.elements[made];
            e.prepare(&stages)
                .map_err(|detail| SimError::IllConditioned {
                    component: e.name().to_string(),
                    detail,
                })?;
        }
        self.boundary_regular = self.layout.boundary(&self.elements, stages.alpha());
        let mut w = Work {
            b: vec![0.0; n],
            x: vec![0.0; n],
            row: vec![0.0; self.probes.len()],
            measured: vec![0.0; self.measurements.len()],
            out_times: Vec::with_capacity(OUTPUT_BATCH),
            out_values: Vec::with_capacity(OUTPUT_BATCH * self.probes.len()),
            actions: Vec::new(),
            preview: vec![0.0; self.controls.values.len()],
            vertices: Vec::new(),
            port_b: vec![0.0; self.layout.port_count],
            port_x: vec![0.0; self.layout.port_count],
            x_first: vec![0.0; if !self.layout.is_whole() { n } else { 0 }],
            ahead: [
                vec![0.0; self.layout.port_count],
                vec![0.0; self.layout.port_count],
            ],
            ahead_ready: false,
            staged: vec![0.0; self.outputs.as_ref().map_or(0, |o| o.destinations.len())],
            diode_states: Vec::with_capacity(self.diodes.len()),
            diode_currents: Vec::with_capacity(self.diodes.len()),
            overrides: Vec::new(),
            controller_updates: Vec::new(),
        };
        // The threads of the run and their tasks: several groups of parts
        // per thread, so that a thread on a slower core holds up less.
        self.pool = Pool::new(self.threads_of_run());
        let threads = self.pool.as_ref().map_or(1, Pool::threads);
        let tasks = if threads > 1 { 4 * threads } else { 1 };
        self.part_groups = parts::balanced(self.layout.parts.iter().map(parts::work), tasks);
        self.link_groups = parts::balanced(self.layout.links.iter().map(|_| 1), tasks);
        stats.parts = self.layout.parts.len();
        stats.largest_part_unknowns = self.parts().into_iter().max().unwrap_or(0);
        stats.links = self.layout.links.len();
        stats.threads = threads;

        // The steady state before t = 0: switches in their initial state,
        // faults not started. Actions at t = 0 then take effect as usual.
        let mut steady = match self.init_frequency {
            Some(f) => Some(self.steady_state(f, stats)?),
            None => None,
        };

        let run_result = self.run_steps(
            &stages,
            &mut w,
            &mut steady,
            sink,
            interval_steps,
            controller,
            stats,
        );
        let flush_result = Self::flush_output(&mut w, sink, stats);
        self.pool = None;
        run_result?;
        flush_result?;
        self.finish(steady.as_ref(), stats);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn run_steps<F, C>(
        &mut self,
        stages: &Stages,
        w: &mut Work,
        steady: &mut Option<Steady>,
        sink: &mut F,
        interval_steps: u64,
        controller: &mut C,
        stats: &mut RunStats,
    ) -> Result<(), SimError>
    where
        F: FnMut(f64, &[f64]) -> std::io::Result<()>,
        C: ControllerHook,
    {
        let h = self.h;
        self.start(stages, w, steady, controller, stats)?;
        self.emit(0.0, w, sink, stats)?;

        let mut matrix = Matrix {
            lu: Lu::new(self.layout.parts.len()),
            alpha: stages.alpha(),
        };
        self.refresh(&mut matrix.lu, stages.alpha(), 0.0, stats)?;
        for step in 0..self.steps {
            let call_controller = C::ENABLED && (step + 1) % interval_steps == 0;
            if self.switching == model::Switching::Interpolated {
                self.step_split(
                    step,
                    call_controller,
                    stages,
                    &mut matrix,
                    w,
                    steady,
                    controller,
                    stats,
                )?;
            } else {
                self.step(
                    step,
                    call_controller,
                    stages,
                    &mut matrix,
                    w,
                    steady,
                    controller,
                    stats,
                )?;
            }
            if (step + 1) % self.interval == 0 {
                self.emit((step + 1) as f64 * h, w, sink, stats)?;
            }
        }
        Ok(())
    }

    /// The solution at t = 0+, for the first output row: time-controlled
    /// actions at t = 0 are in effect and the diode states follow from the
    /// initial conditions. The controls are evaluated on this solution; if
    /// they turn a switch on, or change a value that an element reads,
    /// t = 0+ is solved again.
    fn start<C: ControllerHook>(
        &mut self,
        stages: &Stages,
        w: &mut Work,
        steady: &mut Option<Steady>,
        controller: &mut C,
        stats: &mut RunStats,
    ) -> Result<(), SimError> {
        let tol = self.action_tolerance(0.0);
        self.pre_step(0.0, tol, &mut w.actions);
        if !w.actions.is_empty() {
            // The circuit leaves the steady state at once.
            if let Some(s) = steady.as_mut() {
                s.active = false;
            }
        }
        for (name, action) in w.actions.drain(..) {
            push_event(stats, 0.0, &name, action);
        }
        for e in self.elements.iter_mut() {
            e.save();
        }
        let no_current = vec![0.0; self.diodes.len()];
        let mut lu = Lu::new(self.layout.parts.len());
        let peaks = self.peaks;
        for pass in 0..2 {
            if pass == 1 {
                if self.controls.is_empty() && !C::ENABLED {
                    break;
                }
                if C::ENABLED {
                    self.evaluate_controls_with_controller(0.0, true, w, stats, controller)?;
                } else {
                    self.evaluate_controls(0.0, &w.measured, stats)?;
                }
                let (switched, value_changed) = self.read_signals(0.0, stats);
                if !(switched || value_changed) {
                    break;
                }
                for e in self.elements.iter_mut() {
                    e.restore();
                }
                // The first solution is not kept: its voltages and currents
                // do not set the thresholds of the diode states.
                self.peaks = peaks;
            }
            if !self.diodes.is_empty() {
                let start = self.diode_states();
                self.settle(&mut lu, &stages.probe, 0.0, w, stats)?;
                self.diode_events(0.0, &start, &no_current, stats);
            }
            self.refresh(&mut lu, stages.initial[0].alpha, 0.0, stats)?;
            self.attempt(
                &mut lu,
                &stages.initial,
                0.0,
                stages.initial_interval,
                w,
                stats,
            )?;
        }
        if let Some(s) = steady.as_mut() {
            s.check(0.0, &w.x, &w.row, &self.voltage_unknowns);
        }
        Ok(())
    }

    /// The time by which an action may lie after the instant `t` at which
    /// the elements are asked to act, and still be taken there. With
    /// switching at grid points it only absorbs rounding. With switching
    /// instants within steps, an instant closer than the shortest part of a
    /// step (`min_part`) to a grid point is moved to it.
    fn action_tolerance(&self, t: f64) -> f64 {
        if self.places_instants() {
            self.min_part(t)
        } else {
            self.time_tolerance(t)
        }
    }

    /// The switching actions at the grid point `t0`, after its output: a
    /// diode contradicted by the solution there changes its state,
    /// time-controlled elements and elements that read signals act, and the
    /// diode states are settled.
    fn grid_point(
        &mut self,
        t0: f64,
        stages: &Stages,
        matrix: &mut Matrix,
        w: &mut Work,
        steady: &mut Option<Steady>,
        stats: &mut RunStats,
    ) -> Result<GridPoint, SimError> {
        let mut changed = false;
        let mut opened_at_zero = false;
        // `matrix.lu` already holds the regular matrix of the present states.
        let mut fresh = false;
        // A diode contradicted by the solution at t0 (the end of the
        // last step) changes its state now, after the output of t0.
        let mut start = std::mem::take(&mut w.diode_states);
        let mut start_current = std::mem::take(&mut w.diode_currents);
        self.diode_states_into(&mut start);
        self.diode_currents_into(&w.x, &mut start_current);
        let mut reversed = Vec::new();
        if !self.diodes.is_empty() {
            let tol_x = self.tolerance(&w.x, &matrix.lu);
            for k in 0..self.diodes.len() {
                let e = self.diodes[k];
                if self.inconsistent(e, &w.x, &tol_x) {
                    if start[k] {
                        reversed.push(k);
                    }
                    self.elements[e].flip();
                    self.touch(e);
                    changed = true;
                }
            }
        }
        let tol = self.action_tolerance(t0);
        changed |= self.pre_step(t0, tol, &mut w.actions);
        for (name, action) in w.actions.drain(..) {
            changed = true;
            push_event(stats, t0, &name, action);
        }
        let (switched, value_changed) = self.read_signals(t0, stats);
        changed |= switched;
        if changed || value_changed {
            // A changed value can be the gate of a thyristor, which changes
            // its zero-crossing function (see `act`).
            self.crossings.start_valid = false;
        }
        if changed {
            for e in self.elements.iter_mut() {
                e.after_action();
            }
            if let Some(s) = steady.as_mut() {
                s.active = false;
            }
        }
        if changed && !self.diodes.is_empty() {
            self.settle(&mut matrix.lu, &stages.probe, t0, w, stats)?;
            fresh = true;
            matrix.alpha = stages.probe.alpha;
            self.diode_events(t0, &start, &start_current, stats);
            let now = self.diode_states();
            opened_at_zero = reversed.iter().any(|&k| !now[k]);
        }
        w.diode_states = start;
        w.diode_currents = start_current;
        Ok(GridPoint {
            changed,
            opened_at_zero,
            fresh,
        })
    }

    /// One time step, from grid point `step` to the next: the switching
    /// actions at its start, the stages, and the controls at its end.
    #[allow(clippy::too_many_arguments)]
    fn step<C: ControllerHook>(
        &mut self,
        step: u64,
        call_controller: bool,
        stages: &Stages,
        matrix: &mut Matrix,
        w: &mut Work,
        steady: &mut Option<Steady>,
        controller: &mut C,
        stats: &mut RunStats,
    ) -> Result<(), SimError> {
        let h = self.h;
        let t0 = step as f64 * h;
        let t1 = (step + 1) as f64 * h;
        let at = if step > 0 {
            self.grid_point(t0, stages, matrix, w, steady, stats)?
        } else {
            GridPoint::default()
        };
        let GridPoint {
            changed,
            opened_at_zero,
            fresh,
        } = at;
        // A diode that stops conducting one step after its current crossed
        // zero cuts a small current. If an inductor carried it, the current
        // now decays through the blocking resistances, a mode far faster
        // than the step. The 2S-DIRK stability function tends to zero from
        // below there, so the mode would change sign at the next grid point
        // and forward-bias the diode again (chattering). Backward Euler
        // damps it without a change of sign.
        let stages_now = if opened_at_zero && self.method == Method::TwoStageDirk {
            self.refresh(&mut matrix.lu, stages.half_steps[0].alpha, t0, stats)?;
            matrix.alpha = stages.half_steps[0].alpha;
            &stages.half_steps
        } else {
            if (changed && !fresh) || matrix.alpha != stages.alpha() {
                self.refresh(&mut matrix.lu, stages.alpha(), t0, stats)?;
                matrix.alpha = stages.alpha();
            }
            if changed {
                &stages.after_event
            } else {
                &stages.regular
            }
        };
        if !changed && !opened_at_zero && self.plain && !self.layout.is_whole() {
            let t2 = (step + 2) as f64 * h;
            self.plain_step(&mut matrix.lu, stages_now, t0, t1, t2, w, stats)?;
        } else {
            self.attempt(&mut matrix.lu, stages_now, t0, t1, w, stats)?;
        }
        self.end_of_step(t1, false, call_controller, w, steady, controller, stats)
    }

    /// Updates step counters, steady-state comparison and controls at the end
    /// of a step (`t1`).
    #[allow(clippy::too_many_arguments)]
    pub(in crate::sim) fn end_of_step<C: ControllerHook>(
        &mut self,
        t1: f64,
        split: bool,
        call_controller: bool,
        w: &mut Work,
        steady: &mut Option<Steady>,
        controller: &mut C,
        stats: &mut RunStats,
    ) -> Result<(), SimError> {
        stats.steps += 1;
        if split {
            stats.split_steps += 1;
        }
        if let Some(s) = steady.as_mut() {
            s.check(t1, &w.x, &w.row, &self.voltage_unknowns);
        }
        if C::ENABLED {
            self.evaluate_controls_with_controller(t1, call_controller, w, stats, controller)?;
        } else if !self.controls.is_empty() {
            self.evaluate_controls(t1, &w.measured, stats)?;
        }
        Ok(())
    }

    /// Warning W271: diodes, controlled switches and thyristors that the model leaves at
    /// a default resistance and that blocked more than one device can.
    fn warn_of_default_resistances(&self, stats: &mut RunStats) {
        let unit = |v: f64, unit: &str| {
            if v >= 1e6 {
                format!("{:.3} M{unit}", v / 1e6)
            } else if v >= 1e3 {
                format!("{:.3} k{unit}", v / 1e3)
            } else {
                format!("{v:.3} {unit}")
            }
        };
        let found: Vec<(&str, DefaultStress)> = self
            .two_state
            .iter()
            .filter_map(|&i| {
                let e = &self.elements[i];
                e.default_stress().map(|s| (e.name(), s))
            })
            .filter(|(_, s)| s.blocked_v > SINGLE_DEVICE_BLOCKING_VOLTAGE_V)
            .collect();
        if found.is_empty() {
            return;
        }
        const LISTED: usize = 12;
        let mut list: Vec<String> = found
            .iter()
            .take(LISTED)
            .map(|(name, s)| {
                let defaults = match (s.default_on, s.default_off) {
                    (true, true) => "on_resistance_ohm and off_resistance_ohm",
                    (true, false) => "on_resistance_ohm",
                    _ => "off_resistance_ohm",
                };
                format!(
                    "{name} (default {defaults}; blocked {}, a leak loss of up to V^2/R_off = {}; \
                     carried {}, a drop of R_on I = {})",
                    unit(s.blocked_v, "V"),
                    unit(s.blocked_v * s.blocked_v / s.off_resistance_ohm, "W"),
                    unit(s.carried_a, "A"),
                    unit(s.on_resistance_ohm * s.carried_a, "V"),
                )
            })
            .collect();
        if found.len() > LISTED {
            list.push(format!("and {} more", found.len() - LISTED));
        }
        let names: Vec<&str> = found.iter().map(|(name, _)| *name).collect();
        stats.warnings.push(Diagnostic::warning(
            "W271",
            names.join(", "),
            format!(
                "{} with default resistances blocked more than {}, which is more than one \
                 semiconductor device blocks: {}",
                if found.len() == 1 {
                    "1 element".to_string()
                } else {
                    format!("{} elements", found.len())
                },
                unit(SINGLE_DEVICE_BLOCKING_VOLTAGE_V, "V"),
                list.join("; ")
            ),
            "Such an element stands for n devices in series, and the defaults (1e-3 and 1e6 ohm) \
             are those of one device: the conduction loss comes out n times too small, and the \
             leak loss V^2/R_off has no counterpart in the valve. Set on_resistance_ohm to \
             n (V_T0 + r_T I)/I (threshold voltage V_T0 and slope resistance r_T of one device \
             from its data sheet, I the current the element carries) and off_resistance_ohm to \
             the rated voltage / leakage current, or to the grading resistance of the valve. A \
             resistor with two values has no threshold voltage, no switching loss and no \
             snubber. To keep the defaults, write them in the model.",
        ));
    }

    /// The warnings and the report at the end of a run.
    fn finish(&self, steady: Option<&Steady>, stats: &mut RunStats) {
        if self.threads > 1 && !self.exact_threads && self.layout.parts.len() == 1 {
            stats.warnings.push(Diagnostic::warning(
                "W301",
                "simulation.threads",
                format!(
                    "threads is {} but the circuit is one part: it was solved by one thread",
                    self.threads
                ),
                if self.divide {
                    "Threads solve the parts of a circuit that only bergeron_line components \
                     join. This circuit has no such line, or other components (a pi_line, a \
                     coupled_rl, a transformer) hold the buses of its lines together, or it is \
                     so small that \"partitioning\": \"auto\" keeps it in one part (parts joined \
                     by a line stay together up to 48 named nodes). Remove \"threads\", or model \
                     the lines between its areas as bergeron_line (their travel times must be \
                     at least time_step_s)."
                } else {
                    "Threads solve the parts of a divided circuit, and \"partitioning\" is \
                     \"off\". Remove \"partitioning\" (the default \"auto\" divides the circuit at \
                     its bergeron_line components), or remove \"threads\"."
                },
            ));
        }
        let waiting: Vec<&str> = (0..self.elements.len())
            .map(|k| &self.elements[self.made(k)])
            .filter(|e| e.waiting_to_open())
            .map(|e| e.name())
            .collect();
        if !waiting.is_empty() {
            stats.warnings.push(Diagnostic::warning(
                "W221",
                waiting.join(", "),
                format!(
                    "still closed at the end of the run, waiting for a current zero to open: {}",
                    waiting.join(", ")
                ),
                "The current did not cross zero after the opening time (a direct current never \
                 does). Set \"open_at_current_zero\": false (for a fault, \
                 \"clear_at_current_zero\": false) to open at the given time, or run longer.",
            ));
        }
        if stats.instant_limit_steps > 0 {
            stats.warnings.push(Diagnostic::warning(
                "W270",
                "simulation.switching",
                format!(
                    "{} time steps had more than {} switching instants within the step; the \
                     later ones were moved to the next grid point",
                    stats.instant_limit_steps,
                    self.instant_limit()
                ),
                "Elements keep switching within one time step: look in the events for the \
                 elements that change again and again (a diode that conducts and blocks in \
                 turn, a gate signal that oscillates). Reduce time_step_s, or put an RC \
                 snubber across the element. The run is complete; these instants were handled \
                 as with \"switching\": \"grid\".",
            ));
        }
        self.warn_of_default_resistances(stats);
        if let (Some(s), Some(f)) = (steady, self.init_frequency) {
            self.report_steady(s, f, stats);
        }
    }
}

/// The terms of the current (`letter` "I") or voltage ("U") of `component`
/// whose column has the suffix `suffix`, for the measurement at `path`.
fn measured_terms(
    map: &HashMap<String, Vec<netlist::Quantity>>,
    letter: &str,
    component: &str,
    suffix: &str,
    path: &str,
) -> Result<Vec<Term>, Box<Diagnostic>> {
    let quantities = map.get(component).map_or(&[][..], Vec::as_slice);
    if let Some(q) = quantities.iter().find(|q| q.suffix == suffix) {
        return Ok(q.terms.clone());
    }
    // Validation reports this for the component types that declare their
    // currents (`current_suffixes`); this is for those that do not.
    let existing: Vec<String> = quantities.iter().map(|q| q.suffix.clone()).collect();
    Err(Box::new(model::Quantity::missing(
        letter, component, suffix, &existing, path,
    )))
}

thread_local! {
    static LAST_DETAIL_CONTROL_S: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

/// Control evaluation time recorded by the last run with detail timing enabled
/// on the current thread.
#[doc(hidden)]
pub fn last_detail_control_s() -> f64 {
    LAST_DETAIL_CONTROL_S.with(std::cell::Cell::get)
}

#[cfg(test)]
mod ports_tests;

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::element::{self, Node, Tolerance};
    use crate::linalg::{Complex64, Stamp};

    /// A conductance to gnd that behaves like a diode whose consistency
    /// follows a rule on the states of all mocks.
    struct Mock {
        name: String,
        a: Node,
        index: usize,
        states: Arc<Mutex<Vec<bool>>>,
        contradicted: fn(&[bool], usize) -> bool,
        /// Its zero-crossing function says in every part that its state
        /// does not hold.
        restless: bool,
        /// Number of calls of `take_event`.
        taken: Arc<AtomicU64>,
    }

    impl Element for Mock {
        fn name(&self) -> &str {
            &self.name
        }
        fn stamp(&self, _alpha: f64, m: &mut dyn Stamp) {
            let i = self.a.unwrap();
            m.add(i, i, 1.0);
        }
        fn current(&self, _k: usize, _t: f64, _stage: &Stage, x: &[f64]) -> f64 {
            element::volt(x, self.a)
        }
        fn current_phasor(&self, _k: usize, _omega: f64, x: &[Complex64]) -> Complex64 {
            element::phasor(x, self.a)
        }
        fn inconsistent(&self, _x: &[f64], _tol: &Tolerance) -> bool {
            (self.contradicted)(&self.states.lock().unwrap(), self.index)
        }
        fn flip(&mut self) {
            let mut s = self.states.lock().unwrap();
            s[self.index] = !s[self.index];
        }
        fn event_count(&self) -> usize {
            1
        }
        fn event_functions(&self, _x: &[f64], _tol: &Tolerance, g: &mut [[f64; 2]]) {
            // Contradicted from the start of every part when `restless`.
            g[0] = [if self.restless { -1.0 } else { 1.0 }, 0.0];
        }
        fn take_event(&mut self, _k: usize, _actions: &mut Vec<(String, SwitchAction)>) {
            self.taken.fetch_add(1, Ordering::Relaxed);
        }
        fn conducting(&self) -> Option<bool> {
            Some(self.states.lock().unwrap()[self.index])
        }
    }

    /// A source, a resistor and two diodes replaced by mocks with `rule`.
    fn with_mocks(rule: fn(&[bool], usize) -> bool) -> Simulator {
        mocks(rule, "grid", false).0
    }

    /// The same with the given `switching`; `restless` mocks contradict
    /// their state in every part of a step. Also returns the number of
    /// calls of `take_event`.
    fn mocks(
        rule: fn(&[bool], usize) -> bool,
        switching: &str,
        restless: bool,
    ) -> (Simulator, Arc<AtomicU64>) {
        let text = r#"{"format": "openkeito-model", "version": "0.5",
            "simulation": {"time_step_s": 1e-6, "end_time_s": 1e-5, "switching": "SWITCHING"},
            "components": [
              {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
              {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 1.0},
              {"type": "diode", "name": "D1", "nodes": ["n2", "gnd"]},
              {"type": "diode", "name": "D2", "nodes": ["n2", "gnd"]}
            ]}"#;
        let text = text.replace("SWITCHING", switching);
        let model = openkeito_model::load_str(&text).unwrap().model;
        let mut sim = Simulator::from_model(&model).unwrap();
        let states = Arc::new(Mutex::new(vec![false, false]));
        let taken = Arc::new(AtomicU64::new(0));
        for (index, name) in ["D1", "D2"].into_iter().enumerate() {
            let k = sim.diodes[index];
            // The diodes are connected from n2 to gnd.
            let a = Some(1);
            sim.elements[k] = Box::new(Mock {
                name: name.to_string(),
                a,
                index,
                states: states.clone(),
                contradicted: rule,
                restless,
                taken: taken.clone(),
            });
        }
        (sim, taken)
    }

    #[test]
    fn instants_beyond_the_limit_of_a_step_wait_for_the_grid_point() {
        // Both mocks say in every part that their state does not hold from
        // its start, and taking the event does not change that: the solver
        // places instants at the start of the step until the limit, then
        // ends the step. The run completes with a warning.
        let (sim, taken) = mocks(|_, _| false, "interpolated", true);
        let limit = sim.instant_limit();
        assert_eq!(limit, 20 + 2 * (2 + 2));
        let stats = sim.run(|_, _| Ok(())).unwrap();
        assert_eq!(stats.steps, 10);
        assert_eq!(stats.instant_limit_steps, 10);
        // Two mocks per instant.
        assert_eq!(taken.load(Ordering::Relaxed), 10 * limit * 2);
        let w: Vec<String> = stats.warnings.iter().map(|d| d.code.to_string()).collect();
        assert_eq!(w, ["W270"]);
        assert!(stats.warnings[0].hint.contains("snubber"));
        // Without them the same circuit has no instant and no warning.
        let (sim, taken) = mocks(|_, _| false, "interpolated", false);
        let stats = sim.run(|_, _| Ok(())).unwrap();
        assert_eq!(
            (stats.instant_limit_steps, taken.load(Ordering::Relaxed)),
            (0, 0)
        );
        assert!(stats.warnings.is_empty());
    }

    #[test]
    fn states_without_a_consistent_combination_end_in_e202() {
        // D1 holds when it equals D2, D2 when it differs from D1: no
        // combination satisfies both.
        let sim = with_mocks(|s, i| if i == 0 { s[0] != s[1] } else { s[0] == s[1] });
        let err = sim.run(|_, _| Ok(())).unwrap_err();
        match &err {
            SimError::SwitchingNotSettled {
                time_s, components, ..
            } => {
                assert_eq!(*time_s, 0.0);
                assert_eq!(components, &["D1", "D2"]);
            }
            other => panic!("unexpected error {other:?}"),
        }
        let d = err.to_diagnostic();
        assert_eq!(d.code, "E202");
        assert!(d.hint.contains("snubber"));
    }

    #[test]
    fn a_cycle_of_simultaneous_changes_falls_back_to_one_change_at_a_time() {
        // Both are contradicted while they are equal. Changing both at once
        // cycles (off, off) -> (on, on) -> (off, off); from (on, on) only D1
        // is changed instead, which settles at (off, on): D2 has closed.
        let sim = with_mocks(|s, _| s[0] == s[1]);
        let stats = sim.run(|_, _| Ok(())).unwrap();
        assert_eq!(stats.cycle_fallbacks, 1);
        assert_eq!(stats.events.len(), 1);
        assert_eq!(stats.events[0].component, "D2");
        assert_eq!(stats.events[0].action, "close");
    }

    #[test]
    fn buffered_output_flushes_rows_up_to_failure_and_with_zero_columns() {
        // Fails at step 290 (> OUTPUT_BATCH = 256) when S1 opens a floating node:
        // rows 0..=289 (290 rows) must be flushed before returning Err.
        let h = 1e-5;
        let text = format!(
            r#"{{"format": "openkeito-model", "version": "0.5",
                "simulation": {{"time_step_s": {h}, "end_time_s": {end}}},
                "components": [
                  {{"type": "current_source", "name": "J1", "nodes": ["gnd", "n1"],
                    "waveform": {{"type": "dc", "value": 1.0}}}},
                  {{"type": "switch", "name": "S1", "nodes": ["n1", "gnd"],
                    "initially_closed": true, "open_time_s": {open}, "open_at_current_zero": false}}
                ],
                "outputs": [{{"type": "node_voltage", "node": "n1"}}]}}"#,
            end = 400.0 * h,
            open = 290.0 * h,
        );
        let model = openkeito_model::load_str(&text).unwrap().model;
        let sim = Simulator::from_model(&model).unwrap();
        let mut received = Vec::new();
        let (stats, res) = sim.run_with_stats(|t, row| {
            received.push((t, row[0]));
            Ok(())
        });
        assert!(matches!(res, Err(SimError::Singular { .. })));
        assert_eq!(received.len(), 291);
        assert_eq!(stats.rows, 291);

        // Zero output columns must also buffer and flush without panicking.
        let empty_out = r#"{"format": "openkeito-model", "version": "0.5",
            "simulation": {"time_step_s": 1e-5, "end_time_s": 3e-3},
            "components": [
              {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
               "waveform": {"type": "dc", "value": 1.0}},
              {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 1.0}
            ],
            "outputs": []}"#;
        let model = openkeito_model::load_str(empty_out).unwrap().model;
        let sim = Simulator::from_model(&model).unwrap();
        let mut times = Vec::new();
        let stats = sim
            .run(|t, row| {
                assert!(row.is_empty());
                times.push(t);
                Ok(())
            })
            .unwrap();
        assert_eq!(times.len(), 301);
        assert_eq!(stats.rows, 301);
    }

    #[test]
    fn detail_timing_measures_control_evaluation_time() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let text = std::fs::read_to_string(dir.join("control_blocks.json")).unwrap();
        let model = openkeito_model::load_str(&text).unwrap().model;
        let stats = Simulator::from_model(&model)
            .unwrap()
            .with_detail_timing(true)
            .run(|_, _| Ok(()))
            .unwrap();
        assert!(
            stats.timing.control_s > 0.0 && stats.timing.control_s <= stats.timing.solve_s,
            "control_s={} solve_s={}",
            stats.timing.control_s,
            stats.timing.solve_s
        );
        let serialized = serde_json::to_value(&stats.timing).unwrap();
        assert!(serialized.get("control_s").is_none());

        if std::env::var_os("OPENKEITO_DETAIL_TIMING").is_none() {
            let default_stats = Simulator::from_model(&model)
                .unwrap()
                .run(|_, _| Ok(()))
                .unwrap();
            assert_eq!(default_stats.timing.control_s, 0.0);
        }
    }

    #[test]
    fn controller_every_step_matches_builtin_gain_block() {
        let builtin_json = r#"{
            "format": "openkeito-model",
            "version": "0.5",
            "simulation": {"time_step_s": 1e-4, "end_time_s": 5e-3},
            "components": [
                {"type": "controlled_voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "signal": "u"},
                {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 2.0},
                {"type": "inductor", "name": "L1", "nodes": ["n2", "n3"], "inductance_h": 1e-2},
                {"type": "capacitor", "name": "C1", "nodes": ["n3", "gnd"], "capacitance_f": 1e-4}
            ],
            "controls": [
                {"type": "constant", "name": "v_ref", "value": 10.0},
                {"type": "measurement", "name": "v_c", "quantity": {"type": "node_voltage", "node": "n3"}},
                {"type": "sum", "name": "err", "inputs": ["v_ref", "v_c"], "signs": [1.0, -1.0]},
                {"type": "gain", "name": "u", "input": "err", "gain": 3.5}
            ],
            "outputs": [
                {"type": "node_voltage", "node": "n3"},
                {"type": "signal", "signal": "u"}
            ]
        }"#;
        let ctrl_json = r#"{
            "format": "openkeito-model",
            "version": "0.5",
            "simulation": {"time_step_s": 1e-4, "end_time_s": 5e-3},
            "components": [
                {"type": "controlled_voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "signal": "u"},
                {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 2.0},
                {"type": "inductor", "name": "L1", "nodes": ["n2", "n3"], "inductance_h": 1e-2},
                {"type": "capacitor", "name": "C1", "nodes": ["n3", "gnd"], "capacitance_f": 1e-4}
            ],
            "controls": [
                {"type": "measurement", "name": "v_c", "quantity": {"type": "node_voltage", "node": "n3"}},
                {"type": "constant", "name": "u", "value": 0.0}
            ],
            "outputs": [
                {"type": "node_voltage", "node": "n3"},
                {"type": "signal", "signal": "u"}
            ]
        }"#;

        let m_ref = openkeito_model::load_str(builtin_json).unwrap().model;
        let mut ref_rows = Vec::new();
        Simulator::from_model(&m_ref)
            .unwrap()
            .run(|t, row| {
                ref_rows.push((t, row[0], row[1]));
                Ok(())
            })
            .unwrap();

        let m_ctrl = openkeito_model::load_str(ctrl_json).unwrap().model;
        let sim = Simulator::from_model(&m_ctrl).unwrap();
        let v_c_idx = sim.signal_names().iter().position(|n| n == "v_c").unwrap();
        let u_idx = sim.signal_names().iter().position(|n| n == "u").unwrap();
        let mut ctrl_rows = Vec::new();
        let mut calls = 0_u64;
        sim.run_with_controller(
            |t, row| {
                ctrl_rows.push((t, row[0], row[1]));
                Ok(())
            },
            1,
            |_t, _cols, signals, updates| {
                calls += 1;
                let err = 10.0 - signals[v_c_idx];
                updates.push((u_idx, 3.5 * err));
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(calls, 51);
        assert_eq!(ref_rows.len(), ctrl_rows.len());
        for (a, b) in ref_rows.iter().zip(&ctrl_rows) {
            assert!((a.0 - b.0).abs() <= 1e-15);
            assert!((a.1 - b.1).abs() <= 1e-12, "V(n3): {} vs {}", a.1, b.1);
            assert!((a.2 - b.2).abs() <= 1e-12, "S(u): {} vs {}", a.2, b.2);
        }
    }

    #[test]
    fn controller_periodic_zoh_calls_and_holds_between_updates() {
        let json = r#"{
            "format": "openkeito-model",
            "version": "0.5",
            "simulation": {"time_step_s": 1e-4, "end_time_s": 3.5e-3},
            "components": [
                {"type": "controlled_voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "signal": "u"},
                {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0}
            ],
            "controls": [
                {"type": "constant", "name": "u", "value": 0.0}
            ],
            "outputs": [
                {"type": "node_voltage", "node": "n1"},
                {"type": "signal", "signal": "u"}
            ]
        }"#;
        let model = openkeito_model::load_str(json).unwrap().model;
        let sim = Simulator::from_model(&model).unwrap();
        assert_eq!(sim.steps, 35);
        let u_idx = sim.signal_names().iter().position(|n| n == "u").unwrap();
        let mut calls = Vec::new();
        let mut rows = Vec::new();
        sim.run_with_controller(
            |t, row| {
                rows.push((t, row[0], row[1]));
                Ok(())
            },
            10,
            |t, _cols, _signals, updates| {
                let k = calls.len() + 1;
                calls.push(t);
                updates.push((u_idx, k as f64));
                Ok(())
            },
        )
        .unwrap();

        // steps = 35, interval_steps = 10 -> t = 0, 10h, 20h, 30h -> 4 calls.
        assert_eq!(calls.len() as u64, 1 + 35 / 10);
        assert_eq!(rows.len(), 36);
        for (step, &(_, v_n1, s_u)) in rows.iter().enumerate() {
            let expected_u = (step / 10 + 1) as f64;
            assert_eq!(s_u, expected_u, "step {step}");
            // At step 0, pass 1 sets u = 1.0 and re-solves t = 0+.
            // At step 10, u becomes 2.0 at end_of_step, reaching circuit at step 11.
            let expected_v = if step == 0 {
                1.0
            } else {
                ((step - 1) / 10 + 1) as f64
            };
            assert!(
                (v_n1 - expected_v).abs() <= 1e-12,
                "step {step}: {v_n1} vs {expected_v}"
            );
        }
    }

    #[test]
    fn controller_error_stops_with_e252_and_flushes_partial_rows() {
        // Steps = 300 (> OUTPUT_BATCH = 256), fail at step 280.
        let json = r#"{
            "format": "openkeito-model",
            "version": "0.5",
            "simulation": {"time_step_s": 1e-4, "end_time_s": 3e-2},
            "components": [
                {"type": "controlled_voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "signal": "u"},
                {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0}
            ],
            "controls": [
                {"type": "constant", "name": "u", "value": 0.0}
            ],
            "outputs": [
                {"type": "node_voltage", "node": "n1"},
                {"type": "signal", "signal": "u"}
            ]
        }"#;
        let model = openkeito_model::load_str(json).unwrap().model;
        let sim = Simulator::from_model(&model).unwrap();
        let u_idx = sim.signal_names().iter().position(|n| n == "u").unwrap();
        let mut rows = Vec::new();
        let mut calls = 0_u64;
        let (stats, res) = sim.run_with_stats_and_controller(
            |t, row| {
                rows.push((t, row[0], row[1]));
                Ok(())
            },
            1,
            |_t, _cols, _signals, updates| {
                if calls == 280 {
                    return Err("boom".to_string());
                }
                calls += 1;
                updates.push((u_idx, 1.0));
                Ok(())
            },
        );

        match &res {
            Err(SimError::Controller { time_s, message }) => {
                assert!((*time_s - 280e-4).abs() <= 1e-15);
                assert_eq!(message, "boom");
            }
            other => panic!("unexpected result: {other:?}"),
        }
        let diag = res.unwrap_err().to_diagnostic();
        assert_eq!(diag.code, "E252");
        assert_eq!(diag.path, "controller");
        // Calls 0..279 emitted rows 0..=279 (280 rows); call 280 at t = 280*h failed before emit.
        assert_eq!(rows.len(), 280);
        assert_eq!(stats.rows, 280);
    }
}
