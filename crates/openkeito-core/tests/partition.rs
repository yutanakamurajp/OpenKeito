//! A circuit divided at its travelling-wave lines (`simulation.partitioning`)
//! and solved by several threads (`simulation.threads`).
//!
//! What is checked, and against what:
//!
//! - Sequential and parallel runs of a divided circuit agree to the last
//!   bit: rows, events and counts. The parts of a step share nothing that
//!   one of them writes, so there is nothing for the order to change.
//! - Divided and undivided runs agree to rounding. The equations are the
//!   same; the matrix of the undivided run is block diagonal with the
//!   matrices of the parts as blocks, and only the order of the eliminations
//!   and of the sums differs. The bound is the one of `RunStats`: the
//!   condition estimate times the machine epsilon, relative to the largest
//!   values, per solution. The tests allow that bound for the whole run
//!   (the differences found are 1e-14 to 1e-12 of the largest value, far
//!   below it), and more only where diodes decide on the solution.
//! - A switching action factorizes the matrix of its part only.

mod common;

use common::{run_sim, Run};
use openkeito_core::{SimError, Simulator};
use serde_json::{json, Value};

const PER_KM: &str = r#"{"type": "sequence", "r1_ohm_per_km": 0.03, "l1_h_per_km": 0.9e-3,
    "c1_f_per_km": 12.7e-9, "r0_ohm_per_km": 0.2, "l0_h_per_km": 3e-3, "c0_f_per_km": 7e-9}"#;

fn per_km() -> Value {
    serde_json::from_str(PER_KM).unwrap()
}

/// A ring of `n` buses at 66 kV joined by 10 km Bergeron lines (travel
/// times 33.8 and 45.8 us), a load on every bus and a source behind a
/// transformer on every fourth bus. A three-phase fault on bus n/2 starts at
/// 2 ms and is cleared at the current zeros after 6 ms.
fn ring(n: usize, method: &str, fault: bool) -> Value {
    let w = 2.0 * std::f64::consts::PI * 50.0;
    let mut c = Vec::new();
    for k in 0..n {
        let next = (k + 1) % n;
        c.push(json!({"type": "bergeron_line", "name": format!("L{k}"),
            "buses": [format!("b{k}"), format!("b{next}")], "length_km": 10.0, "per_km": per_km()}));
        let (p, q, v) = (10e6, 3e6, 66e3);
        let s2 = p * p + q * q;
        let (r, x) = (v * v * p / s2, v * v * q / s2);
        c.push(json!({"type": "coupled_rl", "name": format!("LD{k}"), "buses": [format!("b{k}"), "gnd"],
            "impedance": {"type": "sequence", "r1_ohm": r, "l1_h": x / w, "r0_ohm": r, "l0_h": x / w}}));
        if k % 4 == 0 {
            c.push(json!({"type": "three_phase_source", "name": format!("G{k}"), "bus": format!("g{k}"),
                "line_voltage_rms_v": 11e3, "frequency_hz": 50.0,
                "impedance": {"type": "sequence", "r1_ohm": 0.01, "l1_h": 0.1 / w, "r0_ohm": 0.01, "l0_h": 0.1 / w}}));
            c.push(
                json!({"type": "three_phase_transformer", "name": format!("T{k}"),
                "buses": [format!("b{k}"), format!("g{k}")],
                "vector_group": "YNd11", "rated_voltage_kv": [66.0, 11.0], "rated_power_mva": 100.0,
                "rated_frequency_hz": 50.0, "impedance_percent": 12.0, "resistance_percent": 0.3,
                "no_load_current_percent": 0.2, "no_load_loss_kw": 50.0}),
            );
        }
    }
    let faulted = n / 2;
    if fault {
        c.push(json!({"type": "fault", "name": "F1", "bus": format!("b{faulted}"), "phases": ["a", "b", "c"],
            "ground": true, "start_time_s": 2e-3, "clear_time_s": 6e-3}));
    }
    json!({
        "format": "openkeito-model", "version": "0.5",
        "simulation": {"time_step_s": 10e-6, "end_time_s": 0.02, "method": method,
                       "initialization": "steady_state"},
        "components": c,
        "outputs": [{"type": "bus_voltage", "bus": format!("b{faulted}")},
                    {"type": "bus_voltage", "bus": "b1"},
                    {"type": "branch_current", "component": "L0"},
                    {"type": "branch_voltage", "component": "L1"},
                    {"type": "branch_current", "component": "LD2"},
                    {"type": "branch_current", "component": "T0"}],
    })
}

