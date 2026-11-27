//! Validation rejects loops of ideal branches (E142) and nodes whose voltage
//! nothing holds (E143) only where the solver finds the equations singular
//! (E200).
//!
//! `Simulator::from_model` validates, so a rejected model cannot be run.
//! Each circuit is therefore written with a switch (or a fault) that brings
//! about the state in question at the time `at`: with `at` = 0 it is the
//! state at t = 0 and validation rejects the model; with `at` one time step
//! later it is the same circuit one step on, validation warns (W142, W143)
//! and the run fails there with E200.

use openkeito_core::{SimError, Simulator};
use serde_json::{json, Value};

const H: f64 = 1e-4;

/// A model of version 0.5; of version 0.10 with switching instants at the
/// grid points (the default of 0.5) when it has dependent sources.
fn model(components: Value) -> Value {
    let dependent = components.as_array().is_some_and(|c| {
        c.iter()
            .any(|c| ["vcvs", "vccs", "cccs", "ccvs"].contains(&c["type"].as_str().unwrap_or("")))
    });
    let mut m = json!({
        "format": "openkeito-model",
        "version": "0.5",
        "simulation": {"time_step_s": H, "end_time_s": 20.0 * H},
        "components": components,
    });
    if dependent {
        m["version"] = json!("0.10");
        m["simulation"]["switching"] = json!("grid");
    }
    m
}

/// The code of the errors with which validation rejects the circuit: one
/// error, or one per phase of a three-phase circuit.
fn rejected(components: Value) -> &'static str {
    let errors = openkeito_model::load_str(&model(components).to_string())
        .expect_err("validation rejects the model");
    assert!(errors.len() == 1 || errors.len() == 3, "{errors:#?}");
    assert!(
        errors.iter().all(|e| e.code == errors[0].code),
        "{errors:#?}"
    );
    errors[0].code
}

/// The codes of the warnings of validation, and the time and the unknown at
/// which the run finds its equations singular.
fn singular(components: Value) -> (Vec<&'static str>, f64, String) {
    let loaded = match openkeito_model::load_str(&model(components).to_string()) {
        Ok(loaded) => loaded,
        Err(e) => panic!("validation only warns: {e:#?}"),
    };
    let warnings = loaded.warnings.iter().map(|w| w.code).collect();
    let sim = Simulator::from_model(&loaded.model).expect("circuit builds");
    match sim.run(|_, _| Ok(())) {
        Err(e @ SimError::Singular { .. }) => {
            assert_eq!(e.to_diagnostic().code, "E200");
            let SimError::Singular {
                time_s, unknown, ..
            } = e
            else {
                unreachable!()
            };
            (warnings, time_s, unknown)
        }
        Ok(_) => panic!("the run completes"),
        Err(other) => panic!("{other:?}"),
    }
}

/// With the action at t = 0 the circuit is rejected with `error`; with the
/// action one step later validation gives `warning` and the run is singular
/// at that step. Returns the unknown that the solver names.
fn pair(circuit: impl Fn(f64) -> Value, error: &str, warning: &str) -> String {
    assert_eq!(rejected(circuit(0.0)), error);
    let (warnings, time_s, unknown) = singular(circuit(H));
    assert!(warnings.len() == 1 || warnings.len() == 3, "{warnings:?}");
    assert!(warnings.iter().all(|w| *w == warning), "{warnings:?}");
    assert!((time_s - H).abs() < 1e-9 * H, "singular at t = {time_s:e}");
    unknown
}

fn dc(name: &str, nodes: [&str; 2]) -> Value {
    json!({"type": "voltage_source", "name": name, "nodes": nodes,
           "waveform": {"type": "dc", "value": 1.0}})
}

fn current(name: &str, nodes: [&str; 2]) -> Value {
    json!({"type": "current_source", "name": name, "nodes": nodes,
           "waveform": {"type": "dc", "value": 1.0}})
}

fn resistor(name: &str, nodes: [&str; 2]) -> Value {
    json!({"type": "resistor", "name": name, "nodes": nodes, "resistance_ohm": 10.0})
}

fn closing(name: &str, nodes: [&str; 2], at: f64) -> Value {
    json!({"type": "switch", "name": name, "nodes": nodes, "close_time_s": at})
}

