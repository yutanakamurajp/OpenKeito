//! The validation of the dependent sources (`vcvs`, `vccs`, `cccs`, `ccvs`,
//! format 0.10) and the default of `simulation.switching` from 0.10 on.

use openkeito_model::{load_str, parse_str, topology, validate, Component, Switching};
use serde_json::{json, Value};

fn model(version: &str, components: Value) -> String {
    json!({
        "format": "openkeito-model",
        "version": version,
        "simulation": {"time_step_s": 1e-5, "end_time_s": 1e-4},
        "components": components,
    })
    .to_string()
}

/// The codes of the errors and warnings of a model of version 0.10.
fn codes(components: Value) -> Vec<&'static str> {
    let (_, d) = parse_str(&model("0.10", components)).expect("the model can be read");
    d.iter().map(|d| d.code).collect()
}

fn dc(name: &str, nodes: [&str; 2]) -> Value {
    json!({"type": "voltage_source", "name": name, "nodes": nodes, "waveform": {"type": "dc", "value": 1.0}})
}

fn r(name: &str, nodes: [&str; 2]) -> Value {
    json!({"type": "resistor", "name": name, "nodes": nodes, "resistance_ohm": 10.0})
}

fn vcvs(name: &str, control: [&str; 2], output: [&str; 2], k: f64) -> Value {
    json!({"type": "vcvs", "name": name, "control": control, "output": output, "gain_v_per_v": k})
}

#[test]
fn the_four_types_are_valid_in_0_10_and_not_before() {
    let components = json!([
        dc("V1", ["i", "gnd"]),
        r("R1", ["i", "c"]),
        r("R2", ["c", "gnd"]),
        vcvs("E1", ["c", "gnd"], ["e", "gnd"], 3.0),
        r("R3", ["e", "gnd"]),
        {"type": "vccs", "name": "G1", "control": ["c", "gnd"], "output": ["gnd", "g"], "transconductance_s": 0.1},
        r("R4", ["g", "gnd"]),
        {"type": "cccs", "name": "F1", "sense": ["e", "f"], "output": ["gnd", "h"], "gain_a_per_a": 2.0},
        r("R5", ["f", "gnd"]),
        r("R6", ["h", "gnd"]),
        {"type": "ccvs", "name": "H1", "sense": ["g", "k"], "output": ["m", "gnd"], "transresistance_ohm": 5.0},
        r("R7", ["k", "gnd"]),
        r("R8", ["m", "gnd"]),
    ]);
    let loaded = load_str(&model("0.10", components.clone())).unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    let e = load_str(&model("0.9", components)).unwrap_err();
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].code, "E102");
    for what in ["(vcvs)", "(vccs)", "(cccs)", "(ccvs)"] {
        assert!(e[0].message.contains(what), "{}", e[0].message);
    }
    assert!(
        e[0].hint
            .contains("Set \"version\": \"0.10\". Version 0.10 adds vcvs, vccs, cccs, ccvs, "),
        "{}",
        e[0].hint
    );
}

#[test]
fn from_0_10_on_switching_instants_are_interpolated_by_default() {
    let components = json!([dc("V1", ["i", "gnd"]), r("R1", ["i", "gnd"])]);
    let of = |version: &str, switching: Option<&str>| {
        let mut m: Value = serde_json::from_str(&model(version, components.clone())).unwrap();
        if let Some(s) = switching {
            m["simulation"]["switching"] = json!(s);
        }
        load_str(&m.to_string())
            .unwrap()
            .model
            .simulation
            .switching()
    };
    assert_eq!(of("0.10", None), Switching::Interpolated);
    assert_eq!(of("0.10", Some("grid")), Switching::Grid);
    assert_eq!(of("0.9", None), Switching::Grid);
    assert_eq!(of("0.5", None), Switching::Grid);
    assert_eq!(of("0.9", Some("interpolated")), Switching::Interpolated);
}