/// A source feeds a three-phase diode bridge with a smoothed load through a
/// 20 km Bergeron line, and beyond the bridge a second line with a load on
/// which a switch closes: three parts, with diodes in the middle one.
fn rectifier(switching: &str) -> Value {
    let mut c = vec![
        json!({"type": "three_phase_source", "name": "G1", "bus": "b1", "line_voltage_rms_v": 6600.0,
            "frequency_hz": 50.0,
            "impedance": {"type": "sequence", "r1_ohm": 0.05, "l1_h": 1e-3, "r0_ohm": 0.1, "l0_h": 3e-3}}),
        json!({"type": "bergeron_line", "name": "LINE1", "buses": ["b1", "b2"], "length_km": 20.0, "per_km": per_km()}),
        json!({"type": "coupled_rl", "name": "LD2", "buses": ["b2", "gnd"],
            "impedance": {"type": "sequence", "r1_ohm": 200.0, "l1_h": 0.1, "r0_ohm": 200.0, "l0_h": 0.1}}),
        json!({"type": "capacitor", "name": "CD", "nodes": ["p", "n"], "capacitance_f": 2e-3}),
        json!({"type": "resistor", "name": "RD", "nodes": ["p", "n"], "resistance_ohm": 50.0}),
        json!({"type": "resistor", "name": "RN", "nodes": ["n", "gnd"], "resistance_ohm": 1e4}),
        json!({"type": "bergeron_line", "name": "LINE2", "buses": ["b2", "b3"], "length_km": 20.0, "per_km": per_km()}),
        json!({"type": "coupled_rl", "name": "LD3", "buses": ["b3", "gnd"],
            "impedance": {"type": "sequence", "r1_ohm": 100.0, "l1_h": 0.05, "r0_ohm": 100.0, "l0_h": 0.05}}),
        json!({"type": "switch", "name": "S3", "nodes": ["b3.a", "s"], "close_time_s": 7.503e-3}),
        json!({"type": "resistor", "name": "RS", "nodes": ["s", "gnd"], "resistance_ohm": 20.0}),
    ];
    for ph in ["a", "b", "c"] {
        // A commutation inductance in front of every leg of the bridge.
        c.push(json!({"type": "inductor", "name": format!("LC{ph}"), "nodes": [format!("b2.{ph}"), format!("r.{ph}")],
            "inductance_h": 2e-3}));
        c.push(
            json!({"type": "diode", "name": format!("DP{ph}"), "nodes": [format!("r.{ph}"), "p"]}),
        );
        c.push(
            json!({"type": "diode", "name": format!("DN{ph}"), "nodes": ["n", format!("r.{ph}")]}),
        );
    }
    json!({
        "format": "openkeito-model", "version": "0.5",
        "simulation": {"time_step_s": 20e-6, "end_time_s": 0.03, "switching": switching},
        "components": c,
        "outputs": [{"type": "bus_voltage", "bus": "b2"}, {"type": "bus_voltage", "bus": "b3"},
                    {"type": "branch_voltage", "component": "RD"},
                    {"type": "branch_current", "component": "DPa"},
                    {"type": "branch_current", "component": "LINE2"},
                    {"type": "branch_current", "component": "S3"}],
    })
}

/// The machine of `examples/synchronous_machine.json` (stage iteration of
/// its speed terms) behind a 100 km Bergeron line to the infinite bus, with
/// a surge arrester (a nonlinear resistor, whose segments change the
/// matrix) at the far end and the fault at the machine: two parts, each
/// with elements that iterate.
fn machine_and_arrester() -> Value {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let text = std::fs::read_to_string(dir.join("synchronous_machine.json")).unwrap();
    let mut m: Value = serde_json::from_str(&text).unwrap();
    let components = m["components"].as_array_mut().unwrap();
    components.retain(|c| c["name"] != "LINE1" && c["name"] != "LINE2");
    components.push(
        json!({"type": "bergeron_line", "name": "LINE1", "buses": ["b1", "far"],
        "length_km": 100.0, "per_km": per_km()}),
    );
    components.push(json!({"type": "coupled_rl", "name": "TIE", "buses": ["far", "inf"],
        "impedance": {"type": "sequence", "r1_ohm": 0.02, "l1_h": 0.0013263, "r0_ohm": 0.06, "l0_h": 0.0039789}}));
    for ph in ["a", "b", "c"] {
        // Conducts from about 0.6 of 30 kV on: the peaks of the 24 kV
        // system lie on its curved part.
        components.push(json!({"type": "nonlinear_resistor", "name": format!("MOV{ph}"),
            "nodes": [format!("far.{ph}"), "gnd"],
            "characteristic": {"type": "power_law", "reference_voltage_v": 30e3, "max_current_a": 40e3,
                "segments": [{"coefficient_a": 0.01, "exponent": 12.0, "from_voltage_pu": 0.6},
                             {"coefficient_a": 0.0017985878990921362, "exponent": 30.0, "from_voltage_pu": 1.1}]}}));
    }
    m["simulation"]["end_time_s"] = 0.3.into();
    m["simulation"]["output_interval_steps"] = 1.into();
    m["outputs"].as_array_mut().unwrap().extend([
        json!({"type": "bus_voltage", "bus": "far"}),
        json!({"type": "branch_current", "component": "MOVa"}),
    ]);
    m
}

