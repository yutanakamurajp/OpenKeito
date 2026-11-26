//! Validation of a whole model. What belongs to one component type or one
//! block type is checked by its `ComponentSpec` or `ControlSpec`
//! (`components`, `controls`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::check::{
    check_bus, check_component, check_node, check_signal, positive, topology, valid_name, Edge,
    Sets,
};
use crate::components::{
    expanded, owner_of_block, Branch, BusRef, Component, ComponentInitialization, SteadyState,
};
use crate::controls::{evaluation_plan, signal_names, Scope};
use crate::diagnostic::{Diagnostic, Severity};
use crate::model::*;

/// How far end_time_s / time_step_s may be from a whole number and still
/// count as one (the two are decimal numbers, their quotient is rounded).
const GRID_TOLERANCE: f64 = 1e-6;

/// More steps than this cannot be meant: at a microsecond per step they take
/// more than eleven days.
const MAX_STEPS: f64 = 1e12;

/// Above this many output rows the result file takes gigabytes.
const MANY_ROWS: u64 = 100_000_000;

/// Number of time steps of a run: the largest n with n * time_step_s <=
/// end_time_s, where a quotient within rounding of a whole number is that
/// number (60 s / 2.5 us is 23999999.999999996 in floating point).
pub fn step_count(sim: &Simulation) -> u64 {
    let ratio = sim.end_time_s / sim.time_step_s;
    let n = if (ratio - ratio.round()).abs() <= GRID_TOLERANCE {
        ratio.round()
    } else {
        ratio.floor()
    };
    if n.is_finite() && n > 0.0 {
        n as u64
    } else {
        0
    }
}

/// Checks a parsed model and returns every error and warning found.
pub fn validate(model: &Model) -> Vec<Diagnostic> {
    let mut d = Vec::new();

    if model.format != FORMAT {
        d.push(Diagnostic::error(
            "E100",
            "format",
            format!("format is \"{}\"", model.format),
            format!("Set \"format\": \"{FORMAT}\"."),
        ));
    }
    if !SUPPORTED_VERSIONS.contains(&model.version.as_str()) {
        d.push(Diagnostic::error(
            "E101",
            "version",
            format!("version \"{}\" is not supported", model.version),
            format!(
                "Set \"version\": \"{VERSION}\". This program reads the versions {}.",
                SUPPORTED_VERSIONS.join(", ")
            ),
        ));
    } else {
        check_version_additions(model, &mut d);
    }

    let sim = &model.simulation;
    let step_ok = positive(sim.time_step_s);
    if !step_ok {
        d.push(Diagnostic::error(
            "E110",
            "simulation.time_step_s",
            format!("time_step_s is {}", sim.time_step_s),
            "Use a positive time step in seconds, for example 1e-5.",
        ));
    }
    if !sim.end_time_s.is_finite() || (step_ok && sim.end_time_s < sim.time_step_s) {
        d.push(Diagnostic::error(
            "E111",
            "simulation.end_time_s",
            format!(
                "end_time_s is {} and time_step_s is {}",
                sim.end_time_s, sim.time_step_s
            ),
            "end_time_s must be at least one time step.",
        ));
    } else if step_ok && sim.end_time_s / sim.time_step_s > MAX_STEPS {
        d.push(Diagnostic::error(
            "E113",
            "simulation.time_step_s",
            format!(
                "the run has {:e} time steps (end_time_s {:e} / time_step_s {:e})",
                sim.end_time_s / sim.time_step_s,
                sim.end_time_s,
                sim.time_step_s
            ),
            "A run has at most 1e12 time steps. Raise time_step_s or lower end_time_s; check \
             their units (seconds).",
        ));
    } else if step_ok {
        let rows = step_count(sim) / sim.output_interval() + 1;
        if rows > MANY_ROWS {
            d.push(Diagnostic::warning(
                "W114",
                "simulation.output_interval_steps",
                format!(
                    "the run records {rows} rows: result.csv takes about {} GB per ten columns",
                    rows / 5_000_000
                ),
                "Record every n-th step with simulation.output_interval_steps, list only the \
                 outputs you need, or shorten the run.",
            ));
        }
        let ratio = sim.end_time_s / sim.time_step_s;
        if (ratio - ratio.round()).abs() > GRID_TOLERANCE {
            let last = step_count(sim) as f64 * sim.time_step_s;
            d.push(Diagnostic::warning(
                "W112",
                "simulation.end_time_s",
                format!(
                    "end_time_s is not a multiple of time_step_s; the last output is at t = {last:e} s"
                ),
                "Choose end_time_s as a multiple of time_step_s.",
            ));
        }
    }
    if sim.threads == Some(0) {
        d.push(Diagnostic::error(
            "E301",
            "simulation.threads",
            "threads is 0",
            "Use a whole number >= 1: 1 (the default) solves the parts of the circuit one after \
             the other, n lets up to n threads solve them side by side.",
        ));
    }
    if sim.output_interval_steps == Some(0) {
        d.push(Diagnostic::error(
            "E130",
            "simulation.output_interval_steps",
            "output_interval_steps is 0",
            "Use a whole number >= 1: 1 records every step, 10 every tenth step.",
        ));
    }

    if model.components.is_empty() {
        d.push(Diagnostic::error(
            "E160",
            "components",
            "the model has no components",
            "Add at least a source and one component connected to \"gnd\".",
        ));
    }

    // Names of components and control blocks share one namespace.
    let mut names: HashMap<&str, String> = HashMap::new();
    let mut component_names: BTreeSet<&str> = BTreeSet::new();
    let mut node_names: BTreeSet<String> = BTreeSet::new();
    for (i, c) in model.components.iter().enumerate() {
        let p = format!("components[{i}]");
        register_name(c.name(), &p, &mut names, &mut d);
        component_names.insert(c.name());
        c.spec().check_terminals(&p, &mut d);
        node_names.extend(c.node_names());
        c.spec().check_parameters(&p, &mut d);
    }
    check_derived_names(model, &mut d);
    if step_ok {
        check_conditioning(model, &mut d);
        for (i, c) in model.components.iter().enumerate() {
            c.spec()
                .check_simulation(sim, &format!("components[{i}]"), &mut d);
        }
    }
    let nodes: BTreeSet<&str> = node_names.iter().map(String::as_str).collect();
    for (i, c) in model.controls.iter().enumerate() {
        register_name(c.name(), &format!("controls[{i}]"), &mut names, &mut d);
        check_converter_prefix(model, i, &mut d);
    }
    check_signal_names(model, &mut d);
    // The signals of the model's blocks and of the blocks of its converters.
    let with_converters = expanded(model);
    let all_signals = signal_names(&with_converters.controls);
    let signals: BTreeSet<&str> = all_signals.iter().map(String::as_str).collect();

    check_connectivity(model, &nodes, &mut d);

    for (i, c) in model.components.iter().enumerate() {
        for (field, s) in c.spec().signals() {
            check_signal(s, format!("components[{i}].{field}"), &signals, &mut d);
        }
    }
    let scope = Scope {
        nodes: &nodes,
        component_names: &component_names,
        components: &model.components,
        time_step_s: sim.time_step_s,
    };
    for (i, c) in model.controls.iter().enumerate() {
        let p = format!("controls[{i}]");
        for (field, s) in c.inputs() {
            check_signal(s, format!("{p}.{field}"), &signals, &mut d);
        }
        c.spec().check(&p, &scope, &mut d);
    }
    check_algebraic_loops(&with_converters, &mut d);

    if let Some(outputs) = &model.outputs {
        let mut seen_columns: HashMap<String, usize> = HashMap::new();
        for (i, o) in outputs.iter().enumerate() {
            let p = format!("outputs[{i}]");
            let columns = output_columns(o, model);
            if let Some((col, first)) = columns
                .iter()
                .find_map(|c| seen_columns.get(c).map(|&first| (c.as_str(), first)))
            {
                d.push(Diagnostic::error(
                    "E155",
                    p.clone(),
                    format!("column \"{col}\" is already produced by outputs[{first}]"),
                    "List every output once: result.csv cannot hold two columns of one name.",
                ));
            }
            for col in columns {
                seen_columns.entry(col).or_insert(i);
            }
            match o {
                Output::NodeVoltage { node } => {
                    check_node(node, format!("{p}.node"), &nodes, &mut d)
                }
                Output::BranchCurrent { component } | Output::BranchVoltage { component } => {
                    check_component(
                        component,
                        format!("{p}.component"),
                        &component_names,
                        &mut d,
                    )
                }
                Output::Signal { signal } => {
                    check_signal(signal, format!("{p}.signal"), &signals, &mut d)
                }
                Output::BusVoltage { bus } => check_bus(bus, &format!("{p}.bus"), &nodes, &mut d),
            }
            if let Output::BranchVoltage { component } = o {
                if let Some(c) = model.components.iter().find(|c| c.name() == component) {
                    if !c.spec().has_branch_voltage() {
                        d.push(Diagnostic::error(
                            "E153",
                            format!("{p}.component"),
                            format!(
                                "a branch voltage is not defined for \"{component}\" ({})",
                                c.type_name()
                            ),
                            "Record the voltages of its buses with {\"type\": \"bus_voltage\", \"bus\": ...} or of its nodes with node_voltage.",
                        ));
                    }
                }
            }
        }
    }
    check_initialization(model, &mut d);
    if let Some(scan) = &model.frequency_scan {
        check_scan(scan, &nodes, &mut d);
        warn_of_states_in_a_scan(model, &mut d);
    }
    // What the components are to each other is only known of a model
    // without errors.
    if d.iter().all(|e| e.severity != Severity::Error) {
        check_ideal_branches(model, &mut d);
    }

    d
}