fn opening(name: &str, nodes: [&str; 2], at: f64) -> Value {
    json!({"type": "switch", "name": name, "nodes": nodes, "initially_closed": true,
           "open_time_s": at, "open_at_current_zero": false})
}

fn ideal_source(name: &str, bus: &str) -> Value {
    json!({"type": "three_phase_source", "name": name, "bus": bus,
           "line_voltage_rms_v": 6600.0, "frequency_hz": 50.0})
}

fn load(name: &str, bus: &str) -> Value {
    json!({"type": "three_phase_load", "name": name, "bus": bus, "rated_voltage_kv": 6.6,
           "rated_frequency_hz": 50.0, "active_power_mw": 1.0, "reactive_power_mvar": 0.5})
}

fn injection(name: &str, bus: &str) -> Value {
    json!({"type": "three_phase_current_source", "name": name, "bus": bus,
           "current_rms_a": 10.0, "frequency_hz": 50.0})
}

/// The switches that join the phases of two buses at `at`.
fn tie(from: &str, to: &str, at: f64) -> Vec<Value> {
    ["a", "b", "c"]
        .iter()
        .map(|p| {
            closing(
                &format!("TIE{p}"),
                [&format!("{from}.{p}"), &format!("{to}.{p}")],
                at,
            )
        })
        .collect()
}

#[test]
fn two_voltage_sources_in_parallel() {
    // E1 and E2 across the same two nodes, through a switch.
    pair(
        |at| {
            json!([
                dc("E1", ["n1", "gnd"]),
                dc("E2", ["n2", "gnd"]),
                closing("S1", ["n1", "n2"], at),
                resistor("R1", ["n1", "gnd"]),
            ])
        },
        "E142",
        "W142",
    );
    // Without the switch it is the same loop from the start.
    assert_eq!(
        rejected(json!([
            dc("E1", ["n1", "gnd"]),
            dc("E2", ["n1", "gnd"]),
            resistor("R1", ["n1", "gnd"]),
        ])),
        "E142"
    );
}

#[test]
fn a_voltage_source_short_circuited_by_a_switch() {
    pair(
        |at| {
            json!([
                dc("E1", ["n1", "gnd"]),
                resistor("R1", ["n1", "gnd"]),
                closing("S1", ["n1", "gnd"], at),
            ])
        },
        "E142",
        "W142",
    );
    // Two closed switches in parallel: no source in the loop.
    pair(
        |at| {
            json!([
                dc("E1", ["n1", "gnd"]),
                opening("S0", ["n1", "n2"], 1.0),
                closing("S1", ["n1", "n2"], at),
                resistor("R1", ["n2", "gnd"]),
            ])
        },
        "E142",
        "W142",
    );
}

#[test]
fn two_ideal_three_phase_sources_on_one_bus() {
    pair(
        |at| {
            let mut c = vec![
                ideal_source("S1", "b1"),
                ideal_source("S2", "b2"),
                load("LD", "b1"),
            ];
            c.extend(tie("b2", "b1", at));
            json!(c)
        },
        "E142",
        "W142",
    );
    assert_eq!(
        rejected(json!([
            ideal_source("S1", "b1"),
            ideal_source("S2", "b1"),
            load("LD", "b1"),
        ])),
        "E142"
    );
}

#[test]
fn an_ideal_transformer_between_two_ideal_sources() {
    let transformer = |shift: f64| {
        json!({"type": "three_phase_ideal_transformer", "name": "PST", "buses": ["b1", "b2"],
               "rated_voltage_kv": [6.6, 6.6], "phase_shift_deg": shift})
    };
    for shift in [0.0, 30.0] {
        pair(
            |at| {
                let mut c = vec![
                    ideal_source("S1", "b1"),
                    transformer(shift),
                    load("LD", "b2"),
                    ideal_source("S2", "b3"),
                ];
                c.extend(tie("b3", "b2", at));
                json!(c)
            },
            "E142",
            "W142",
        );
    }
    assert_eq!(
        rejected(json!([
            ideal_source("S1", "b1"),
            transformer(30.0),
            ideal_source("S2", "b2"),
            load("LD", "b2"),
        ])),
        "E142"
    );
}