/// `examples/converter.json` (a grid-following converter: 38 control
/// blocks and controlled sources that follow their signals) with a
/// synchronous machine behind a 100 km Bergeron line on its bus: the
/// converter with the grid in one part, the machine in the other. A fault
/// at the machine and the step of the power command of the example.
fn machine_and_converter() -> Value {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let read = |name: &str| -> Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join(name)).unwrap()).unwrap()
    };
    let mut m = read("converter.json");
    let mut machine = read("synchronous_machine.json")["components"][0].clone();
    assert_eq!(machine["type"], "synchronous_machine");
    machine["name"] = "SM1".into();
    machine["bus"] = "m1".into();
    machine["rated_power_mva"] = 2.0.into();
    machine["rated_voltage_kv"] = 6.6.into();
    machine["frequency_hz"] = 50.0.into();
    let components = m["components"].as_array_mut().unwrap();
    components.push(machine);
    components.push(
        json!({"type": "bergeron_line", "name": "LINE1", "buses": ["b1", "m1"],
        "length_km": 100.0, "per_km": per_km()}),
    );
    components.push(
        json!({"type": "three_phase_load", "name": "LDM", "bus": "m1", "rated_voltage_kv": 6.6,
        "rated_frequency_hz": 50.0, "active_power_mw": 0.5, "reactive_power_mvar": 0.1}),
    );
    components.push(
        json!({"type": "fault", "name": "F1", "bus": "m1", "phases": ["a", "b", "c"],
        "ground": true, "fault_resistance_ohm": 1.0, "ground_resistance_ohm": 1.0,
        "start_time_s": 0.15, "clear_time_s": 0.2}),
    );
    m["simulation"]["end_time_s"] = 0.3.into();
    m["outputs"].as_array_mut().unwrap().extend([
        json!({"type": "bus_voltage", "bus": "m1"}),
        json!({"type": "branch_current", "component": "SM1"}),
        json!({"type": "branch_current", "component": "LINE1"}),
    ]);
    m
}

fn simulator(model: &Value, partitioning: Option<&str>) -> Simulator {
    let mut model = model.clone();
    if let Some(p) = partitioning {
        model["simulation"]["partitioning"] = p.into();
    }
    let loaded = openkeito_model::load_str(&model.to_string()).expect("valid model");
    match Simulator::from_model(&loaded.model) {
        Ok(sim) => sim,
        Err(diagnostics) => panic!("{diagnostics:?}"),
    }
}

/// Runs `model` not divided ("off") or divided ("every_line", "auto") on exactly
/// `threads` threads (fewer when the parts are fewer).
fn run(model: &Value, partitioning: &str, threads: usize) -> Run {
    let mut sim = simulator(model, Some(partitioning));
    sim.use_threads(threads);
    let h = model["simulation"]["time_step_s"].as_f64().unwrap();
    run_sim(sim, h)
}