/// Column names that `o` produces in `result.csv`.
fn output_columns(o: &Output, model: &Model) -> Vec<String> {
    match o {
        Output::NodeVoltage { node } => vec![format!("V({node})")],
        Output::BusVoltage { bus } => BusRef::Bus(bus.clone())
            .nodes()
            .into_iter()
            .map(|n| format!("V({n})"))
            .collect(),
        Output::BranchCurrent { component } => model
            .components
            .iter()
            .find(|c| c.name() == component)
            .and_then(|c| c.current_suffixes())
            .map(|suffixes| {
                suffixes
                    .into_iter()
                    .map(|s| format!("I({component}{s})"))
                    .collect()
            })
            .unwrap_or_else(|| vec![format!("I({component})")]),
        Output::BranchVoltage { component } => model
            .components
            .iter()
            .find(|c| c.name() == component)
            .map(|c| {
                let suffixes = c.voltage_suffixes();
                if suffixes.is_empty() {
                    vec![format!("U({component})")]
                } else {
                    suffixes
                        .into_iter()
                        .map(|s| format!("U({component}{s})"))
                        .collect()
                }
            })
            .unwrap_or_else(|| vec![format!("U({component})")]),
        Output::Signal { signal } => vec![format!("S({signal})")],
    }
}

/// The format version that added the controls; a block type of that version
/// is covered by the entry "controls".
const CONTROLS_SINCE: &str = "0.2";

/// What each format version adds, as (version, path) of every use in the
/// model.
fn additions(model: &Model) -> Vec<(&'static str, String)> {
    let mut v = Vec::new();
    if model.simulation.output_interval_steps.is_some() {
        v.push(("0.2", "simulation.output_interval_steps".to_string()));
    }
    for (i, c) in model.components.iter().enumerate() {
        let version = c.spec().since();
        if version != SUPPORTED_VERSIONS[0] {
            v.push((version, format!("components[{i}] ({})", c.type_name())));
        }
        if c.initialization() == ComponentInitialization::Exclude {
            v.push(("0.7", format!("components[{i}].initialization")));
        }
    }
    if !model.controls.is_empty() {
        v.push((CONTROLS_SINCE, "controls".to_string()));
    }
    for (i, c) in model.controls.iter().enumerate() {
        let version = c.spec().since();
        if version != CONTROLS_SINCE {
            v.push((version, format!("controls[{i}] ({})", c.type_name())));
        }
    }
    if let Some(outputs) = &model.outputs {
        for (i, o) in outputs.iter().enumerate() {
            match o {
                Output::Signal { .. } => v.push(("0.2", format!("outputs[{i}] (signal)"))),
                Output::BusVoltage { .. } => v.push(("0.3", format!("outputs[{i}] (bus_voltage)"))),
                _ => {}
            }
        }
    }
    if !model.simulation.initialization.is_none() {
        v.push(("0.3", "simulation.initialization".to_string()));
    }
    if model.frequency_scan.is_some() {
        v.push(("0.3", "frequency_scan".to_string()));
    }
    if !model.simulation.linear_solver.is_auto() {
        v.push(("0.3", "simulation.linear_solver".to_string()));
    }
    if model.simulation.switching.is_some() {
        v.push(("0.5", "simulation.switching".to_string()));
    }
    if model.simulation.partitioning.is_some() {
        v.push(("0.5", "simulation.partitioning".to_string()));
    }
    if model.simulation.threads.is_some() {
        v.push(("0.5", "simulation.threads".to_string()));
    }
    v
}

/// What every version after 0.1 adds, for the hint of E102: the type names of
/// components and blocks, and the other additions in words.
///
/// A component type or block type added in the current version gets one line
/// in the last list (and that version as `since` in its `ComponentSpec` or
/// `ControlSpec`); a test keeps the two in step. A model that declares an
/// older version and uses the addition then gets E102.
const VERSION_ADDITIONS: [(&str, &[&str]); 8] = [
    (
        "0.2",
        &[
            "diode",
            "controlled_switch",
            "controls",
            "signal outputs",
            "simulation.output_interval_steps",
        ],
    ),
    (
        "0.3",
        &[
            "three_phase_source",
            "coupled_rl",
            "pi_line",
            "transformer",
            "three_phase_transformer",
            "fault",
            "bus_voltage outputs",
            "simulation.initialization",
            "simulation.linear_solver",
            "frequency_scan",
        ],
    ),
    (
        "0.4",
        &[
            "three_phase_load",
            "three_phase_current_source",
            "three_phase_ideal_transformer",
        ],
    ),
    (
        "0.5",
        &[
            // One line per addition of 0.5.
            "nonlinear_resistor",
            "nonlinear_inductor",
            "the saturation of transformer and three_phase_transformer",
            "bergeron_line",
            "simulation.switching",
            "controlled_voltage_source",
            "controlled_current_source",
            "three_phase_controlled_source",
            "three_phase_controlled_current_source",
            "transfer_function",
            "integrator",
            "lag",
            "lead_lag",
            "delay",
            "limiter",
            "rate_limiter",
            "dead_band",
            "product",
            "quotient",
            "function",
            "selector",
            "initial_value",
            "step",
            "ramp",
            "sine_wave",
            "time_series",
            "abc_to_dq0",
            "dq0_to_abc",
            "pll",
            "three_phase_measurement",
            "measurements with terminal and phase or of a component_signal",
            "a signal as initial_integral of pi",
            "synchronous_machine",
            "grid_following_converter",
            "dq_limiter",
            "pi_dq",
            "simulation.partitioning and simulation.threads",
        ],
    ),
    (
        "0.6",
        &[
            "zero_sequence_inverted on three_phase_ideal_transformer",
            "tap_changes on transformers",
            "impedance_changes on coupled_rl",
        ],
    ),
    (
        "0.7",
        &[
            "piecewise_linear",
            "signal limits on lag, pi, integrator and limiter",
            "pi.anti_windup",
            "initialization on components",
        ],
    ),
    ("0.8", &["expression"]),
    ("0.9", &["thyristor"]),
];