#[test]
fn a_fault_without_resistance_at_an_ideal_source() {
    // The two faulted phases are joined without an impedance, and so are
    // the two sources of these phases.
    let circuit = |start: f64| {
        json!([
            ideal_source("S1", "b1"),
            load("LD", "b1"),
            {"type": "fault", "name": "F1", "bus": "b1", "phases": ["a", "b"],
             "start_time_s": start},
        ])
    };
    pair(circuit, "E142", "W142");
}

#[test]
fn a_node_fed_only_by_a_current_source() {
    let unknown = pair(
        |at| {
            json!([
                current("J1", ["gnd", "n1"]),
                opening("S1", ["n1", "n2"], at),
                resistor("R1", ["n2", "gnd"]),
            ])
        },
        "E143",
        "W143",
    );
    assert_eq!(unknown, "V(n1)");
    // Without the switch nothing holds the node from the start.
    assert_eq!(
        rejected(json!([
            dc("E1", ["n2", "gnd"]),
            resistor("R1", ["n2", "gnd"]),
            current("J1", ["gnd", "n1"]),
            current("J2", ["n1", "n2"]),
        ])),
        "E143"
    );
}

#[test]
fn a_node_behind_an_open_switch() {
    // Nothing but the switch at n2 and n3: the inductor joins the two, and
    // neither has a voltage once the switch is open.
    let unknown = pair(
        |at| {
            json!([
                dc("E1", ["n1", "gnd"]),
                resistor("R1", ["n1", "gnd"]),
                opening("S1", ["n1", "n2"], at),
                {"type": "inductor", "name": "L1", "nodes": ["n2", "n3"], "inductance_h": 1e-3},
            ])
        },
        "E143",
        "W143",
    );
    assert!(unknown == "V(n2)" || unknown == "V(n3)", "{unknown}");
    // A switch that never closes.
    assert_eq!(
        rejected(json!([
            dc("E1", ["n1", "gnd"]),
            resistor("R1", ["n1", "gnd"]),
            {"type": "switch", "name": "S1", "nodes": ["n1", "n2"]},
            current("J1", ["gnd", "n2"]),
        ])),
        "E143"
    );
}

#[test]
fn a_bus_that_only_a_fault_connects() {
    // The bus b2 has an injection and a fault to gnd. While the fault
    // lasts it holds the bus; before it starts and after it is cleared
    // nothing does.
    let network = |fault: Value| {
        json!([
            {"type": "three_phase_source", "name": "S1", "bus": "b1",
             "line_voltage_rms_v": 6600.0, "frequency_hz": 50.0,
             "impedance": {"type": "sequence", "r1_ohm": 0.1, "l1_h": 1e-3,
                           "r0_ohm": 0.1, "l0_h": 1e-3}},
            load("LD", "b1"),
            injection("J1", "b2"),
            fault,
        ])
    };
    let fault = |start: f64, clear: Option<f64>| {
        let mut f = json!({"type": "fault", "name": "F1", "bus": "b2",
            "phases": ["a", "b", "c"], "ground": true, "start_time_s": start,
            "clear_at_current_zero": false});
        if let Some(clear) = clear {
            f["clear_time_s"] = json!(clear);
        }
        f
    };
    // Not started at t = 0.
    assert_eq!(rejected(network(fault(5.0 * H, None))), "E143");
    // On from t = 0, cleared one step later.
    let (warnings, time_s, unknown) = singular(network(fault(0.0, Some(H))));
    assert_eq!(warnings, ["W143"]);
    assert!((time_s - H).abs() < 1e-9 * H, "{time_s:e}");
    assert!(unknown.starts_with("V(b2."), "{unknown}");
}