/// Everything of a run that must not depend on the number of threads.
fn fingerprint(r: &Run) -> (Vec<Vec<u64>>, String) {
    let bits =
        r.t.iter()
            .zip(&r.rows)
            .map(|(t, row)| {
                std::iter::once(t.to_bits())
                    .chain(row.iter().map(|v| v.to_bits()))
                    .collect()
            })
            .collect();
    let s = &r.stats;
    let counts = format!(
        "{:?} {} {} {} {} {} {} {} {} {} {} {:?} {:?} {:?}",
        s.events
            .iter()
            .map(|e| (
                e.time_s.to_bits(),
                &e.component,
                e.action,
                e.interrupted_current_a.map(f64::to_bits)
            ))
            .collect::<Vec<_>>(),
        s.steps,
        s.rows,
        s.factorizations,
        s.solves,
        s.resolves,
        s.operating_point_iterations,
        s.max_operating_point_iterations,
        s.instants_within_steps,
        s.split_steps,
        s.max_condition_estimate.to_bits(),
        s.initialization.as_ref().map(|i| (
            i.max_voltage_deviation_v.to_bits(),
            i.max_current_deviation_a.map(f64::to_bits)
        )),
        s.warnings.iter().map(|w| w.code).collect::<Vec<_>>(),
        (s.parts, s.largest_part_unknowns, s.links),
    );
    (bits, counts)
}

/// Largest difference between the rows of two runs, relative to the largest
/// value of each column.
fn difference(a: &Run, b: &Run) -> f64 {
    assert_eq!(a.rows.len(), b.rows.len());
    let columns = a.rows[0].len();
    let mut worst = 0.0_f64;
    for c in 0..columns {
        let largest = a.rows.iter().fold(0.0_f64, |m, r| m.max(r[c].abs()));
        let apart = a
            .rows
            .iter()
            .zip(&b.rows)
            .fold(0.0_f64, |m, (ra, rb)| m.max((ra[c] - rb[c]).abs()));
        if largest > 0.0 {
            worst = worst.max(apart / largest);
        }
    }
    worst
}

/// The same events at the same times.
fn same_events(a: &Run, b: &Run) {
    let list = |r: &Run| -> Vec<(u64, String, &'static str)> {
        r.stats
            .events
            .iter()
            .map(|e| (e.time_s.to_bits(), e.component.clone(), e.action))
            .collect()
    };
    assert_eq!(list(a), list(b));
}

/// Checks a model as the header says: divided runs on 1, 2, 3 and 8 threads
/// to the last bit, and the divided run against the undivided one to
/// `allowed` (relative), or the rounding bound when `None`. Returns the
/// undivided and the divided run.
fn check(model: &Value, parts: usize, allowed: Option<f64>) -> (Run, Run) {
    let whole = run(model, "off", 1);
    let divided = run(model, "every_line", 1);
    assert_eq!(whole.stats.parts, 1);
    assert_eq!(whole.stats.threads, 1);
    assert_eq!(divided.stats.parts, parts);
    assert_eq!(divided.stats.threads, 1);
    assert_eq!(divided.stats.unknowns, whole.stats.unknowns);
    let reference = fingerprint(&divided);
    for threads in [2, 3, 8] {
        let parallel = run(model, "every_line", threads);
        assert_eq!(parallel.stats.threads, threads.min(parts));
        let found = fingerprint(&parallel);
        assert!(
            found.1 == reference.1,
            "{threads} threads: {} / {}",
            found.1,
            reference.1
        );
        assert!(found.0 == reference.0, "{threads} threads: rows differ");
    }
    let bound = whole.stats.max_condition_estimate * f64::EPSILON;
    let apart = difference(&whole, &divided);
    println!(
        "{parts} parts: divided against undivided {apart:.1e} of the largest values; \
         condition estimate {:.1e} (bound {bound:.1e})",
        whole.stats.max_condition_estimate
    );
    assert!(
        apart <= allowed.unwrap_or(bound),
        "{apart:e} against {bound:e}"
    );
    (whole, divided)
}

#[test]
fn a_ring_divides_into_its_buses_and_gives_the_same_result() {
    for method in ["2s-dirk", "trapezoidal"] {
        let (whole, divided) = check(&ring(12, method, true), 12, None);
        same_events(&whole, &divided);
        assert_eq!(divided.stats.links, 12);
        // A bus with a transformer: 3 bus nodes, 3 nodes of the source,
        // currents of the windings and of the source.
        assert!(divided.stats.largest_part_unknowns < whole.stats.unknowns / 2);
        // The fault and its clearing happened: every phase closed and
        // opened at its current zero.
        assert_eq!(divided.stats.events.len(), 6);
        // The runs start on the steady state and stay on it until the fault.
        let init = divided.stats.initialization.as_ref().unwrap();
        assert!(init.relative_voltage_deviation < 1e-4, "{init:?}");
    }
}