/// "a", "a and b" or "a, b and c".
fn in_words(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

fn version_rank(v: &str) -> usize {
    SUPPORTED_VERSIONS.iter().position(|s| *s == v).unwrap_or(0)
}

/// A model that declares an older version must not use what later versions
/// add.
fn check_version_additions(model: &Model, d: &mut Vec<Diagnostic>) {
    let declared = version_rank(&model.version);
    let newer: Vec<(&str, String)> = additions(model)
        .into_iter()
        .filter(|(v, _)| version_rank(v) > declared)
        .collect();
    let Some(first) = newer.first() else {
        return;
    };
    let needed = newer
        .iter()
        .map(|(v, _)| *v)
        .max_by_key(|v| version_rank(v))
        .unwrap_or(VERSION);
    let added: Vec<String> = VERSION_ADDITIONS
        .iter()
        .filter(|(v, what)| {
            !what.is_empty()
                && version_rank(v) > declared
                && version_rank(v) <= version_rank(needed)
        })
        .map(|(v, what)| format!("Version {v} adds {}.", in_words(what)))
        .collect();
    d.push(Diagnostic::error(
        "E102",
        first.1.clone(),
        format!(
            "version \"{}\" does not have what the model uses: {}",
            model.version,
            newer
                .iter()
                .map(|(_, p)| p.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        format!("Set \"version\": \"{needed}\". {}", added.join(" ")),
    ));
}

fn register_name<'a>(
    name: &'a str,
    path: &str,
    names: &mut HashMap<&'a str, String>,
    d: &mut Vec<Diagnostic>,
) {
    if !valid_name(name) {
        d.push(Diagnostic::error(
            "E120",
            format!("{path}.name"),
            format!("name \"{name}\" is not valid"),
            "Use letters, digits, '_', '-' and '.' only, with at least one character.",
        ));
    } else if let Some(first) = names.get(name) {
        d.push(Diagnostic::error(
            "E121",
            format!("{path}.name"),
            format!("name \"{name}\" is already used by {first}"),
            "Give every component and control block a different name.",
        ));
    } else {
        names.insert(name, path.to_string());
    }
}

/// The quantities of a component with several phases or windings are named
/// after it: `<name>.a`, `<name>.0.a`, `<name>.mag` and so on. Another
/// component with such a name would give two columns of the same name.
fn check_derived_names(model: &Model, d: &mut Vec<Diagnostic>) {
    for (i, c) in model.components.iter().enumerate() {
        let owner = model.components.iter().find(|o| {
            o.two_nodes().is_none()
                && c.name()
                    .strip_prefix(o.name())
                    .is_some_and(|rest| rest.starts_with('.'))
        });
        if let Some(o) = owner {
            d.push(Diagnostic::error(
                "E125",
                format!("components[{i}].name"),
                format!(
                    "name \"{}\" collides with the names of the phases and windings of \"{}\" ({})",
                    c.name(),
                    o.name(),
                    o.type_name()
                ),
                format!(
                    "Names that start with \"{}.\" belong to the quantities of that component: \
                     use another name, for example with '_' in place of the '.'.",
                    o.name()
                ),
            ));
        }
    }
}

/// Smallest pivot of the Gaussian elimination of a symmetric 3x3 matrix,
/// relative to its largest entry: about the reciprocal of its condition
/// number. 0 for a matrix that is singular or not finite.
fn relative_pivot(m: &[[f64; 3]; 3]) -> f64 {
    let mut a = *m;
    let scale = a.iter().flatten().fold(0.0_f64, |s, v| s.max(v.abs()));
    if !(scale.is_finite() && scale > 0.0) {
        return 0.0;
    }
    let mut smallest = f64::INFINITY;
    for col in 0..3 {
        let Some(p) = (col..3).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs())) else {
            return 0.0;
        };
        a.swap(col, p);
        let pivot = a[col][col];
        if !pivot.is_finite() || pivot == 0.0 {
            return 0.0;
        }
        smallest = smallest.min(pivot.abs() / scale);
        for r in col + 1..3 {
            let f = a[r][col] / pivot;
            for k in col..3 {
                a[r][k] -= f * a[col][k];
            }
        }
    }
    smallest
}