#[test]
fn the_ports_are_terminals_and_a_control_port_connects_nothing() {
    let components = json!([
        dc("V1", ["i", "gnd"]),
        r("R1", ["i", "gnd"]),
        vcvs("E1", ["i", "gnd"], ["e", "gnd"], 2.0),
        r("R2", ["e", "gnd"]),
    ]);
    let loaded = load_str(&model("0.10", components)).unwrap();
    let t = topology(&loaded.model, &[]);
    let e1 = t.components.iter().find(|c| c.name == "E1").unwrap();
    let names: Vec<&str> = e1.terminals.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        ["control[0]", "control[1]", "output[0]", "output[1]"]
    );
    assert_eq!(e1.terminals[0].path, "components[2].control[0]");
    let Component::Vcvs(v) = &loaded.model.components[2] else {
        panic!()
    };
    assert_eq!(v.gain_v_per_v, 2.0);

    // A node that only a control port reaches is not connected to gnd.
    let c = codes(json!([
        dc("V1", ["i", "gnd"]),
        r("R1", ["i", "gnd"]),
        vcvs("E1", ["x", "gnd"], ["e", "gnd"], 2.0),
        r("R2", ["e", "gnd"]),
    ]));
    assert_eq!(c, ["E141"]);
}

#[test]
fn a_port_with_both_nodes_at_one_node_and_a_gain_that_is_not_finite_are_errors() {
    let (_, d) = parse_str(&model(
        "0.10",
        json!([
            dc("V1", ["i", "gnd"]),
            r("R1", ["i", "gnd"]),
            vcvs("E1", ["i", "i"], ["e", "gnd"], 2.0),
            r("R2", ["e", "gnd"]),
            {"type": "cccs", "name": "F1", "sense": ["i", "gnd"], "output": ["f", "f"], "gain_a_per_a": 2.0},
        ]),
    ))
    .unwrap();
    let e123: Vec<&str> = d
        .iter()
        .filter(|d| d.code == "E123")
        .map(|d| d.path.as_str())
        .collect();
    assert_eq!(e123, ["components[2].control", "components[4].output"]);
    // Node f, which only the shorted output reaches, is not connected.
    let other: Vec<&str> = d.iter().map(|d| d.code).filter(|c| *c != "E123").collect();
    assert_eq!(other, ["E141"]);

    // A sense port and an output at the same two nodes are allowed (a
    // ccvs across its own sense port: W144).
    let c = codes(json!([
        {"type": "current_source", "name": "J1", "nodes": ["gnd", "o"], "waveform": {"type": "dc", "value": 1.0}},
        r("R1", ["o", "gnd"]),
        {"type": "ccvs", "name": "H1", "sense": ["o", "gnd"], "output": ["o", "gnd"], "transresistance_ohm": 50.0},
    ]));
    assert_eq!(c, ["W144"]);

    // JSON has no infinite numbers: a gain that is not finite comes from a
    // model built in code.
    let (mut m, _) = parse_str(&model(
        "0.10",
        json!([
            dc("V1", ["i", "gnd"]),
            r("R1", ["i", "gnd"]),
            vcvs("E1", ["i", "gnd"], ["e", "gnd"], 2.0),
            r("R2", ["e", "gnd"])
        ]),
    ))
    .unwrap();
    let Component::Vcvs(v) = &mut m.components[2] else {
        panic!()
    };
    v.gain_v_per_v = f64::INFINITY;
    let d = validate(&m);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(
        (d[0].code, d[0].path.as_str()),
        ("E130", "components[2].gain_v_per_v")
    );
}