#[test]
fn a_fault_factorizes_the_matrix_of_its_part_only() {
    // Without the fault, a run factorizes every matrix twice at the start
    // (for t = 0+ and for the regular stages), after the one of the steady
    // state. With the fault, the matrix of the faulted bus again at every
    // instant with an action: the fault and the three current zeros at
    // which it is cleared. The undivided run factorizes its one matrix as
    // often. (The half steps that follow an action under the trapezoidal
    // rule have the stage coefficient of its regular steps.)
    let parts = 12;
    let count = |method: &str, partitioning: &str, fault: bool| {
        run(&ring(parts, method, fault), partitioning, 1)
            .stats
            .factorizations
    };
    for method in ["2s-dirk", "trapezoidal"] {
        assert_eq!(count(method, "off", false), 1 + 2);
        assert_eq!(count(method, "every_line", false), 1 + 2 * parts as u64);
        assert_eq!(count(method, "off", true), 1 + 2 + 4);
        assert_eq!(count(method, "every_line", true), 1 + 2 * parts as u64 + 4);
    }
}

#[test]
fn diodes_in_one_part_switch_as_in_the_undivided_circuit() {
    for switching in ["grid", "interpolated"] {
        // The diode states are decided on the solution, with thresholds far
        // above rounding (1e-6 of the largest values): both runs take the
        // same decisions at the same instants, and the waveforms between
        // them agree to rounding. The blocking resistances make the
        // condition estimate 1e12, so the bound is 2e-4; the runs are 1e-10
        // (grid) and 1e-9 (interpolated) apart. Instants within a step are
        // placed by interpolation on the solution, so they agree to
        // rounding too, not to the last bit: the events are compared by
        // name and their times to 1e-12 s.
        let model = rectifier(switching);
        let (whole, divided) = check(&model, 3, None);
        let names = |r: &Run| -> Vec<(String, &'static str)> {
            r.stats
                .events
                .iter()
                .map(|e| (e.component.clone(), e.action))
                .collect()
        };
        assert_eq!(names(&whole), names(&divided));
        assert!(
            divided.stats.events.len() > 10,
            "{}",
            divided.stats.events.len()
        );
        for (a, b) in whole.stats.events.iter().zip(&divided.stats.events) {
            assert!((a.time_s - b.time_s).abs() < 1e-12, "{a:?} {b:?}");
        }
        if switching == "interpolated" {
            assert!(divided.stats.split_steps > 10);
        }
    }
}

#[test]
fn stage_iterations_stay_within_their_part() {
    // The matrices are well conditioned (estimate 4e2), so the bound of one
    // solution is 9e-14. The run has 3000 steps, each iterated to the
    // tolerance of the machine (1e-10 relative), and the swing of the
    // machine after the fault carries a difference on: 1e-9 for the run
    // (found: 3e-13).
    let model = machine_and_arrester();
    let (whole, divided) = check(&model, 2, Some(1e-9));
    same_events(&whole, &divided);
    // Both parts iterate: the machine (right-hand sides) and the arresters
    // (matrix).
    assert!(divided.stats.operating_point_iterations > 0);
    assert!(divided.stats.max_operating_point_iterations >= 1);
    // An iteration solves one part again, not the circuit: fewer
    // additional solutions than iterations of the undivided run would need
    // for both parts is not required, but no more.
    assert!(
        divided.stats.operating_point_iterations <= 2 * whole.stats.operating_point_iterations + 2
    );
}

#[test]
fn a_machine_and_a_converter_in_two_parts_do_not_depend_on_the_threads() {
    // The controls are evaluated for the whole circuit at the grid points;
    // the sources of the converter follow them in one part while the
    // machine iterates its stages in the other. The condition estimate is
    // 7e2 (bound of one solution 2e-13); over the 6000 steps with the
    // fault, the swing of the machine and the control loops, 1e-8 is
    // allowed for the run (found: 3e-13).
    let model = machine_and_converter();
    let (whole, divided) = check(&model, 2, Some(1e-8));
    same_events(&whole, &divided);
    assert!(divided.stats.operating_point_iterations > 0);
    // The fault closes in three phases; two of them have opened at a
    // current zero by the end of the run.
    assert_eq!(divided.stats.events.len(), 5, "{:?}", divided.stats.events);
}