/// A series impedance whose phase matrix cannot be inverted in floating
/// point: the solver needs (R + alpha L)^-1 for every stage coefficient
/// alpha of the run (`Simulation::stage_coefficients`), from 2 / time_step_s
/// to 1e6 / time_step_s and beyond. A zero-sequence value far above the
/// positive-sequence one (meant as "no zero-sequence path") makes the three
/// phases indistinguishable.
///
/// Every coefficient is checked, not the smallest alone: where the
/// resistances and the inductances differ in their ratio between the
/// sequences, R decides at the smallest alpha and L at the largest, and a
/// matrix that can be inverted at one cannot at the other. The run would
/// then stop with E204 in a model that passed the validation.
fn check_conditioning(model: &Model, d: &mut Vec<Diagnostic>) {
    /// The solver gives up at 1e-14; stay well away from it.
    const SMALLEST_PIVOT: f64 = 1e-12;
    let alphas = model.simulation.stage_coefficients();
    let (least, most) = alphas
        .iter()
        .fold((f64::INFINITY, 0.0_f64), |(lo, hi), &a| {
            (lo.min(a), hi.max(a))
        });
    for (i, c) in model.components.iter().enumerate() {
        for (r, l, field) in c.spec().series_impedances() {
            let path = format!("components[{i}].{field}");
            // Entries that are themselves wrong are reported on their own.
            if d.iter().any(|e| e.path.starts_with(&path)) {
                continue;
            }
            // The stage coefficient with the smallest pivot.
            let (pivot, alpha) = alphas
                .iter()
                .map(|&alpha| {
                    let mut m = [[0.0; 3]; 3];
                    for j in 0..3 {
                        for k in 0..3 {
                            m[j][k] = r[j][k] + alpha * l[j][k];
                        }
                    }
                    (relative_pivot(&m), alpha)
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .expect("a run has stage coefficients");
            if pivot < SMALLEST_PIVOT {
                d.push(Diagnostic::error(
                    "E139",
                    path,
                    format!(
                        "the resistance and inductance of \"{}\" differ by too many orders of \
                         magnitude between the sequences (or the phases) to be solved: the phase \
                         matrix R + alpha L is singular to rounding for alpha = {alpha:.3e} / s \
                         (smallest pivot: {pivot:.0e} of its largest entry). The run solves with \
                         alpha from {least:.3e} to {most:.3e} / s (2 / time_step_s to \
                         1e6 / time_step_s and, with \"switching\": \"interpolated\", beyond)",
                        c.name(),
                    ),
                    "Keep the zero-sequence values within about 1e9 times the positive-sequence \
                     values, the resistances and the inductances each: where the two ratios differ, \
                     the resistances decide at the smallest alpha and the inductances at the \
                     largest. For a branch without a zero-sequence path, a ratio of 1e6 is ample.",
                ));
            }
        }
    }
}

/// A block with several outputs gives the signals `<name>.<output>`. Another
/// block with such a name would give a signal of the same name.
fn check_signal_names(model: &Model, d: &mut Vec<Diagnostic>) {
    for (i, c) in model.controls.iter().enumerate() {
        let mine = c.signal_names();
        let owner = model.controls.iter().enumerate().find(|(k, o)| {
            *k != i && !o.outputs().is_empty() && o.signal_names().iter().any(|s| mine.contains(s))
        });
        if let Some((_, o)) = owner {
            d.push(Diagnostic::error(
                "E240",
                format!("controls[{i}].name"),
                format!(
                    "name \"{}\" gives a signal that \"{}\" ({}) already has: {}",
                    c.name(),
                    o.name(),
                    o.type_name(),
                    o.signal_names().join(", ")
                ),
                format!(
                    "The outputs of \"{}\" are named after it. Use another name, for example \
                     with '_' in place of the '.'.",
                    o.name()
                ),
            ));
        }
    }
}

/// The control blocks of a converter are named `<converter>.<...>`. A block
/// of the model with such a name could give a signal of the same name.
fn check_converter_prefix(model: &Model, i: usize, d: &mut Vec<Diagnostic>) {
    let name = model.controls[i].name();
    if let Some(owner) = owner_of_block(model, name) {
        d.push(Diagnostic::error(
            "E290",
            format!("controls[{i}].name"),
            format!(
                "name \"{name}\" collides with the names of the control signals of the converter \"{owner}\""
            ),
            format!(
                "Names that start with \"{owner}.\" belong to the control blocks of that \
                 converter: use another name, for example with '_' in place of the '.'."
            ),
        ));
    }
}

/// Control blocks are evaluated in the order of their inputs. A loop of
/// blocks has such an order only when it passes through a state.
fn check_algebraic_loops(model: &Model, d: &mut Vec<Diagnostic>) {
    if let Err(cycle) = evaluation_plan(&model.controls, model.simulation.time_step_s) {
        let names: Vec<&str> = cycle.iter().map(|&i| model.controls[i].name()).collect();
        d.push(Diagnostic::error(
            "E171",
            format!("controls[{}]", cycle[0]),
            format!(
                "the control blocks {} form a loop without a state",
                names.join(" -> ")
            ),
            "Every block of this loop gives an output that depends on its input at the same \
             instant, so the loop has no order of evaluation. A loop must pass through a block \
             with a state (integrator, lag, a transfer_function whose numerator has a lower \
             order than its denominator, a delay of at least one time step) or through the \
             circuit (a measurement of a quantity that the controls act on).",
        ));
    }
}

/// Every node must reach gnd through components; otherwise its voltage has no
/// reference and the circuit equations are singular.
fn check_connectivity(model: &Model, nodes: &BTreeSet<&str>, d: &mut Vec<Diagnostic>) {
    if model.components.is_empty() {
        return;
    }
    // A pi line is connected to gnd through its capacitances without naming it.
    let implicit_ground = model
        .components
        .iter()
        .any(|c| c.spec().grounds_implicitly());
    if !nodes.contains(GROUND) && !implicit_ground {
        d.push(Diagnostic::error(
            "E140",
            "components",
            "no component is connected to \"gnd\"",
            "Name the reference node \"gnd\" in at least one component's nodes.",
        ));
        return;
    }
    let mut with_ground = nodes.clone();
    with_ground.insert(GROUND);
    let nodes = &with_ground;
    let index: BTreeMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let mut sets = Sets::new(index.len());
    let mut has_transformer = false;
    for c in &model.components {
        has_transformer |= c.spec().has_separate_windings();
        for (a, b) in c.spec().connections() {
            let (Some(&ia), Some(&ib)) = (index.get(a.as_str()), index.get(b.as_str())) else {
                continue;
            };
            sets.join(ia, ib);
        }
    }
    let ground = sets.find(index[GROUND]);
    let floating: Vec<&str> = nodes
        .iter()
        .copied()
        .filter(|n| sets.find(index[n]) != ground)
        .collect();
    if !floating.is_empty() {
        let about: Vec<String> = floating.iter().map(|n| n.to_string()).collect();
        d.push(Diagnostic::error(
            "E141",
            "components",
            format!(
                "nodes not connected to \"gnd\" through any component: {}",
                floating.join(", ")
            ),
            if has_transformer {
                "Connect this part of the circuit to \"gnd\", for example through a large resistor. The windings of a transformer are not connected to each other, so a side without any grounded component floats: ground a star point (YN, yn) or connect one phase of a delta side to gnd through a large resistor (such as 1e6 ohm)."
            } else {
                "Connect this part of the circuit to \"gnd\", for example through a large resistor."
            },
        ).about_nodes(about));
    }
}

/// The nodes of the model and of the inside of its components, numbered
/// from gnd (0) on.
struct Numbered {
    number: HashMap<String, usize>,
    names: Vec<String>,
}

impl Numbered {
    fn of(&mut self, name: String) -> usize {
        if let Some(&n) = self.number.get(&name) {
            return n;
        }
        self.names.push(name.clone());
        self.number.insert(name, self.names.len() - 1);
        self.names.len() - 1
    }

    /// Adds the branches of component `owner` at time `t` to `edges`.
    fn branches(&mut self, c: &Component, owner: usize, t: f64, edges: &mut Vec<(usize, Edge)>) {
        for branch in c.spec().branches(t) {
            match branch {
                Branch::Conductance(a, b) => {
                    edges.push((owner, Edge::Conductance(self.of(a), self.of(b))));
                }
                Branch::Voltage(a, b) => {
                    edges.push((owner, Edge::Voltage(self.of(a), self.of(b))));
                }
                Branch::Constraint(terms) => {
                    let terms: Vec<(usize, f64)> =
                        terms.into_iter().map(|(n, x)| (self.of(n), x)).collect();
                    if terms.iter().all(|t| t.1.is_finite()) {
                        edges.push((owner, Edge::Constraint(terms)));
                    } else {
                        // Ratings that are wrong are reported on their own:
                        // the component then holds its nodes.
                        for pair in terms.windows(2) {
                            edges.push((owner, Edge::Conductance(pair[0].0, pair[1].0)));
                        }
                    }
                }
            }
        }
    }
}

/// An action this far (in time steps) after a grid point is taken there.
const ACTION_TOLERANCE: f64 = 1e-9;

/// With switching instants within steps, the solver takes an action up to
/// this far (in time steps) after a grid point at the grid point; how far
/// depends on the conditioning of the circuit.
const MOVED_ACTION: f64 = 0.05;

/// As many warnings of one kind (W142, W143) are given.
const MAX_SWITCHING_WARNINGS: usize = 8;

/// The names of the components `members`, for the subjects of a diagnostic.
fn names_of(model: &Model, members: &[usize]) -> Vec<String> {
    members
        .iter()
        .map(|&i| model.components[i].name().to_string())
        .collect()
}

/// "\"a\" (type)" of the components `members`, at most eight of them.
fn named(model: &Model, members: &[usize]) -> String {
    let mut names: Vec<String> = members
        .iter()
        .take(8)
        .map(|&i| {
            let c = &model.components[i];
            format!("\"{}\" ({})", c.name(), c.type_name())
        })
        .collect();
    if members.len() > 8 {
        names.push(format!("{} more", members.len() - 8));
    }
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    in_words(&names)
}

const LOOP_HINT: &str = "A voltage source, a three-phase source without impedance, a closed \
    switch, a fault without resistance and an ideal transformer are ideal branches: they have \
    no impedance that would share a current between them. Put a small resistor in series with \
    one of them (such as 1e-3 ohm), give the source an \"impedance\", or remove one of two \
    parallel sources or switches.";

const FLOATING_HINT: &str = "A current source, an open switch and a fault that has not started \
    (or is cleared) hold no voltage. Connect each of these nodes to the rest of the circuit \
    through a component that does: a resistor to gnd or across the switch (such as 1e6 ohm), \
    or a capacitor.";

/// Loops of ideal branches (voltage sources, closed switches, ideal
/// transformers), in which the current is not determined, and nodes that
/// only current sources, open switches and faults that are not active
/// connect to the rest, whose voltage is not determined. Both make the
/// equations of a time step singular (E200). In the state at t = 0 they are
/// errors (E142, E143); in a state that the switches and faults reach later
/// in the run, by their times, they are warnings (W142, W143).
fn check_ideal_branches(model: &Model, d: &mut Vec<Diagnostic>) {
    let sim = &model.simulation;
    let h = sim.time_step_s;
    let steps = if positive(h) && sim.end_time_s.is_finite() {
        step_count(sim)
    } else {
        0
    };
    let mut nodes = Numbered {
        number: HashMap::from([(GROUND.to_string(), 0)]),
        names: vec![GROUND.to_string()],
    };
    // Branches that do not change, and the components whose branches do,
    // with the grid points at which they act: the first at or after each of
    // their times. The actions at the last grid point come after the run.
    let mut fixed: Vec<(usize, Edge)> = Vec::new();
    let mut switching: Vec<(usize, Vec<u64>)> = Vec::new();
    for (i, c) in model.components.iter().enumerate() {
        for n in c.node_names() {
            nodes.of(n);
        }
        let times = c.spec().switching_times();
        if times.is_empty() {
            nodes.branches(c, i, 0.0, &mut fixed);
        } else {
            let points = times
                .into_iter()
                .filter(|t| t.is_finite() && *t > 0.0 && steps > 0)
                .map(|t| (t / h - ACTION_TOLERANCE).ceil() as u64)
                .filter(|k| (1..steps).contains(k))
                .collect();
            switching.push((i, points));
        }
    }
    // The branches of one of these components after its actions up to the
    // fraction `after` of a time step past a grid point.
    let branches_at = |nodes: &mut Numbered, i: usize, grid_point: u64, after: f64| {
        let t = if steps > 0 {
            (grid_point as f64 + after) * h
        } else {
            0.0
        };
        let mut edges = Vec::new();
        nodes.branches(&model.components[i], i, t, &mut edges);
        edges
    };
    // The nodes to name: those of the model; those inside components only
    // when there are no others.
    let listed = |nodes: &Numbered, floating: &[usize]| -> Vec<String> {
        let names = floating.iter().map(|&n| nodes.names[n].clone());
        let of_model: Vec<String> = names.clone().filter(|n| !n.contains(':')).collect();
        if of_model.is_empty() {
            names.collect()
        } else {
            of_model
        }
    };

    // t = 0: the state the run starts in.
    let mut states: Vec<Vec<(usize, Edge)>> = switching
        .iter()
        .map(|(i, _)| branches_at(&mut nodes, *i, 0, ACTION_TOLERANCE))
        .collect();
    let mut start = topology(nodes.names.len(), &fixed, &states.concat());
    if sim.switching == Some(Switching::Interpolated) {
        // An action shortly after t = 0 may be taken at t = 0: an error is
        // what is there with and without such actions.
        let moved: Vec<Vec<(usize, Edge)>> = switching
            .iter()
            .map(|(i, _)| branches_at(&mut nodes, *i, 0, MOVED_ACTION))
            .collect();
        if moved != states {
            let other = topology(nodes.names.len(), &fixed, &moved.concat());
            start.loops.retain(|members| other.loops.contains(members));
            start.more_loops = start.more_loops.min(other.more_loops);
            start.floating.retain(|n| other.floating.contains(n));
        }
    }
    for members in &start.loops {
        d.push(
            Diagnostic::error(
                "E142",
                format!("components[{}]", members[0]),
                format!(
                "{} form a loop of ideal branches at t = 0: the current around the loop cannot \
                 be determined",
                named(model, members)
            ),
                LOOP_HINT,
            )
            .about_components(names_of(model, members)),
        );
    }
    if start.more_loops > 0 {
        d.push(Diagnostic::error(
            "E142",
            "components",
            format!("{} more loops of ideal branches at t = 0", start.more_loops),
            LOOP_HINT,
        ));
    }
    if !start.floating.is_empty() {
        let names = listed(&nodes, &start.floating);
        // The components at these nodes, or the ones the nodes are inside.
        let inside = |c: &Component| {
            let own = |n: &String| {
                n.split_once(':')
                    .is_some_and(|(owner, _)| owner == c.name())
            };
            names.iter().any(own)
        };
        let at: Vec<usize> = model
            .components
            .iter()
            .enumerate()
            .filter(|(_, c)| c.node_names().iter().any(|n| names.contains(n)) || inside(c))
            .map(|(i, _)| i)
            .collect();
        d.push(
            Diagnostic::error(
                "E143",
                "components",
                format!(
                    "nothing holds the voltage of the nodes {} at t = 0: the components at these \
                 nodes ({}) do not connect them to a node with a voltage",
                    names.join(", "),
                    named(model, &at)
                ),
                FLOATING_HINT,
            )
            .about_components(names_of(model, &at))
            .about_nodes(names.iter().filter(|n| !n.contains(':'))),
        );
    }

    // The states that the actions of the switches and faults lead to.
    let grid_points: BTreeSet<u64> = switching.iter().flat_map(|(_, k)| k).copied().collect();
    let mut known_loops = start.loops;
    let mut known_floating: BTreeSet<usize> = start.floating.into_iter().collect();
    let (mut loop_warnings, mut floating_warnings) = (0, 0);
    for k in grid_points {
        let mut acting: Vec<usize> = Vec::new();
        let mut changed = false;
        for ((i, points), state) in switching.iter().zip(states.iter_mut()) {
            if points.contains(&k) {
                acting.push(*i);
                let next = branches_at(&mut nodes, *i, k, ACTION_TOLERANCE);
                changed |= next != *state;
                *state = next;
            }
        }
        if !changed {
            continue;
        }
        let found = topology(nodes.names.len(), &fixed, &states.concat());
        let after = format!(
            "after the action of {} at t = {:e} s",
            named(model, &acting),
            k as f64 * h
        );
        for members in found.loops {
            if known_loops.contains(&members) {
                continue;
            }
            if loop_warnings < MAX_SWITCHING_WARNINGS {
                d.push(
                    Diagnostic::warning(
                        "W142",
                        format!("components[{}]", acting[0]),
                        format!(
                        "{after}, {} form a loop of ideal branches: the current around the loop \
                         cannot be determined and the run fails there (E200)",
                        named(model, &members)
                    ),
                        LOOP_HINT,
                    )
                    .about_components(names_of(model, &members)),
                );
            }
            loop_warnings += 1;
            known_loops.push(members);
        }
        let floating: Vec<usize> = found
            .floating
            .into_iter()
            .filter(|n| known_floating.insert(*n))
            .collect();
        if !floating.is_empty() {
            if floating_warnings < MAX_SWITCHING_WARNINGS {
                d.push(
                    Diagnostic::warning(
                        "W143",
                        format!("components[{}]", acting[0]),
                        format!(
                            "{after}, nothing holds the voltage of the nodes {}: the run fails \
                         there (E200), or at the current zero at which the switch opens",
                            listed(&nodes, &floating).join(", ")
                        ),
                        FLOATING_HINT,
                    )
                    .about_nodes(
                        listed(&nodes, &floating)
                            .into_iter()
                            .filter(|n| !n.contains(':')),
                    ),
                );
            }
            floating_warnings += 1;
        }
    }
}

/// Frequency of the sinusoidal sources: the first included sine source
/// (voltage, current or three-phase), or `None` without one. Under
/// steady-state initialization, validation makes every included source a sine
/// of this frequency.
pub fn source_frequency(model: &Model) -> Option<f64> {
    model
        .components
        .iter()
        .filter(|c| c.initialization() == ComponentInitialization::Include)
        .find_map(|c| c.spec().sine_frequency())
}

/// True when `c` connects two or more buses or non-ground nodes (a series
/// element rather than a shunt to `gnd`), so excluding it from the steady-state
/// phasor solution removes a series branch between buses/nodes.
fn is_series_component(c: &Component) -> bool {
    match c {
        Component::Transformer(_)
        | Component::ThreePhaseTransformer(_)
        | Component::ThreePhaseIdealTransformer(_)
        | Component::PiLine(_)
        | Component::BergeronLine(_) => true,
        Component::CoupledRl(r) => !r
            .buses
            .iter()
            .any(|b| b.nodes().iter().all(|n| n == GROUND)),
        Component::Resistor(_)
        | Component::Inductor(_)
        | Component::Capacitor(_)
        | Component::VoltageSource(_)
        | Component::CurrentSource(_)
        | Component::Switch(_)
        | Component::Diode(_)
        | Component::ControlledSwitch(_)
        | Component::Thyristor(_)
        | Component::NonlinearResistor(_)
        | Component::NonlinearInductor(_)
        | Component::ControlledVoltageSource(_)
        | Component::ControlledCurrentSource(_) => c
            .two_nodes()
            .is_some_and(|nodes| !nodes.iter().any(|n| n == GROUND)),
        Component::ThreePhaseSource(_)
        | Component::ThreePhaseLoad(_)
        | Component::ThreePhaseCurrentSource(_)
        | Component::ThreePhaseControlledSource(_)
        | Component::ThreePhaseControlledCurrentSource(_)
        | Component::SynchronousMachine(_)
        | Component::GridFollowingConverter(_)
        | Component::Fault(_) => false,
    }
}

/// Steady-state initialization needs a sinusoidal steady state: sine sources
/// of one frequency and no elements whose state follows from the solution.
fn check_initialization(model: &Model, d: &mut Vec<Diagnostic>) {
    if model.simulation.initialization != Initialization::SteadyState {
        return;
    }
    let mut frequencies: Vec<(String, f64)> = Vec::new();
    let mut has_excluded = false;
    for (i, c) in model.components.iter().enumerate() {
        if c.initialization() == ComponentInitialization::Exclude {
            has_excluded = true;
            if is_series_component(c) {
                d.push(
                    Diagnostic::warning(
                        "W185",
                        format!("components[{i}]"),
                        format!(
                            "\"{}\" ({}) has \"initialization\": \"exclude\" on a series \
                             component, which omits the entire element from the steady-state \
                             phasor solution (splitting the network or leaving connected bus \
                             voltages determined without that branch)",
                            c.name(),
                            c.type_name()
                        ),
                        "Remove \"initialization\": \"exclude\" from transformers, lines, or \
                         series branches. To start a shunt or magnetizing branch from zero, \
                         model it separately to \"gnd\" with \"initialization\": \"exclude\".",
                    )
                    .about_components([c.name()]),
                );
            }
            continue;
        }
        let p = format!("components[{i}]");
        match c.spec().steady_state() {
            SteadyState::Linear => {}
            SteadyState::Sine(f) => frequencies.push((p, f)),
            SteadyState::NotSine(field) => d.push(Diagnostic::error(
                "E180",
                format!("{p}.{field}"),
                format!(
                    "\"{}\" is not a sine source without offset, so the run has no sinusoidal \
                     steady state",
                    c.name()
                ),
                "Steady-state initialization needs sine sources of one frequency (offset 0; a dc \
                 source of 0 is allowed). Set \"initialization\": \"none\" to start from the \
                 initial values, or change the waveform.",
            )),
            SteadyState::NoPhasor => d.push(Diagnostic::error(
                "E182",
                p,
                format!(
                    "\"{}\" ({}) has a state that follows from the solution; its steady state is \
                     not a sine and has no phasor",
                    c.name(),
                    c.type_name()
                ),
                "Set \"initialization\": \"none\" for circuits with diodes or controlled switches, \
                 and let the run reach its periodic steady state.",
            )),
        }
    }
    if let Some((first_path, f0)) = frequencies.first().cloned() {
        let other: Vec<String> = frequencies
            .iter()
            .filter(|(_, f)| (f - f0).abs() > 1e-12 * f0)
            .map(|(p, f)| format!("{p} ({f} Hz)"))
            .collect();
        if !other.is_empty() {
            d.push(Diagnostic::error(
                "E181",
                "simulation.initialization",
                format!(
                    "the sources have different frequencies: {first_path} ({f0} Hz), {}",
                    other.join(", ")
                ),
                "A phasor solution has one frequency. Give every source the same frequency, or set \
                 \"initialization\": \"none\".",
            ));
        }
    } else {
        d.push(Diagnostic::warning(
            "W184",
            "simulation.initialization",
            "there is no sine source, so the steady state is zero everywhere",
            "The run starts from rest as with \"initialization\": \"none\".",
        ));
    }
    for (i, c) in model.components.iter().enumerate() {
        if c.initialization() == ComponentInitialization::Exclude {
            continue;
        }
        if c.spec().lacks_steady_state_phasor() {
            d.push(Diagnostic::warning(
                "W240",
                format!("components[{i}]"),
                format!(
                    "\"{}\" ({}) has no steady_state: it is 0 in the phasor solution",
                    c.name(),
                    c.type_name()
                ),
                "Give \"steady_state\" the phasor that the control signal produces from t = 0 on \
                 (rms and angle_deg, cosine reference). If the source is 0 at the start, the \
                 warning can be ignored.",
            ));
        }
    }
    if has_excluded {
        check_included_subcircuit(model, d);
    }
    if !model.controls.is_empty() {
        d.push(Diagnostic::warning(
            "W183",
            "controls",
            "the phasor solution covers the circuit only: the control blocks start at t = 0 from \
             their own initial values, evaluated on the circuit's steady state",
            "The run stays in the steady state when the controls start in theirs. Blocks with a \
             state (lag, lead_lag, transfer_function) start in the steady state of their input \
             at t = 0 unless initial_input is given; give integrator and pi blocks their value \
             (initial_value, initial_integral: a number or a signal at t = 0); write a set point \
             as the value at t = 0 with an initial_value block; a pll starts locked. A \
             controlled source needs its phasor in steady_state.",
        ));
    }
}

/// When some components have `initialization: "exclude"`, check that the
/// remaining (included) subcircuit is still solvable in the steady-state
/// phasor solution: non-empty, connected to gnd, and with every included
/// node's voltage held at t = 0.
fn check_included_subcircuit(model: &Model, d: &mut Vec<Diagnostic>) {
    let included: Vec<(usize, &Component)> = model
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| c.initialization() == ComponentInitialization::Include)
        .collect();
    if included.is_empty() {
        d.push(Diagnostic::error(
            "E183",
            "simulation.initialization",
            "every component has \"initialization\": \"exclude\", so no component remains in the \
             steady-state phasor solution",
            "Set \"initialization\": \"none\" in simulation, or keep at least the steady-state \
             part of the circuit in the phasor solution.",
        ));
        return;
    }
    let excluded: Vec<usize> = model
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| c.initialization() == ComponentInitialization::Exclude)
        .map(|(i, _)| i)
        .collect();
    let excluded_text = named(model, &excluded);

    let mut included_nodes: BTreeSet<String> = BTreeSet::new();
    for &(_, c) in &included {
        for n in c.node_names() {
            included_nodes.insert(n);
        }
    }
    let implicit_ground = included.iter().any(|&(_, c)| c.spec().grounds_implicitly());
    if !included_nodes.contains(GROUND) && !implicit_ground {
        let about: Vec<String> = included_nodes.iter().cloned().collect();
        d.push(
            Diagnostic::error(
                "E183",
                "simulation.initialization",
                format!(
                    "excluding {excluded_text} leaves the steady-state phasor subcircuit without \
                     a connection to \"gnd\""
                ),
                format!(
                    "Keep a path to \"gnd\" among the included components (not excluded by \
                     {excluded_text}), or set \"initialization\": \"none\"."
                ),
            )
            .about_components(names_of(model, &excluded))
            .about_nodes(about),
        );
        return;
    }

    let mut with_ground = included_nodes.clone();
    with_ground.insert(GROUND.to_string());
    let index: BTreeMap<&str, usize> = with_ground
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let mut sets = Sets::new(index.len());
    for &(_, c) in &included {
        for (a, b) in c.spec().connections() {
            let (Some(&ia), Some(&ib)) = (index.get(a.as_str()), index.get(b.as_str())) else {
                continue;
            };
            sets.join(ia, ib);
        }
    }
    let ground = sets.find(index[GROUND]);
    let disconnected: Vec<String> = with_ground
        .iter()
        .filter(|n| sets.find(index[n.as_str()]) != ground)
        .cloned()
        .collect();
    if !disconnected.is_empty() {
        d.push(
            Diagnostic::error(
                "E183",
                "simulation.initialization",
                format!(
                    "excluding {excluded_text} disconnects node(s) {} from \"gnd\" in the \
                     steady-state phasor solution",
                    disconnected.join(", ")
                ),
                format!(
                    "Keep a path to \"gnd\" for {} among the included components (or include \
                     {excluded_text}), or set \"initialization\": \"none\".",
                    disconnected.join(", ")
                ),
            )
            .about_components(names_of(model, &excluded))
            .about_nodes(disconnected),
        );
        return;
    }

    let h = model.simulation.time_step_s;
    let t = if positive(h) {
        ACTION_TOLERANCE * h
    } else {
        0.0
    };
    let mut nodes = Numbered {
        number: HashMap::from([(GROUND.to_string(), 0)]),
        names: vec![GROUND.to_string()],
    };
    let mut edges: Vec<(usize, Edge)> = Vec::new();
    for &(i, c) in &included {
        for n in c.node_names() {
            nodes.of(n);
        }
        nodes.branches(c, i, t, &mut edges);
    }
    let found = topology(nodes.names.len(), &edges, &[]);
    let is_included_node = |name: &str| {
        included_nodes.contains(name)
            || name
                .split_once(':')
                .is_some_and(|(owner, _)| included.iter().any(|&(_, c)| c.name() == owner))
    };
    let floating: Vec<String> = found
        .floating
        .iter()
        .map(|&n| nodes.names[n].clone())
        .filter(|n| is_included_node(n))
        .collect();
    if !floating.is_empty() {
        let display: Vec<String> = {
            let of_model: Vec<String> = floating
                .iter()
                .filter(|n| !n.contains(':'))
                .cloned()
                .collect();
            if of_model.is_empty() {
                floating.clone()
            } else {
                of_model
            }
        };
        d.push(
            Diagnostic::error(
                "E183",
                "simulation.initialization",
                format!(
                    "excluding {excluded_text} leaves the voltage of node(s) {} unheld in the \
                     steady-state phasor solution",
                    display.join(", ")
                ),
                format!(
                    "Keep a voltage-holding path for {} in the phasor solution (do not exclude \
                     {excluded_text}, or add a resistor to gnd), or set \"initialization\": \
                     \"none\".",
                    display.join(", ")
                ),
            )
            .about_components(names_of(model, &excluded))
            .about_nodes(display.iter().filter(|n| !n.contains(':'))),
        );
    }
}