#[test]
fn ideal_branches_of_dependent_sources_close_loops() {
    // The output of a vcvs in parallel with a voltage source: the columns
    // of the two currents are equal (E142).
    let c = codes(json!([
        dc("V1", ["i", "gnd"]),
        r("R1", ["i", "c"]),
        r("R2", ["c", "gnd"]),
        vcvs("E1", ["c", "gnd"], ["i", "gnd"], 2.0),
    ]));
    assert_eq!(c, ["E142"]);
    // The sense port of a cccs across a closed switch: the rows are equal.
    let c = codes(json!([
        dc("V1", ["i", "gnd"]),
        r("R1", ["i", "s"]),
        {"type": "cccs", "name": "F1", "sense": ["s", "gnd"], "output": ["gnd", "o"], "gain_a_per_a": 2.0},
        {"type": "switch", "name": "S1", "nodes": ["s", "gnd"], "initially_closed": true},
        r("R2", ["o", "gnd"]),
    ]));
    assert_eq!(c, ["E142"]);
    // The sense ports of two ccvs in parallel (equal rows) and their outputs
    // in parallel (equal columns): one loop of the two components.
    let c = codes(json!([
        dc("V1", ["i", "gnd"]),
        r("R1", ["i", "a"]),
        {"type": "ccvs", "name": "H1", "sense": ["a", "gnd"], "output": ["o", "gnd"], "transresistance_ohm": 2.0},
        {"type": "ccvs", "name": "H2", "sense": ["a", "gnd"], "output": ["o", "gnd"], "transresistance_ohm": 3.0},
        r("R2", ["o", "gnd"]),
    ]));
    assert_eq!(c, ["E142"]);
    // The output of a vccs or a cccs is a current source: a node that only
    // it and a current source reach is not held (E143).
    let c = codes(json!([
        dc("V1", ["i", "gnd"]),
        r("R1", ["i", "c"]),
        r("R2", ["c", "gnd"]),
        {"type": "vccs", "name": "G1", "control": ["c", "gnd"], "output": ["gnd", "x"], "transconductance_s": 0.1},
        {"type": "current_source", "name": "J1", "nodes": ["x", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
    ]));
    assert_eq!(c, ["E143"]);
    // A vcvs holds its output: a node that only the output reaches is held.
    let c = codes(json!([
        dc("V1", ["i", "gnd"]),
        r("R1", ["i", "gnd"]),
        vcvs("E1", ["i", "gnd"], ["e", "f"], 2.0),
        {"type": "current_source", "name": "J1", "nodes": ["f", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
        r("R2", ["e", "gnd"]),
    ]));
    assert!(c.is_empty(), "{c:?}");
}

#[test]
fn gains_that_close_a_loop_of_gain_one_are_errors() {
    // A vcvs of gain 1 that measures its own output.
    let own = |k: f64| {
        codes(json!([
            dc("V1", ["i", "gnd"]),
            r("R1", ["i", "o"]),
            vcvs("E1", ["o", "gnd"], ["o", "gnd"], k),
        ]))
    };
    assert_eq!(own(1.0), ["E144"]);
    for k in [0.0, 2.0, -1.0, 1.0 + 1e-9] {
        assert!(own(k).is_empty(), "k = {k}: {:?}", own(k));
    }
    // Two that measure each other: k1 k2 = 1.
    let mutual = |k1: f64, k2: f64| {
        codes(json!([
            r("R1", ["a", "gnd"]),
            r("R2", ["b", "gnd"]),
            vcvs("E1", ["b", "gnd"], ["a", "gnd"], k1),
            vcvs("E2", ["a", "gnd"], ["b", "gnd"], k2),
            dc("V1", ["i", "gnd"]),
            r("R3", ["i", "a"]),
        ]))
    };
    assert_eq!(mutual(4.0, 0.25), ["E144"]);
    assert_eq!(mutual(-2.0, -0.5), ["E144"]);
    assert!(mutual(4.0, 0.3).is_empty());
    // A cccs whose output current flows back through its own sense port
    // with gain 1: the current of R1 cannot be shared.
    let back = |beta: f64| {
        codes(json!([
            dc("V1", ["i", "gnd"]),
            r("R1", ["i", "a"]),
            {"type": "cccs", "name": "F1", "sense": ["a", "b"], "output": ["b", "a"], "gain_a_per_a": beta},
            r("R2", ["b", "gnd"]),
        ]))
    };
    assert_eq!(back(1.0), ["E144"]);
    assert!(back(0.5).is_empty());
    // The error names the sources and says what to change.
    let (_, d) = parse_str(&model(
        "0.10",
        json!([
            r("R1", ["a", "gnd"]),
            r("R2", ["b", "gnd"]),
            vcvs("E1", ["b", "gnd"], ["a", "gnd"], 4.0),
            vcvs("E2", ["a", "gnd"], ["b", "gnd"], 0.25),
            dc("V1", ["i", "gnd"]),
            r("R3", ["i", "a"]),
        ]),
    ))
    .unwrap();
    assert_eq!(d[0].path, "components[2]");
    assert!(
        d[0].message.contains("\"E1\" (vcvs) and \"E2\" (vcvs)"),
        "{}",
        d[0].message
    );
    assert!(d[0]
        .hint
        .contains("Change a gain, or put a resistor into the loop."));
    let about: Vec<&str> = d[0]
        .about
        .iter()
        .map(|s| match s {
            openkeito_model::Subject::Component(n) => n.as_str(),
            _ => "",
        })
        .collect();
    assert_eq!(about, ["E1", "E2"]);
}

#[test]
fn a_loop_of_a_ccvs_output_and_a_sense_port_is_a_warning() {
    // The output of a ccvs across its own sense port: r I = 0 holds the
    // current at 0 for r != 0 (W144), r = 0 leaves it free (E144).
    let loop_of = |rt: f64| {
        codes(json!([
            {"type": "current_source", "name": "J1", "nodes": ["gnd", "o"], "waveform": {"type": "dc", "value": 1.0}},
            r("R1", ["o", "gnd"]),
            {"type": "ccvs", "name": "H1", "sense": ["o", "gnd"], "output": ["o", "gnd"], "transresistance_ohm": rt},
        ]))
    };
    assert_eq!(loop_of(50.0), ["W144"]);
    // r = 0 is a singular gain: another r solves it (E144, "Change a gain").
    let (_, d) = parse_str(&model(
        "0.10",
        json!([
            {"type": "current_source", "name": "J1", "nodes": ["gnd", "o"], "waveform": {"type": "dc", "value": 1.0}},
            r("R1", ["o", "gnd"]),
            {"type": "ccvs", "name": "H1", "sense": ["o", "gnd"], "output": ["o", "gnd"], "transresistance_ohm": 0.0},
        ]),
    ))
    .unwrap();
    assert_eq!(d.iter().map(|d| d.code).collect::<Vec<_>>(), ["E144"]);
    assert!(
        d[0].message.contains("the gains of \"H1\" (ccvs)"),
        "{}",
        d[0].message
    );
    assert!(d[0].hint.contains("Change a gain"), "{}", d[0].hint);
}

#[test]
fn a_loop_of_a_vcvs_output_and_a_sense_port_is_singular_whatever_the_gains() {
    // The output of a vcvs across the sense port of a cccs: the current
    // around the loop is not determined for any gain (E145, not E144, and
    // not the warning W144).
    let shape = |k: f64| {
        let (_, d) = parse_str(&model(
            "0.10",
            json!([
                dc("V1", ["i", "gnd"]),
                r("R1", ["i", "c"]),
                r("R2", ["c", "gnd"]),
                vcvs("E1", ["c", "gnd"], ["s", "gnd"], k),
                {"type": "cccs", "name": "F1", "sense": ["s", "gnd"], "output": ["gnd", "o"], "gain_a_per_a": 2.0},
                r("R3", ["o", "gnd"]),
            ]),
        ))
        .unwrap();
        d
    };
    for k in [0.0, 0.5, 1.0, 2.0, -3.0] {
        let d = shape(k);
        assert_eq!(
            d.iter().map(|d| d.code).collect::<Vec<_>>(),
            ["E145"],
            "k = {k}"
        );
        assert_eq!(d[0].path, "components[3]");
        assert!(
            d[0].message
                .contains("whatever the gains of \"E1\" (vcvs) and \"F1\" (cccs)"),
            "{}",
            d[0].message
        );
        assert!(d[0].hint.contains("Put a small resistor"), "{}", d[0].hint);
        assert!(!d[0].hint.contains("Change a gain"), "{}", d[0].hint);
    }
    // A resistor in the loop solves it.
    let c = codes(json!([
        dc("V1", ["i", "gnd"]),
        r("R1", ["i", "c"]),
        r("R2", ["c", "gnd"]),
        vcvs("E1", ["c", "gnd"], ["s", "gnd"], 2.0),
        r("R4", ["s", "t"]),
        {"type": "cccs", "name": "F1", "sense": ["t", "gnd"], "output": ["gnd", "o"], "gain_a_per_a": 2.0},
        r("R3", ["o", "gnd"]),
    ]));
    assert!(c.is_empty(), "{c:?}");
    // The output of a vcvs across the sense port of a ccvs is singular the
    // same way; gain-caused errors stay E144 (a vcvs of gain 1 that measures
    // its own output, above).
    let c = codes(json!([
        dc("V1", ["i", "gnd"]),
        r("R1", ["i", "c"]),
        r("R2", ["c", "gnd"]),
        vcvs("E1", ["c", "gnd"], ["s", "gnd"], 2.0),
        {"type": "ccvs", "name": "H1", "sense": ["s", "gnd"], "output": ["o", "gnd"], "transresistance_ohm": 5.0},
        r("R3", ["o", "gnd"]),
    ]));
    assert_eq!(c, ["E145"]);
}

/// The vcvs E1 measures node c of the divider V1, R1, R2, and its output is
/// across the sense port of the cccs F1, whose output feeds node c.
fn loop_fed_back(k: f64, beta: f64) -> Value {
    json!([
        dc("V1", ["i", "gnd"]),
        r("R1", ["i", "c"]),
        r("R2", ["c", "gnd"]),
        vcvs("E1", ["c", "gnd"], ["s", "gnd"], k),
        {"type": "cccs", "name": "F1", "sense": ["s", "gnd"], "output": ["gnd", "c"], "gain_a_per_a": beta},
    ])
}

#[test]
fn a_loop_of_a_vcvs_output_and_a_sense_port_whose_current_acts_back_is_a_warning() {
    // The sense port holds V(s) = 0, so k V(c) = 0, and the cccs alone
    // feeds node c: beta I = -V(i) / R1 determines the current around the
    // loop for beta != 0 (W144; the run completes, the closed form is in the
    // tests of openkeito-core).
    for beta in [2.0, -0.5, 1.0] {
        let (_, d) = parse_str(&model("0.10", loop_fed_back(2.0, beta))).unwrap();
        assert_eq!(
            d.iter().map(|d| d.code).collect::<Vec<_>>(),
            ["W144"],
            "beta = {beta}"
        );
        assert!(
            d[0].message.contains("\"E1\" (vcvs) and \"F1\" (cccs)"),
            "{}",
            d[0].message
        );
        // The hint names both ways in which gains determine the current,
        // and the one that does not (E145).
        for words in [
            "outputs of vcvs or ccvs",
            "changes the voltage that the vcvs of the loop measures",
            "E145",
        ] {
            assert!(d[0].hint.contains(words), "{words}: {}", d[0].hint);
        }
    }
    // beta = 0 leaves the current free, and so does k = 0 (the rows of the
    // output and the sense port are then the same): another gain solves
    // both (E144, "Change a gain").
    for (k, beta) in [(2.0, 0.0), (0.0, 2.0)] {
        let (_, d) = parse_str(&model("0.10", loop_fed_back(k, beta))).unwrap();
        assert_eq!(
            d.iter().map(|d| d.code).collect::<Vec<_>>(),
            ["E144"],
            "k = {k}, beta = {beta}"
        );
        assert!(d[0].hint.contains("Change a gain"), "{}", d[0].hint);
    }
}

#[test]
fn a_measured_current_that_returns_but_does_not_change_the_measured_voltage_is_e145() {
    // The vcvs E1 measures node i, which V1 holds, and the cccs F1 feeds
    // node c: the measured current flows back into the circuit, but V(i)
    // does not depend on it, so nothing determines it (E145, not W144).
    let shape = |sense: [&str; 2], loop_resistor: Option<Value>| {
        let mut c = vec![
            dc("V1", ["i", "gnd"]),
            r("R1", ["i", "c"]),
            r("R2", ["c", "gnd"]),
            vcvs("E1", ["i", "gnd"], ["s", "gnd"], 2.0),
            json!({"type": "cccs", "name": "F1", "sense": sense, "output": ["gnd", "c"], "gain_a_per_a": 2.0}),
        ];
        c.extend(loop_resistor);
        let (_, d) = parse_str(&model("0.10", Value::Array(c))).unwrap();
        d
    };
    let d = shape(["s", "gnd"], None);
    assert_eq!(d.iter().map(|d| d.code).collect::<Vec<_>>(), ["E145"]);
    assert!(
        d[0].message
            .contains("whatever the gains of \"E1\" (vcvs) and \"F1\" (cccs)"),
        "{}",
        d[0].message
    );
    for words in [
        "changes the voltage that the vcvs",
        "a node whose voltage the vcvs does not measure",
        "Put a small resistor",
    ] {
        assert!(d[0].hint.contains(words), "{words}: {}", d[0].hint);
    }
    // 1e-3 ohm in the loop solves it.
    let d = shape(
        ["t", "gnd"],
        Some(
            json!({"type": "resistor", "name": "R4", "nodes": ["s", "t"], "resistance_ohm": 1e-3}),
        ),
    );
    assert!(d.is_empty(), "{d:?}");
}

/// A ladder of `n` resistors from node c to gnd, for a group of the circuit
/// that is too large for the check of the gains.
fn ladder(n: usize) -> Vec<Value> {
    (0..n)
        .flat_map(|j| {
            let from = if j == 0 {
                "c".to_string()
            } else {
                format!("l{j}")
            };
            let to = format!("l{}", j + 1);
            [
                r(&format!("RS{j}"), [&from, &to]),
                r(&format!("RP{j}"), [&to, "gnd"]),
            ]
        })
        .collect()
}

#[test]
fn a_group_too_large_for_the_check_of_the_gains_gives_the_warning_with_its_limit() {
    // The output of a vcvs across the sense port of a cccs whose output
    // feeds only R3: singular whatever the gains (E145) while the group is
    // small; with 600 more nodes on node c it is not checked, and the loop
    // of the two is a warning whose hint says why the run may stop with
    // E200 (the run is in the tests of openkeito-core).
    let circuit = |rungs: usize| {
        let mut c = vec![
            dc("V1", ["i", "gnd"]),
            r("R1", ["i", "c"]),
            r("R2", ["c", "gnd"]),
            vcvs("E1", ["c", "gnd"], ["s", "gnd"], 2.0),
            json!({"type": "cccs", "name": "F1", "sense": ["s", "gnd"], "output": ["gnd", "o"], "gain_a_per_a": 2.0}),
            r("R3", ["o", "gnd"]),
        ];
        c.extend(ladder(rungs));
        let (_, d) = parse_str(&model("0.10", Value::Array(c))).unwrap();
        d
    };
    let codes_of = |d: &[openkeito_model::Diagnostic]| d.iter().map(|d| d.code).collect::<Vec<_>>();
    assert_eq!(codes_of(&circuit(10)), ["E145"]);
    let d = circuit(600);
    assert_eq!(codes_of(&d), ["W144"]);
    for words in [
        "E145",
        "more than 600 unknowns",
        "If the run stops with E200, put a small resistor",
    ] {
        assert!(d[0].hint.contains(words), "{words}: {}", d[0].hint);
    }
    assert!(!d[0].hint.contains("outputs of ccvs and"), "{}", d[0].hint);
}

#[test]
fn a_node_that_only_control_ports_and_current_sources_reach_is_singular_whatever_the_gains() {
    // Node x: a current source (or the output of a vccs) and the control
    // port of a vcvs. Without the control port it is E143; the row of the
    // vcvs looks as if it held V(x), but V(x) only sets the output, and
    // nothing determines V(x) for any gain (E145).
    let fed_by = |source: Value| {
        let (_, d) = parse_str(&model(
            "0.10",
            json!([
                dc("V1", ["i", "gnd"]),
                r("R1", ["i", "gnd"]),
                source,
                vcvs("E1", ["x", "gnd"], ["e", "gnd"], 2.0),
                r("R2", ["e", "gnd"]),
            ]),
        ))
        .unwrap();
        d
    };
    let current = json!({"type": "current_source", "name": "J1", "nodes": ["gnd", "x"], "waveform": {"type": "dc", "value": 1.0}});
    let vccs = json!({"type": "vccs", "name": "G1", "control": ["i", "gnd"], "output": ["gnd", "x"], "transconductance_s": 0.1});
    for (source, members) in [
        (current, "\"E1\" (vcvs)"),
        (vccs, "\"G1\" (vccs) and \"E1\" (vcvs)"),
    ] {
        let d = fed_by(source);
        assert_eq!(d.iter().map(|d| d.code).collect::<Vec<_>>(), ["E145"]);
        assert!(
            d[0].message
                .contains(&format!("whatever the gains of {members}")),
            "{}",
            d[0].message
        );
        assert!(
            d[0].hint.contains(
                "A node that only current sources, the outputs of vccs and cccs and control ports \
                 reach needs a resistor to gnd"
            ),
            "{}",
            d[0].hint
        );
    }
    // A resistor from x to gnd holds it.
    let c = codes(json!([
        dc("V1", ["i", "gnd"]),
        r("R1", ["i", "gnd"]),
        {"type": "current_source", "name": "J1", "nodes": ["gnd", "x"], "waveform": {"type": "dc", "value": 1.0}},
        r("RX", ["x", "gnd"]),
        vcvs("E1", ["x", "gnd"], ["e", "gnd"], 2.0),
        r("R2", ["e", "gnd"]),
    ]));
    assert!(c.is_empty(), "{c:?}");
    // Without the control port the node is E143.
    let c = codes(json!([
        dc("V1", ["i", "gnd"]),
        r("R1", ["i", "gnd"]),
        {"type": "current_source", "name": "J1", "nodes": ["gnd", "x"], "waveform": {"type": "dc", "value": 1.0}},
    ]));
    assert_eq!(c, ["E143"]);
}