#[test]
fn the_ideal_branches_of_dependent_sources() {
    // The output of a vcvs and a voltage source in parallel: their currents
    // have the same column.
    pair(
        |at| {
            json!([
                dc("E1", ["n1", "gnd"]),
                resistor("R1", ["n1", "c"]),
                resistor("R2", ["c", "gnd"]),
                {"type": "vcvs", "name": "A1", "control": ["c", "gnd"], "output": ["n2", "gnd"], "gain_v_per_v": 3.0},
                closing("S1", ["n1", "n2"], at),
            ])
        },
        "E142",
        "W142",
    );
    // The sense port of a cccs short-circuited by a switch: the same row.
    pair(
        |at| {
            json!([
                dc("E1", ["n1", "gnd"]),
                resistor("R1", ["n1", "n2"]),
                {"type": "cccs", "name": "F1", "sense": ["n2", "gnd"], "output": ["gnd", "o"], "gain_a_per_a": 2.0},
                resistor("R2", ["o", "gnd"]),
                closing("S1", ["n2", "gnd"], at),
            ])
        },
        "E142",
        "W142",
    );
    // A node that the output of a cccs and a current source feed, once a
    // switch has opened.
    let unknown = pair(
        |at| {
            json!([
                dc("E1", ["n1", "gnd"]),
                resistor("R1", ["n1", "s"]),
                {"type": "cccs", "name": "F1", "sense": ["s", "gnd"], "output": ["gnd", "x"], "gain_a_per_a": 2.0},
                current("J1", ["x", "gnd"]),
                opening("S1", ["x", "y"], at),
                resistor("R2", ["y", "gnd"]),
            ])
        },
        "E143",
        "W143",
    );
    assert_eq!(unknown, "V(x)");
}

#[test]
fn circuits_that_validation_accepts_run() {
    // The same circuits with what the hints of E142 and E143 say: a resistor
    // in series in the loop, a resistor from the node to gnd.
    let completes = |components: Value| {
        let loaded = match openkeito_model::load_str(&model(components).to_string()) {
            Ok(loaded) => loaded,
            Err(e) => panic!("{e:#?}"),
        };
        assert!(loaded.warnings.is_empty(), "{:#?}", loaded.warnings);
        let sim = Simulator::from_model(&loaded.model).expect("circuit builds");
        sim.run(|_, _| Ok(())).expect("run completes");
    };
    completes(json!([
        dc("E1", ["n1", "gnd"]),
        dc("E2", ["n2", "gnd"]),
        closing("S1", ["n1", "n3"], H),
        {"type": "resistor", "name": "R2", "nodes": ["n3", "n2"], "resistance_ohm": 1e-3},
        resistor("R1", ["n1", "gnd"]),
    ]));
    completes(json!([
        current("J1", ["gnd", "n1"]),
        opening("S1", ["n1", "n2"], H),
        resistor("R1", ["n2", "gnd"]),
        {"type": "resistor", "name": "R2", "nodes": ["n1", "gnd"], "resistance_ohm": 1e6},
    ]));
    // A node between two capacitors, and one behind a blocking diode.
    completes(json!([
        dc("E1", ["n1", "gnd"]),
        {"type": "capacitor", "name": "C1", "nodes": ["n1", "n2"], "capacitance_f": 1e-6},
        {"type": "capacitor", "name": "C2", "nodes": ["n2", "gnd"], "capacitance_f": 1e-6},
        {"type": "diode", "name": "D1", "nodes": ["gnd", "n3"]},
        current("J1", ["gnd", "n3"]),
    ]));
    // A ccvs across its own sense port: r I = 0 holds the current (W144,
    // which `completes` does not allow, so the warning is checked here).
    let loop_of_gains = json!([
        current("J1", ["gnd", "o"]),
        resistor("R1", ["o", "gnd"]),
        {"type": "ccvs", "name": "H1", "sense": ["o", "gnd"], "output": ["o", "gnd"], "transresistance_ohm": 50.0},
    ]);
    let loaded = openkeito_model::load_str(&model(loop_of_gains).to_string()).unwrap();
    let warnings: Vec<&str> = loaded.warnings.iter().map(|w| w.code).collect();
    assert_eq!(warnings, ["W144"]);
    Simulator::from_model(&loaded.model)
        .unwrap()
        .run(|_, _| Ok(()))
        .expect("run completes");
    // An ideal transformer alone holds the bus behind it.
    completes(json!([
        ideal_source("S1", "b1"),
        {"type": "three_phase_ideal_transformer", "name": "PST", "buses": ["b1", "b2"],
         "rated_voltage_kv": [6.6, 6.6], "phase_shift_deg": 30.0},
        load("LD", "b2"),
    ]));
}