/// A scan describes one linear network: components with two states (diodes,
/// controlled switches and thyristors) are in it with their blocking
/// resistance. The warning belongs to the model, so `validate`, `run` and
/// `scan` all give it; the scan itself adds nothing to it.
fn warn_of_states_in_a_scan(model: &Model, d: &mut Vec<Diagnostic>) {
    let names: Vec<&str> = model
        .components
        .iter()
        .filter(|c| c.spec().steady_state() == SteadyState::NoPhasor)
        .map(|c| c.name())
        .collect();
    if names.is_empty() {
        return;
    }
    d.push(Diagnostic::warning(
        "W192",
        "components",
        format!(
            "diodes, controlled switches and thyristors are taken as off (their blocking \
             resistance) in the frequency scan: {}",
            names.join(", ")
        ),
        "A scan describes one linear network. Replace an element by a resistor or a closed \
         switch to scan it conducting.",
    ));
}

/// The frequencies and the nodes and buses of a frequency scan.
fn check_scan(scan: &FrequencyScan, nodes: &BTreeSet<&str>, d: &mut Vec<Diagnostic>) {
    let range = |d: &mut Vec<Diagnostic>, path: &str, message: String, hint: &str| {
        d.push(Diagnostic::error(
            "E190",
            format!("frequency_scan.{path}"),
            message,
            hint.to_string(),
        ));
    };
    let by_range = scan.start_hz.is_some() || scan.stop_hz.is_some() || scan.points.is_some();
    match (&scan.frequencies_hz, by_range) {
        (Some(_), true) => range(
            d,
            "frequencies_hz",
            "both frequencies_hz and start_hz/stop_hz/points are given".to_string(),
            "Give either the list frequencies_hz or start_hz, stop_hz and points.",
        ),
        (None, false) => range(
            d,
            "start_hz",
            "no frequencies are given".to_string(),
            "Give start_hz, stop_hz and points (and \"spacing\": \"log\" for equal ratios), or a \
             list frequencies_hz.",
        ),
        (Some(list), false) => {
            if list.is_empty() || list.len() > MAX_SCAN_POINTS as usize {
                range(
                    d,
                    "frequencies_hz",
                    format!("frequencies_hz has {} entries", list.len()),
                    "Give 1 to 1000000 frequencies.",
                );
            }
            for (k, f) in list.iter().enumerate() {
                if !positive(*f) {
                    range(
                        d,
                        &format!("frequencies_hz[{k}]"),
                        format!("frequency {f} is not > 0"),
                        "Every frequency must be > 0 Hz.",
                    );
                }
            }
        }
        (None, true) => {
            let (start, stop, points) = (scan.start_hz, scan.stop_hz, scan.points);
            match (start, stop, points) {
                (Some(a), Some(b), Some(n)) => {
                    if !positive(a) {
                        range(
                            d,
                            "start_hz",
                            format!("start_hz is {a}"),
                            "The first frequency must be > 0 Hz.",
                        );
                    } else if !b.is_finite() || b < a {
                        range(
                            d,
                            "stop_hz",
                            format!("stop_hz ({b}) is less than start_hz ({a})"),
                            "Use stop_hz >= start_hz.",
                        );
                    } else if n == 0 || n > MAX_SCAN_POINTS || (n == 1 && b != a) {
                        range(
                            d,
                            "points",
                            format!("points is {n}"),
                            "Use 2 to 1000000 points (both ends included), or 1 with start_hz = stop_hz.",
                        );
                    }
                }
                _ => range(
                    d,
                    if start.is_none() {
                        "start_hz"
                    } else if stop.is_none() {
                        "stop_hz"
                    } else {
                        "points"
                    },
                    "start_hz, stop_hz and points must be given together".to_string(),
                    "Give all three, or a list frequencies_hz instead.",
                ),
            }
        }
    }
    if scan.nodes.is_empty() && scan.buses.is_empty() {
        d.push(Diagnostic::error(
            "E191",
            "frequency_scan",
            "the scan names no node and no bus",
            "List nodes (self and mutual impedances) under \"nodes\" or buses (sequence \
             impedances) under \"buses\".",
        ));
    }
    let twice = |d: &mut Vec<Diagnostic>, list: &[String], field: &str| {
        for (k, n) in list.iter().enumerate() {
            if let Some(first) = list[..k].iter().position(|other| other == n) {
                d.push(Diagnostic::error(
                    "E194",
                    format!("frequency_scan.{field}[{k}]"),
                    format!("\"{n}\" is already listed at {field}[{first}]"),
                    "List every node and bus once: scan.csv cannot hold two columns of one name.",
                ));
            }
        }
    };
    twice(d, &scan.nodes, "nodes");
    twice(d, &scan.buses, "buses");
    for (k, n) in scan.nodes.iter().enumerate() {
        check_node(n, format!("frequency_scan.nodes[{k}]"), nodes, d);
    }
    for (k, b) in scan.buses.iter().enumerate() {
        check_bus(b, &format!("frequency_scan.buses[{k}]"), nodes, d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::component_catalog;

    #[test]
    fn additions_are_listed_in_words() {
        assert_eq!(in_words(&[]), "");
        assert_eq!(in_words(&["a"]), "a");
        assert_eq!(in_words(&["a", "b"]), "a and b");
        assert_eq!(in_words(&["a", "b", "c"]), "a, b and c");
    }

    #[test]
    fn every_supported_version_after_the_first_has_its_additions() {
        let listed: Vec<&str> = VERSION_ADDITIONS.iter().map(|(v, _)| *v).collect();
        assert_eq!(listed, SUPPORTED_VERSIONS[1..]);
        assert_eq!(VERSION, SUPPORTED_VERSIONS[SUPPORTED_VERSIONS.len() - 1]);
    }

    /// The version that added a type, by the example of its catalog entry.
    fn since(info: &crate::catalog::ComponentInfo) -> &'static str {
        match info.category {
            "circuit" => {
                // A field that a later version added to the type is left
                // out: with it, the example needs that later version.
                let mut example = info.example.clone();
                if let Some(fields) = example.as_object_mut() {
                    for added_later in [
                        "saturation",
                        "zero_sequence_inverted",
                        "tap_changes",
                        "impedance_changes",
                    ] {
                        fields.remove(added_later);
                    }
                }
                let c: Component = serde_json::from_value(example).unwrap();
                c.spec().since()
            }
            _ => {
                let c: Control = serde_json::from_value(info.example.clone()).unwrap();
                c.spec().since()
            }
        }
    }

    #[test]
    fn every_type_is_listed_under_the_version_that_added_it() {
        for info in component_catalog() {
            let version = since(&info);
            assert!(
                SUPPORTED_VERSIONS.contains(&version),
                "{}: since {version}",
                info.type_name
            );
            // The first version needs no entry, and the blocks of the
            // version that added the controls are covered by "controls".
            if version == SUPPORTED_VERSIONS[0]
                || (info.category == "control" && version == CONTROLS_SINCE)
            {
                continue;
            }
            let (_, added) = VERSION_ADDITIONS
                .iter()
                .find(|(v, _)| *v == version)
                .unwrap();
            assert!(
                added.contains(&info.type_name),
                "{} is not in VERSION_ADDITIONS of {version}",
                info.type_name
            );
        }
    }

    fn model(version: &str) -> Model {
        let text = format!(
            r#"{{"format": "openkeito-model", "version": "{version}",
                "simulation": {{"time_step_s": 1e-5, "end_time_s": 1e-3}},
                "components": [
                  {{"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {{"type": "dc", "value": 1.0}}}},
                  {{"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 1.0}}
                ]}}"#
        );
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn every_supported_version_is_read_and_others_are_told_which_are() {
        for version in SUPPORTED_VERSIONS {
            assert!(validate(&model(version)).is_empty(), "version {version}");
        }
        let d = validate(&model("0.10"));
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, "E101");
        assert!(d[0].hint.contains("\"version\": \"0.9\""), "{}", d[0].hint);
        assert!(
            d[0].hint
                .contains("0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9"),
            "{}",
            d[0].hint
        );
    }

    #[test]
    fn the_hint_of_e102_names_the_additions_up_to_the_version_needed() {
        // A diode needs 0.2: the hint does not list what later versions add.
        let mut m = model("0.1");
        m.components.push(
            serde_json::from_str(r#"{"type": "diode", "name": "D1", "nodes": ["n1", "gnd"]}"#)
                .unwrap(),
        );
        let d = validate(&m);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, "E102");
        assert_eq!(
            d[0].hint,
            "Set \"version\": \"0.2\". Version 0.2 adds diode, controlled_switch, controls, \
             signal outputs and simulation.output_interval_steps."
        );
    }
}