#[test]
fn a_circuit_that_other_elements_hold_together_stays_one_part() {
    // A pi line beside one of the Bergeron lines of a ring of four buses:
    // its two buses are one part, the other buses one each.
    let mut model = ring(4, "2s-dirk", true);
    let components = model["components"].as_array_mut().unwrap();
    components.push(
        json!({"type": "pi_line", "name": "P0", "buses": ["b0", "b1"], "length_km": 10.0,
        "per_km": per_km()}),
    );
    let (_, divided) = check(&model, 3, None);
    // L0 lies within the part, the other three lines join the parts.
    assert_eq!(divided.stats.links, 3);
    // A circuit without such lines is one part, and the result is the one
    // of "off" to the last bit.
    for c in model["components"].as_array_mut().unwrap() {
        if c["type"] == "bergeron_line" {
            c["type"] = "pi_line".into();
        }
    }
    let (whole, divided) = (run(&model, "off", 1), run(&model, "every_line", 4));
    assert_eq!((divided.stats.parts, divided.stats.threads), (1, 1));
    assert!(fingerprint(&whole) == fingerprint(&divided));
}

#[test]
fn the_default_divides_into_parts_that_are_not_small() {
    // "auto", the default: parts that a line joins stay together up to 48
    // named nodes (16 buses). A ring of 12 buses (with the 3 buses of its
    // sources, 45 nodes) is one part; one of 40 buses (150 nodes) is four
    // parts. One thread solves them.
    let small = simulator(&ring(12, "2s-dirk", false), None);
    assert_eq!(small.parts(), [small.unknowns()]);
    let large = simulator(&ring(40, "2s-dirk", false), None);
    assert_eq!(large.parts().len(), 4);
    assert_eq!(large.parts().iter().sum::<usize>(), large.unknowns());
    let stats = large.run(|_, _| Ok(())).unwrap();
    assert_eq!((stats.parts, stats.links, stats.threads), (4, 4, 1));
    // "every_line" and "off".
    let fine = simulator(&ring(40, "2s-dirk", false), Some("every_line"));
    assert_eq!(fine.parts().len(), 40);
    let off = simulator(&ring(40, "2s-dirk", false), Some("off"));
    assert_eq!(off.parts(), [off.unknowns()]);
    // The three agree to rounding, with the fault.
    let model = ring(40, "2s-dirk", true);
    let whole = run(&model, "off", 1);
    let bound = whole.stats.max_condition_estimate * f64::EPSILON;
    for (partitioning, parts) in [("auto", 4), ("every_line", 40)] {
        let divided = run(&model, partitioning, 2);
        assert_eq!(divided.stats.parts, parts);
        same_events(&whole, &divided);
        let apart = difference(&whole, &divided);
        assert!(
            apart <= bound,
            "{partitioning}: {apart:e} against {bound:e}"
        );
    }
}

#[test]
fn a_singular_part_is_named_by_its_unknown() {
    // A node of the last bus that only a current source feeds, once the
    // switch that holds it has opened at the first step (from the start,
    // validation would reject the model with E143): the equations of that
    // part are singular, and the error names the node whether the circuit
    // is divided or not.
    let mut model = ring(4, "2s-dirk", false);
    model["simulation"]["initialization"] = "none".into();
    let components = model["components"].as_array_mut().unwrap();
    components.push(
        json!({"type": "current_source", "name": "J1", "nodes": ["b3.a", "loose"],
        "waveform": {"type": "dc", "value": 1.0}}),
    );
    components.push(
        json!({"type": "switch", "name": "HOLD", "nodes": ["loose", "gnd"],
        "initially_closed": true, "open_time_s": 10e-6, "open_at_current_zero": false}),
    );
    for partitioning in ["off", "every_line"] {
        let sim = simulator(&model, Some(partitioning));
        match sim.run(|_, _| Ok(())) {
            Err(SimError::Singular { unknown, .. }) => assert_eq!(unknown, "V(loose)"),
            other => panic!("{partitioning}: {other:?}"),
        }
    }
}

#[test]
fn threads_do_not_change_the_rows_of_a_model_that_asks_for_them() {
    // `simulation.threads` of the model: the solver may use fewer (small
    // parts, few cores), and the rows are those of one thread either way.
    let mut model = ring(12, "2s-dirk", true);
    let one = run(&model, "every_line", 1);
    model["simulation"]["threads"] = 4.into();
    let sim = simulator(&model, Some("every_line"));
    let h = model["simulation"]["time_step_s"].as_f64().unwrap();
    let asked = run_sim(sim, h);
    assert!((1..=4).contains(&asked.stats.threads));
    assert!(fingerprint(&asked).0 == fingerprint(&one).0);
}