#[test]
fn weakly_supported_nodes_behind_open_switches_complete_without_false_singularity() {
    for solver in ["dense", "sparse"] {
        // 1) A node behind an open switch supported only by a 1000 H inductor
        //    (h = 50 us => g = 1 / (alpha L) = 5e-14 S at t = 0+): completes
        //    with V(x) = 0 while open, and I = V t / L once closed.
        let h = 50e-6;
        let close_at = 2.0 * h;
        let m_inductor = json!({
            "format": "openkeito-model",
            "version": "0.5",
            "simulation": {"time_step_s": h, "end_time_s": 10.0 * h, "linear_solver": solver},
            "components": [
                dc("E1", ["n1", "gnd"]),
                resistor("R1", ["n1", "gnd"]),
                {"type": "switch", "name": "S1", "nodes": ["n1", "x"], "close_time_s": close_at},
                {"type": "inductor", "name": "L1", "nodes": ["x", "gnd"], "inductance_h": 1000.0},
            ],
            "outputs": [
                {"type": "node_voltage", "node": "x"},
                {"type": "branch_current", "component": "L1"},
            ],
        });
        let loaded = openkeito_model::load_str(&m_inductor.to_string()).unwrap();
        let sim = Simulator::from_model(&loaded.model).unwrap();
        let mut rows = Vec::new();
        let stats = sim
            .run(|t, row| {
                rows.push((t, row.to_vec()));
                Ok(())
            })
            .unwrap();
        assert!(
            stats.max_condition_estimate < 1e9,
            "{solver}: {}",
            stats.max_condition_estimate
        );
        for &(t, ref row) in &rows {
            if t < close_at - 1e-12 {
                assert_eq!(row[0], 0.0, "{solver} at t={t}");
            } else {
                let expected_i = 1.0 * (t - close_at) / 1000.0;
                assert!(
                    (row[1] - expected_i).abs() < 1e-12,
                    "{solver} at t={t}: {} vs {expected_i}",
                    row[1]
                );
            }
        }

        // 2) A node behind an open switch supported only by a 1e14 ohm
        //    resistor completes with both "steady_state" and "none".
        for init in ["steady_state", "none"] {
            let m_res = json!({
                "format": "openkeito-model",
                "version": "0.5",
                "simulation": {
                    "time_step_s": h,
                    "end_time_s": 5.0 * h,
                    "initialization": init,
                    "linear_solver": solver,
                },
                "components": [
                    {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
                     "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}},
                    resistor("R1", ["n1", "gnd"]),
                    {"type": "switch", "name": "S1", "nodes": ["n1", "x"]},
                    {"type": "resistor", "name": "Rbig", "nodes": ["x", "gnd"], "resistance_ohm": 1e14},
                ],
                "outputs": [{"type": "node_voltage", "node": "x"}],
            });
            let loaded = openkeito_model::load_str(&m_res.to_string()).unwrap();
            let sim = Simulator::from_model(&loaded.model).unwrap();
            let mut rows = Vec::new();
            let stats = sim
                .run(|_, row| {
                    rows.push(row.to_vec());
                    Ok(())
                })
                .unwrap();
            assert!(
                stats.max_condition_estimate < 1e4,
                "{solver}/{init}: {}",
                stats.max_condition_estimate
            );
            assert!(
                rows.iter().all(|r| r[0] == 0.0),
                "{solver}/{init}: {rows:?}"
            );
        }

        // 3) Truly floating nodes (zero column) and dependent ideal-branch
        //    loops still fail as singular under both dense and sparse solvers.
        for components in [
            json!([
                dc("E1", ["n1", "gnd"]),
                resistor("R1", ["n1", "gnd"]),
                opening("S1", ["n1", "n2"], h),
                {"type": "inductor", "name": "L1", "nodes": ["n2", "n3"], "inductance_h": 1e-3},
            ]),
            json!([
                dc("E1", ["n1", "gnd"]),
                dc("E2", ["n2", "gnd"]),
                closing("S1", ["n1", "n2"], h),
                resistor("R1", ["n1", "gnd"]),
            ]),
        ] {
            let m = json!({
                "format": "openkeito-model",
                "version": "0.5",
                "simulation": {"time_step_s": h, "end_time_s": 5.0 * h, "linear_solver": solver},
                "components": components,
            });
            let loaded = openkeito_model::load_str(&m.to_string()).unwrap();
            let sim = Simulator::from_model(&loaded.model).unwrap();
            assert!(
                matches!(sim.run(|_, _| Ok(())), Err(SimError::Singular { .. })),
                "{solver} should stay singular"
            );
        }
    }
}
