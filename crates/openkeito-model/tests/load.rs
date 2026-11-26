use openkeito_model::{load_str, Severity};

const VALID: &str = r#"{
  "format": "openkeito-model",
  "version": "0.1",
  "simulation": {"time_step_s": 1e-5, "end_time_s": 1e-3},
  "components": [
    {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
    {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 1000.0},
    {"type": "capacitor", "name": "C1", "nodes": ["n2", "gnd"], "capacitance_f": 1e-6}
  ]
}"#;

fn codes(text: &str) -> Vec<&'static str> {
    load_str(text)
        .expect_err("model must be rejected")
        .iter()
        .map(|d| d.code)
        .collect()
}

#[test]
fn a_valid_model_loads_without_warnings() {
    let loaded = load_str(VALID).unwrap();
    assert!(loaded.warnings.is_empty());
    assert_eq!(loaded.model.components.len(), 3);
    assert_eq!(openkeito_model::step_count(&loaded.model.simulation), 100);
}

#[test]
fn a_number_without_exponent_letter_is_an_error_with_a_fix() {
    let text = VALID.replace("1e-6", "1.1141-6");
    let errors = load_str(&text).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, "E001");
    assert!(errors[0].path.starts_with("line 8"), "{}", errors[0].path);
    assert!(errors[0].hint.contains("1.1141e-6"), "{}", errors[0].hint);
}

#[test]
fn unknown_and_misspelled_fields_are_errors() {
    assert_eq!(
        codes(&VALID.replace("resistance_ohm", "resistance")),
        ["E001", "E001"]
    );
    assert_eq!(
        codes(&VALID.replace(
            "\"capacitance_f\": 1e-6",
            "\"capacitance_f\": 1e-6, \"esr\": 0.1"
        )),
        ["E001"]
    );
    assert_eq!(
        codes(&VALID.replace("\"resistor\"", "\"resistance\"")),
        ["E001"]
    );
}

#[test]
fn a_read_error_says_what_to_write_and_where() {
    let first = |text: &str| load_str(text).unwrap_err().remove(0);
    let line_of = |text: &str, what: &str| {
        let k = text.lines().position(|l| l.contains(what)).unwrap() + 1;
        let column = text.lines().nth(k - 1).unwrap().find(what).unwrap() + 1;
        format!("line {k} column {column}")
    };
    // A field without its unit: the hint names the field that was meant,
    // and the position is that of the name, not the end of the component
    // (where the parser stops: it reads a component as a whole first).
    let text = VALID.replace("\"resistance_ohm\"", "\"resistance\"");
    let d = first(&text);
    assert_eq!(d.path, line_of(&text, "\"resistance\""));
    assert!(
        d.hint
            .starts_with("Write \"resistance_ohm\" in place of \"resistance\"."),
        "{}",
        d.hint
    );
    assert!(d.hint.contains("openkeito schema"), "{}", d.hint);
    assert!(!d.message.contains(" at line "), "{}", d.message);
    // A field that the component does not have and that resembles none.
    let text = VALID.replace(
        "\"capacitance_f\": 1e-6",
        "\"capacitance_f\": 1e-6, \"esr\": 0.1",
    );
    let d = first(&text);
    assert_eq!(d.path, line_of(&text, "\"esr\""));
    assert!(
        d.hint
            .starts_with("Remove \"esr\", or use one of the fields here:")
            && d.hint.contains("capacitance_f"),
        "{}",
        d.hint
    );
    // A type that does not exist: the closest one.
    let text = VALID.replace("\"resistor\"", "\"resistr\"");
    let d = first(&text);
    assert_eq!(d.path, line_of(&text, "\"resistr\""));
    assert!(
        d.hint
            .starts_with("Write \"resistor\" in place of \"resistr\"."),
        "{}",
        d.hint
    );
    // A field of the simulation, which the parser reads field by field.
    let text = VALID.replace("\"time_step_s\"", "\"time_step\"");
    let d = first(&text);
    assert_eq!(d.path, line_of(&text, "\"time_step\""));
    assert!(d.hint.starts_with("Write \"time_step_s\""), "{}", d.hint);
    // A number without its exponent letter: the number itself, corrected.
    let d = first(&VALID.replace("1e-6", "4.7-9"));
    assert!(d.hint.contains("Write 4.7e-9, not 4.7-9."), "{}", d.hint);
    // A required field left out.
    let d = first(&VALID.replace(", \"resistance_ohm\": 1000.0", ""));
    assert!(d.hint.starts_with("Add \"resistance_ohm\""), "{}", d.hint);
}

#[test]
fn all_errors_are_reported_together() {
    let text = VALID
        .replace("1000.0", "0")
        .replace("\"name\": \"C1\"", "\"name\": \"R1\"")
        .replace("\"time_step_s\": 1e-5", "\"time_step_s\": -1");
    let mut c = codes(&text);
    c.sort_unstable();
    assert_eq!(c, ["E110", "E121", "E130"]);
}

#[test]
fn a_part_of_the_circuit_without_reference_is_an_error() {
    let text = VALID.replace(
        "{\"type\": \"capacitor\"",
        "{\"type\": \"resistor\", \"name\": \"R9\", \"nodes\": [\"x1\", \"x2\"], \"resistance_ohm\": 1.0},\n    {\"type\": \"capacitor\"",
    );
    let errors = load_str(&text).unwrap_err();
    assert_eq!(errors[0].code, "E141");
    assert!(errors[0].message.contains("x1, x2"));

    let no_ground = VALID.replace("gnd", "n0");
    assert_eq!(codes(&no_ground), ["E140"]);
}

#[test]
fn outputs_must_name_existing_nodes_and_components() {
    let text = VALID.replace(
        "\"components\"",
        "\"outputs\": [{\"type\": \"node_voltage\", \"node\": \"n9\"}, {\"type\": \"branch_current\", \"component\": \"L1\"}, {\"type\": \"node_voltage\", \"node\": \"gnd\"}],\n  \"components\"",
    );
    let errors = load_str(&text).unwrap_err();
    let c: Vec<_> = errors.iter().map(|d| d.code).collect();
    assert_eq!(c, ["E150", "E151", "E152"]);
    assert!(errors[1].hint.contains("C1, E1, R1"));
}

#[test]
fn an_end_time_off_the_grid_is_a_warning() {
    let loaded = load_str(&VALID.replace("1e-3", "1.0000055e-3")).unwrap();
    assert_eq!(loaded.warnings.len(), 1);
    assert_eq!(loaded.warnings[0].code, "W112");
    assert_eq!(loaded.warnings[0].severity, Severity::Warning);
}

#[test]
fn the_schema_describes_units_and_components() {
    let schema = openkeito_model::json_schema().to_string();
    for word in ["resistance_ohm", "time_step_s", "2s-dirk", "voltage_source"] {
        assert!(schema.contains(word), "schema lacks {word}");
    }
}

const BUCK: &str = r#"{
  "format": "openkeito-model",
  "version": "0.2",
  "simulation": {"time_step_s": 1e-6, "end_time_s": 1e-3},
  "components": [
    {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 100.0}},
    {"type": "controlled_switch", "name": "S1", "nodes": ["n1", "sw"], "gate": "pwm"},
    {"type": "diode", "name": "D1", "nodes": ["gnd", "sw"]},
    {"type": "inductor", "name": "L1", "nodes": ["sw", "out"], "inductance_h": 1e-3},
    {"type": "resistor", "name": "R1", "nodes": ["out", "gnd"], "resistance_ohm": 10.0}
  ],
  "controls": [
    {"type": "constant", "name": "duty", "value": 0.4},
    {"type": "triangle_wave", "name": "carrier", "frequency_hz": 10000.0},
    {"type": "comparator", "name": "pwm", "inputs": ["duty", "carrier"]}
  ],
  "outputs": [{"type": "signal", "signal": "pwm"}, {"type": "branch_current", "component": "D1"}]
}"#;

fn errors(text: &str) -> Vec<openkeito_model::Diagnostic> {
    load_str(text).expect_err("model must be rejected")
}

#[test]
fn a_power_electronics_model_loads_with_default_resistances() {
    let loaded = load_str(BUCK).unwrap();
    assert!(loaded.warnings.is_empty());
    match &loaded.model.components[2] {
        openkeito_model::Component::Diode(d) => {
            assert_eq!(d.on_resistance_ohm, 1e-3);
            assert_eq!(d.off_resistance_ohm, 1e6);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(loaded.model.controls.len(), 3);
    assert_eq!(loaded.model.simulation.output_interval(), 1);
}

#[test]
fn version_0_1_does_not_have_the_additions_of_0_2() {
    let e = errors(&BUCK.replace("\"0.2\"", "\"0.1\""));
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].code, "E102");
    assert_eq!(e[0].path, "components[1] (controlled_switch)");
    for what in ["components[2] (diode)", "controls", "outputs[0] (signal)"] {
        assert!(e[0].message.contains(what), "{}", e[0].message);
    }
    assert!(e[0].hint.contains("\"version\": \"0.2\""));
    let e = errors(&BUCK.replace("\"0.2\"", "\"0.10\""));
    assert_eq!(e[0].code, "E101");
    assert!(e[0].hint.contains("0.4"));
}

#[test]
fn resistances_of_switching_elements_are_checked() {
    let e = errors(&BUCK.replace(
        "\"nodes\": [\"gnd\", \"sw\"]",
        "\"nodes\": [\"gnd\", \"sw\"], \"on_resistance_ohm\": 0",
    ));
    assert_eq!(e[0].code, "E130");
    assert_eq!(e[0].path, "components[2].on_resistance_ohm");
    let e = errors(&BUCK.replace(
        "\"gate\": \"pwm\"",
        "\"gate\": \"pwm\", \"on_resistance_ohm\": 1.0, \"off_resistance_ohm\": 0.5",
    ));
    assert_eq!(e[0].code, "E132");
    assert_eq!(e[0].path, "components[1].off_resistance_ohm");
}

#[test]
fn signals_must_exist() {
    let e = errors(&BUCK.replace("\"gate\": \"pwm\"", "\"gate\": \"pmw\""));
    assert_eq!(e[0].code, "E170");
    assert_eq!(e[0].path, "components[1].gate");
    assert!(e[0].hint.contains("carrier, duty, pwm"), "{}", e[0].hint);
    let e = errors(&BUCK.replace("\"signal\": \"pwm\"", "\"signal\": \"x\""));
    assert_eq!(e[0].code, "E170");
    assert_eq!(e[0].path, "outputs[0].signal");
    // Without control blocks the hint says where to add one.
    let start = BUCK.find("  \"controls\"").unwrap();
    let end = BUCK.find("  \"outputs\"").unwrap();
    let text = format!("{}{}", &BUCK[..start], &BUCK[end..]);
    let e = errors(&text);
    assert!(e.iter().all(|d| d.code == "E170"));
    assert!(e[0].hint.contains("No control blocks"));
}

#[test]
fn control_blocks_are_checked() {
    // Loop of blocks without a measurement.
    let text = BUCK.replace(
        "{\"type\": \"constant\", \"name\": \"duty\", \"value\": 0.4}",
        "{\"type\": \"gain\", \"name\": \"duty\", \"input\": \"twice\", \"gain\": 0.5}, \
         {\"type\": \"gain\", \"name\": \"twice\", \"input\": \"duty\", \"gain\": 2.0}",
    );
    let e = errors(&text);
    assert_eq!(e[0].code, "E171");
    assert!(e[0].message.contains("duty -> twice"), "{}", e[0].message);
    assert!(e[0].hint.contains("measurement"));
    // Signs of a sum.
    let text = BUCK.replace(
        "{\"type\": \"constant\", \"name\": \"duty\", \"value\": 0.4}",
        "{\"type\": \"constant\", \"name\": \"k\", \"value\": 0.4}, \
         {\"type\": \"sum\", \"name\": \"duty\", \"inputs\": [\"k\", \"k\"], \"signs\": [1.0, 2.0]}",
    );
    let e = errors(&text);
    assert_eq!(
        (e[0].code, e[0].path.as_str()),
        ("E172", "controls[1].signs[1]")
    );
    let text = text.replace("[1.0, 2.0]", "[1.0]");
    assert_eq!(errors(&text)[0].code, "E172");
    // Carrier limits and frequency.
    let text = BUCK.replace(
        "\"frequency_hz\": 10000.0",
        "\"frequency_hz\": 0, \"minimum\": 1.0, \"maximum\": 1.0",
    );
    let mut c: Vec<_> = errors(&text).iter().map(|d| d.code).collect();
    c.sort_unstable();
    assert_eq!(c, ["E130", "E132"]);
    // Measurements name existing nodes and components.
    let text = BUCK.replace(
        "{\"type\": \"constant\", \"name\": \"duty\", \"value\": 0.4}",
        "{\"type\": \"constant\", \"name\": \"duty\", \"value\": 0.4}, \
         {\"type\": \"measurement\", \"name\": \"m1\", \"quantity\": {\"type\": \"node_voltage\", \"node\": \"o\"}}, \
         {\"type\": \"measurement\", \"name\": \"m2\", \"quantity\": {\"type\": \"branch_current\", \"component\": \"L9\"}}",
    );
    let e = errors(&text);
    assert_eq!(
        (e[0].code, e[0].path.as_str()),
        ("E150", "controls[1].quantity.node")
    );
    assert_eq!(
        (e[1].code, e[1].path.as_str()),
        ("E151", "controls[2].quantity.component")
    );
}

#[test]
fn components_and_control_blocks_share_one_namespace() {
    let e = errors(
        &BUCK
            .replace("\"name\": \"duty\"", "\"name\": \"L1\"")
            .replace("[\"duty\",", "[\"L1\","),
    );
    assert_eq!(e[0].code, "E121");
    assert_eq!(e[0].path, "controls[0].name");
    assert!(e[0].message.contains("components[3]"));
}

#[test]
fn output_interval_must_be_at_least_one() {
    let text = BUCK.replace(
        "\"end_time_s\": 1e-3}",
        "\"end_time_s\": 1e-3, \"output_interval_steps\": 0}",
    );
    assert_eq!(errors(&text)[0].code, "E130");
    let text = BUCK.replace(
        "\"end_time_s\": 1e-3}",
        "\"end_time_s\": 1e-3, \"output_interval_steps\": 10}",
    );
    assert_eq!(
        load_str(&text).unwrap().model.simulation.output_interval(),
        10
    );
}

#[test]
fn the_schema_describes_the_power_electronics_additions() {
    let schema = openkeito_model::json_schema().to_string();
    for word in [
        "controls",
        "diode",
        "controlled_switch",
        "triangle_wave",
        "integral_gain_per_s",
        "output_interval_steps",
    ] {
        assert!(schema.contains(word), "schema lacks {word}");
    }
}

const THREE_PHASE: &str = r#"{
  "format": "openkeito-model",
  "version": "0.3",
  "simulation": {"time_step_s": 2e-5, "end_time_s": 0.1},
  "components": [
    {"type": "three_phase_source", "name": "G1", "bus": "src", "line_voltage_rms_v": 66000.0, "frequency_hz": 50.0,
     "impedance": {"type": "sequence", "r1_ohm": 0.5, "l1_h": 0.016, "r0_ohm": 1.0, "l0_h": 0.048}},
    {"type": "pi_line", "name": "LINE1", "buses": ["src", "b2"], "length_km": 50.0,
     "per_km": {"type": "sequence", "r1_ohm_per_km": 0.03, "l1_h_per_km": 0.0009, "c1_f_per_km": 1.25e-8,
                "r0_ohm_per_km": 0.2, "l0_h_per_km": 0.003, "c0_f_per_km": 6.0e-9}},
    {"type": "three_phase_transformer", "name": "T1", "buses": ["b2", "b3"], "vector_group": "YNd11",
     "rated_voltage_kv": [66.0, 6.6], "rated_power_mva": 30.0, "rated_frequency_hz": 50.0,
     "impedance_percent": 10.0, "resistance_percent": 0.5, "no_load_current_percent": 0.5, "no_load_loss_kw": 20.0},
    {"type": "coupled_rl", "name": "LOAD", "buses": ["b3", "gnd"],
     "impedance": {"type": "matrix", "resistance_ohm": [[8, 0, 0], [0, 8, 0], [0, 0, 8]], "inductance_h": [[0.01, 0, 0], [0, 0.01, 0], [0, 0, 0.01]]}},
    {"type": "fault", "name": "F1", "bus": "b2", "phases": ["a"], "ground": true, "start_time_s": 0.05, "clear_time_s": 0.08}
  ],
  "outputs": [{"type": "bus_voltage", "bus": "b2"}, {"type": "branch_current", "component": "F1"}]
}"#;

#[test]
fn a_three_phase_model_loads_with_its_defaults() {
    let loaded = load_str(THREE_PHASE).unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    let m = &loaded.model;
    match &m.components[0] {
        openkeito_model::Component::ThreePhaseSource(s) => {
            assert_eq!(s.neutral, "gnd");
            assert_eq!(s.angle_deg, 0.0);
            assert_eq!(s.bus.nodes(), ["src.a", "src.b", "src.c"]);
        }
        other => panic!("{other:?}"),
    }
    match &m.components[1] {
        openkeito_model::Component::PiLine(l) => assert_eq!(l.sections, 1),
        other => panic!("{other:?}"),
    }
    match &m.components[4] {
        openkeito_model::Component::Fault(f) => {
            assert_eq!(f.fault_resistance_ohm, 0.0);
            assert!(f.clear_at_current_zero);
        }
        other => panic!("{other:?}"),
    }
    // "gnd" as a bus is gnd in every phase.
    assert_eq!(
        openkeito_model::BusRef::Bus("gnd".into()).nodes(),
        ["gnd", "gnd", "gnd"]
    );
}

#[test]
fn version_0_2_does_not_have_the_three_phase_components() {
    let e = errors(&THREE_PHASE.replace("\"0.3\"", "\"0.2\""));
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].code, "E102");
    assert_eq!(e[0].path, "components[0] (three_phase_source)");
    assert!(e[0].message.contains("outputs[0] (bus_voltage)"));
    assert!(e[0].hint.contains("\"version\": \"0.3\""), "{}", e[0].hint);
    assert!(!e[0].hint.contains("Version 0.2"), "{}", e[0].hint);
    // From 0.1, the hint lists what both later versions add.
    let e = errors(&THREE_PHASE.replace("\"0.3\"", "\"0.1\""));
    assert!(e[0].hint.contains("Version 0.2") && e[0].hint.contains("Version 0.3"));
}

fn single_code(text: &str) -> openkeito_model::Diagnostic {
    let e = errors(text);
    assert_eq!(e.len(), 1, "{e:#?}");
    e.into_iter().next().unwrap()
}

#[test]
fn transformer_parameters_are_checked() {
    let d = single_code(&THREE_PHASE.replace("YNd11", "YNd3"));
    assert_eq!(d.code, "E135");
    assert!(d.hint.contains("Dy11"), "{}", d.hint);
    assert_eq!(
        single_code(&THREE_PHASE.replace("YNd11", "ynd11")).code,
        "E135"
    );
    let d = single_code(&THREE_PHASE.replace(
        "\"vector_group\": \"YNd11\"",
        "\"vector_group\": \"YNd11\", \"lv_neutral\": \"gnd\"",
    ));
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E136", "components[2].lv_neutral")
    );
    let d = single_code(&THREE_PHASE.replace("YNd11", "Yy0").replace(
        "\"no_load_current_percent\": 0.5, \"no_load_loss_kw\": 20.0",
        "\"no_load_current_percent\": 0.0",
    ));
    assert_eq!(d.code, "E137");
    let d = single_code(&THREE_PHASE.replace(
        "\"resistance_percent\": 0.5",
        "\"resistance_percent\": 10.0",
    ));
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E132", "components[2].resistance_percent")
    );
    // 300 kW of core loss is 1 percent of 30 MVA, more than i0 = 0.5 percent.
    let d = single_code(
        &THREE_PHASE.replace("\"no_load_loss_kw\": 20.0", "\"no_load_loss_kw\": 300.0"),
    );
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E132", "components[2].no_load_loss_kw")
    );
    let d = single_code(
        &THREE_PHASE.replace("\"impedance_percent\": 10.0", "\"impedance_percent\": 0"),
    );
    assert_eq!(d.code, "E130");
}

#[test]
fn coupled_matrices_must_be_symmetric_and_definite() {
    let d = single_code(&THREE_PHASE.replace(
        "\"inductance_h\": [[0.01, 0, 0], [0, 0.01, 0]",
        "\"inductance_h\": [[0.01, 0.002, 0], [0, 0.01, 0]",
    ));
    assert_eq!(d.code, "E133");
    assert_eq!(d.path, "components[3].impedance.inductance_h[0][1]");
    // Mutual inductance larger than the self inductance.
    let d = single_code(&THREE_PHASE.replace(
        "\"inductance_h\": [[0.01, 0, 0], [0, 0.01, 0]",
        "\"inductance_h\": [[0.01, 0.02, 0], [0.02, 0.01, 0]",
    ));
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E134", "components[3].impedance.inductance_h")
    );
    // A zero resistance matrix is allowed (semidefinite), a negative one is not.
    let zero = THREE_PHASE.replace(
        "[[8, 0, 0], [0, 8, 0], [0, 0, 8]]",
        "[[0, 0, 0], [0, 0, 0], [0, 0, 0]]",
    );
    assert!(load_str(&zero).is_ok());
    let d = single_code(&THREE_PHASE.replace(
        "[[8, 0, 0], [0, 8, 0], [0, 0, 8]]",
        "[[8, 0, 0], [0, -1, 0], [0, 0, 8]]",
    ));
    assert_eq!(d.code, "E134");
    let d = single_code(&THREE_PHASE.replace("\"l0_h_per_km\": 0.003", "\"l0_h_per_km\": 0"));
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E130", "components[1].per_km.l0_h_per_km")
    );
}

#[test]
fn faults_are_checked() {
    let d = single_code(&THREE_PHASE.replace("\"ground\": true, ", ""));
    assert_eq!((d.code, d.path.as_str()), ("E138", "components[4].phases"));
    let d = single_code(&THREE_PHASE.replace("\"phases\": [\"a\"]", "\"phases\": [\"a\", \"a\"]"));
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E138", "components[4].phases[1]")
    );
    let d = single_code(&THREE_PHASE.replace("\"clear_time_s\": 0.08", "\"clear_time_s\": 0.05"));
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E132", "components[4].clear_time_s")
    );
    assert_eq!(
        single_code(&THREE_PHASE.replace("\"phases\": [\"a\"]", "\"phases\": [\"d\"]")).code,
        "E001"
    );
}

#[test]
fn buses_and_outputs_of_three_phase_components_are_checked() {
    let d = single_code(&THREE_PHASE.replace("\"bus\": \"b2\"}", "\"bus\": \"b9\"}"));
    assert_eq!((d.code, d.path.as_str()), ("E154", "outputs[0].bus"));
    assert!(d.hint.contains("b2, b3, src"), "{}", d.hint);
    let d = single_code(&THREE_PHASE.replace(
        "{\"type\": \"branch_current\", \"component\": \"F1\"}",
        "{\"type\": \"branch_voltage\", \"component\": \"T1\"}",
    ));
    assert_eq!(d.code, "E153");
    let d = single_code(&THREE_PHASE.replace(
        "\"buses\": [\"src\", \"b2\"]",
        "\"buses\": [\"src\", \"src\"]",
    ));
    assert_eq!(d.code, "E123");
    // The new low-voltage nodes also float (E141).
    let e = errors(&THREE_PHASE.replace(
        "\"buses\": [\"b2\", \"b3\"]",
        "\"buses\": [\"b2\", [\"x\", \"x\", \"y\"]]",
    ));
    assert_eq!(
        (e[0].code, e[0].path.as_str()),
        ("E123", "components[2].buses[1]")
    );
    let d = single_code(&THREE_PHASE.replace("\"bus\": \"src\"", "\"bus\": \"s rc\""));
    assert_eq!(d.code, "E124");
}

#[test]
fn a_delta_side_without_ground_floats() {
    // With the load connected to its own star point, nothing grounds the
    // delta side of T1.
    let text = THREE_PHASE.replace(
        "\"buses\": [\"b3\", \"gnd\"]",
        "\"buses\": [\"b3\", [\"n\", \"n\", \"n\"]]",
    );
    let d = single_code(&text);
    assert_eq!(d.code, "E141");
    assert!(
        d.message.contains("b3.a") && d.message.contains(", n"),
        "{}",
        d.message
    );
    assert!(d.hint.contains("delta"), "{}", d.hint);
}

#[test]
fn measurements_of_three_phase_components_are_not_supported_yet() {
    let measure = |quantity: &str| {
        THREE_PHASE.replace(
            "\"outputs\":",
            &format!(
                "\"controls\": [{{\"type\": \"measurement\", \"name\": \"m\", \"quantity\": {quantity}}}],\n  \"outputs\":"
            ),
        )
    };
    let d = single_code(&measure(
        "{\"type\": \"branch_current\", \"component\": \"F1\"}",
    ));
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E173", "controls[0].quantity.component")
    );
    assert!(load_str(&measure("{\"type\": \"node_voltage\", \"node\": \"b2.a\"}")).is_ok());
}

#[test]
fn a_terminal_and_phase_that_name_no_quantity_are_found_without_building_the_circuit() {
    let with = |control: &str| {
        THREE_PHASE
            .replace("\"version\": \"0.3\"", "\"version\": \"0.5\"")
            .replace(
                "\"outputs\":",
                &format!(
                    "\"controls\": [{control}],
  \"outputs\":"
                ),
            )
    };
    let measure = |quantity: &str| {
        with(&format!(
            "{{\"type\": \"measurement\", \"name\": \"m\", \"quantity\": {quantity}}}"
        ))
    };
    let three = |quantity: &str| {
        with(&format!(
            "{{\"type\": \"three_phase_measurement\", \"name\": \"m\", \"quantity\": {quantity}}}"
        ))
    };
    let refused = |text: String, column: &str, has: &str| {
        let d = single_code(&text);
        assert_eq!((d.code, d.path.as_str()), ("E173", "controls[0].quantity"));
        assert!(d.message.contains(column), "{}", d.message);
        assert!(d.hint.contains(has), "{}", d.hint);
    };
    // A line has two terminals.
    refused(
        measure("{\"type\": \"branch_current\", \"component\": \"LINE1\", \"phase\": \"a\"}"),
        "I(LINE1.a)",
        "I(LINE1.0.a), I(LINE1.0.b), I(LINE1.0.c), I(LINE1.1.a)",
    );
    refused(
        measure("{\"type\": \"branch_current\", \"component\": \"LINE1\", \"terminal\": 2, \"phase\": \"a\"}"),
        "I(LINE1.2.a)",
        "I(LINE1.1.c)",
    );
    // The fault is on phase a only, with a ground path.
    refused(
        measure("{\"type\": \"branch_current\", \"component\": \"F1\", \"phase\": \"b\"}"),
        "I(F1.b)",
        "I(F1.a), I(F1.g)",
    );
    // A transformer has no branch voltages; a series impedance has no terminals.
    refused(
        measure("{\"type\": \"branch_voltage\", \"component\": \"T1\", \"phase\": \"a\"}"),
        "U(T1.a)",
        "node_voltage",
    );
    refused(
        measure("{\"type\": \"branch_voltage\", \"component\": \"LOAD\", \"phase\": \"d\"}"),
        "U(LOAD.d)",
        "U(LOAD.a), U(LOAD.b), U(LOAD.c)",
    );
    // The three phases at once: one diagnostic for the block.
    refused(
        three("{\"type\": \"branch_current\", \"component\": \"LINE1\"}"),
        "I(LINE1.a)",
        "I(LINE1.0.a)",
    );
    refused(
        three("{\"type\": \"branch_current\", \"component\": \"LOAD\", \"terminal\": 0}"),
        "I(LOAD.0.a)",
        "I(LOAD.a)",
    );
    refused(
        three("{\"type\": \"branch_current\", \"component\": \"F1\"}"),
        "I(F1.b)",
        "I(F1.a), I(F1.g)",
    );
    // Those that name one.
    for ok in [
        measure("{\"type\": \"branch_current\", \"component\": \"LINE1\", \"terminal\": 1, \"phase\": \"c\"}"),
        measure("{\"type\": \"branch_current\", \"component\": \"F1\", \"phase\": \"g\"}"),
        measure("{\"type\": \"branch_voltage\", \"component\": \"LINE1\", \"phase\": \"b\"}"),
        three("{\"type\": \"branch_current\", \"component\": \"T1\", \"terminal\": 0}"),
        three("{\"type\": \"branch_current\", \"component\": \"LOAD\"}"),
    ] {
        assert!(load_str(&ok).is_ok(), "{:#?}", load_str(&ok).err());
    }
}

#[test]
fn the_schema_describes_the_three_phase_additions() {
    let schema = openkeito_model::json_schema().to_string();
    for word in [
        "three_phase_source",
        "coupled_rl",
        "pi_line",
        "three_phase_transformer",
        "vector_group",
        "fault_resistance_ohm",
        "bus_voltage",
        "c0_f_per_km",
    ] {
        assert!(schema.contains(word), "schema lacks {word}");
    }
}

fn with_init(text: &str) -> String {
    text.replace(
        "\"end_time_s\": 0.1}",
        "\"end_time_s\": 0.1, \"initialization\": \"steady_state\"}",
    )
}

#[test]
fn steady_state_initialization_needs_sine_sources_of_one_frequency() {
    let loaded = load_str(&with_init(THREE_PHASE)).unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(
        loaded.model.simulation.initialization,
        openkeito_model::Initialization::SteadyState
    );
    assert_eq!(openkeito_model::source_frequency(&loaded.model), Some(50.0));
    let extra = |component: &str| {
        with_init(THREE_PHASE).replace(
            "  \"components\": [\n",
            &format!("  \"components\": [\n    {component},\n"),
        )
    };
    let d = single_code(&extra(
        "{\"type\": \"current_source\", \"name\": \"J1\", \"nodes\": [\"gnd\", \"b2.a\"], \"waveform\": {\"type\": \"step\", \"time_s\": 0.01, \"after\": 1.0}}",
    ));
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E180", "components[0].waveform")
    );
    let d = single_code(&extra(
        "{\"type\": \"current_source\", \"name\": \"J1\", \"nodes\": [\"gnd\", \"b2.a\"], \"waveform\": {\"type\": \"sine\", \"amplitude\": 1.0, \"frequency_hz\": 60.0}}",
    ));
    assert_eq!(d.code, "E181");
    assert!(d.message.contains("60 Hz"), "{}", d.message);
    // A dc source of 0 (an ammeter) is allowed.
    assert!(load_str(&extra(
        "{\"type\": \"voltage_source\", \"name\": \"A1\", \"nodes\": [\"b2.a\", \"x\"], \"waveform\": {\"type\": \"dc\", \"value\": 0.0}},\n    {\"type\": \"resistor\", \"name\": \"RX\", \"nodes\": [\"x\", \"gnd\"], \"resistance_ohm\": 100.0}",
    ))
    .is_ok());
    let d = single_code(&extra(
        "{\"type\": \"diode\", \"name\": \"D1\", \"nodes\": [\"b2.a\", \"gnd\"]}",
    ));
    assert_eq!((d.code, d.path.as_str()), ("E182", "components[0]"));
    // Controls are not initialized: a warning.
    let text = with_init(THREE_PHASE).replace(
        "\"outputs\":",
        "\"controls\": [{\"type\": \"constant\", \"name\": \"k\", \"value\": 1.0}],\n  \"outputs\":",
    );
    let loaded = load_str(&text).unwrap();
    assert_eq!(loaded.warnings[0].code, "W183");
    // In version 0.2 the setting does not exist.
    let e = errors(&BUCK.replace(
        "\"end_time_s\": 1e-3}",
        "\"end_time_s\": 1e-3, \"initialization\": \"steady_state\"}",
    ));
    assert!(e
        .iter()
        .any(|d| d.code == "E102" && d.message.contains("simulation.initialization")));
}

fn with_scan(scan: &str) -> String {
    THREE_PHASE.replace(
        "  \"outputs\":",
        &format!("  \"frequency_scan\": {scan},\n  \"outputs\":"),
    )
}

#[test]
fn a_frequency_scan_is_checked() {
    let loaded = load_str(&with_scan(
        "{\"start_hz\": 1, \"stop_hz\": 2500, \"points\": 2500, \"nodes\": [\"b2.a\"], \"buses\": [\"b3\"]}",
    ))
    .unwrap();
    let scan = loaded.model.frequency_scan.unwrap();
    let f = scan.frequencies();
    assert_eq!((f.len(), f[0], f[2499]), (2500, 1.0, 2500.0));
    assert!((f[1] - 2.0).abs() < 1e-12);
    let code = |s: &str| single_code(&with_scan(s));
    let d = code("{\"start_hz\": 0, \"stop_hz\": 100, \"points\": 10, \"nodes\": [\"b2.a\"]}");
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E190", "frequency_scan.start_hz")
    );
    let d = code("{\"start_hz\": 10, \"stop_hz\": 1, \"points\": 10, \"nodes\": [\"b2.a\"]}");
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E190", "frequency_scan.stop_hz")
    );
    let d = code("{\"start_hz\": 10, \"stop_hz\": 100, \"nodes\": [\"b2.a\"]}");
    assert_eq!((d.code, d.path.as_str()), ("E190", "frequency_scan.points"));
    let d = code("{\"start_hz\": 10, \"stop_hz\": 100, \"points\": 10, \"frequencies_hz\": [50], \"nodes\": [\"b2.a\"]}");
    assert_eq!(d.code, "E190");
    let d = code("{\"frequencies_hz\": [50, -1], \"nodes\": [\"b2.a\"]}");
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E190", "frequency_scan.frequencies_hz[1]")
    );
    let d = code("{\"frequencies_hz\": [50]}");
    assert_eq!(d.code, "E191");
    let d = code("{\"frequencies_hz\": [50], \"nodes\": [\"b9.a\"]}");
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E150", "frequency_scan.nodes[0]")
    );
    let d = code("{\"frequencies_hz\": [50], \"buses\": [\"b9\"]}");
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E154", "frequency_scan.buses[0]")
    );
}

#[test]
fn loads_and_current_sources_are_checked() {
    let with = |component: &str, version: &str| {
        THREE_PHASE
            .replace("\"0.3\"", &format!("\"{version}\""))
            .replace(
                "  \"components\": [\n",
                &format!("  \"components\": [\n    {component},\n"),
            )
    };
    let load = "{\"type\": \"three_phase_load\", \"name\": \"LD2\", \"bus\": \"b2\", \"rated_voltage_kv\": 66.0, \"rated_frequency_hz\": 50.0, \"active_power_mw\": 10.0, \"reactive_power_mvar\": -2.0}";
    assert!(load_str(&with(load, "0.4")).is_ok());
    let d = single_code(&with(load, "0.3"));
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E102", "components[0] (three_phase_load)")
    );
    assert!(d.hint.contains("\"version\": \"0.4\""), "{}", d.hint);
    let d = single_code(&with(&load.replace("10.0", "-10.0"), "0.4"));
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E130", "components[0].active_power_mw")
    );
    let d = single_code(&with(
        &load.replace("10.0", "0.0").replace("-2.0", "0.0"),
        "0.4",
    ));
    assert_eq!(d.code, "E130");
    let source = "{\"type\": \"three_phase_current_source\", \"name\": \"PV\", \"bus\": \"b2\", \"current_rms_a\": 50.0, \"frequency_hz\": 50.0}";
    assert!(load_str(&with(source, "0.4")).is_ok());
    let d = single_code(&with(&source.replace("50.0,", "-1.0,"), "0.4"));
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E130", "components[0].current_rms_a")
    );
}

#[test]
fn ideal_transformers_are_checked() {
    let with = |component: &str, version: &str| {
        THREE_PHASE
            .replace("\"0.3\"", &format!("\"{version}\""))
            .replace(
                "  \"components\": [
",
                &format!(
                    "  \"components\": [
    {component},
"
                ),
            )
    };
    let t = "{\"type\": \"three_phase_ideal_transformer\", \"name\": \"PST\", \"buses\": [\"b2\", \"b9\"], \"rated_voltage_kv\": [66.0, 66.0], \"phase_shift_deg\": 3.5}";
    let load = "{\"type\": \"three_phase_load\", \"name\": \"LD9\", \"bus\": \"b9\", \"rated_voltage_kv\": 66.0, \"rated_frequency_hz\": 50.0, \"active_power_mw\": 10.0, \"reactive_power_mvar\": 2.0}";
    let both = format!(
        "{t},
    {load}"
    );
    assert!(load_str(&with(&both, "0.4")).is_ok());
    let d = single_code(&with(&both, "0.3"));
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E102", "components[0] (three_phase_ideal_transformer)")
    );
    let d = single_code(&with(&both.replace("[66.0, 66.0]", "[66.0, 0.0]"), "0.4"));
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E130", "components[0].rated_voltage_kv[1]")
    );
    let d = single_code(&with(
        &both.replace("[\"b2\", \"b9\"]", "[\"b2\", \"b2\"]"),
        "0.4",
    ));
    assert_eq!(d.path, "components[0].buses[1]");
}

#[test]
fn a_fault_without_phases_is_an_error_and_not_a_panic() {
    let d = single_code(&THREE_PHASE.replace(
        "\"phases\": [\"a\"], \"ground\": true",
        "\"phases\": [], \"ground\": true",
    ));
    assert_eq!((d.code, d.path.as_str()), ("E138", "components[4].phases"));
}

#[test]
fn the_faulted_phases_are_different_nodes_and_not_gnd() {
    let on = |bus: &str| THREE_PHASE.replace("\"bus\": \"b2\", \"phases\": [\"a\"]", bus);
    let d = single_code(&on(
        "\"bus\": [\"gnd\", \"b2.b\", \"b2.c\"], \"phases\": [\"a\", \"b\"]",
    ));
    assert_eq!((d.code, d.path.as_str()), ("E123", "components[4].bus"));
    let d = single_code(&on(
        "\"bus\": [\"b2.a\", \"b2.a\", \"b2.c\"], \"phases\": [\"a\", \"b\"]",
    ));
    assert_eq!((d.code, d.path.as_str()), ("E123", "components[4].bus"));
    // A phase that is not faulted may be any node.
    load_str(&on(
        "\"bus\": [\"b2.a\", \"gnd\", \"gnd\"], \"phases\": [\"a\"]",
    ))
    .unwrap();
}

#[test]
fn a_pi_line_reaches_gnd_through_its_capacitances() {
    // No component names gnd: the neutral of the source is a node of its own.
    let text = r#"{
      "format": "openkeito-model", "version": "0.3",
      "simulation": {"time_step_s": 2e-5, "end_time_s": 0.02},
      "components": [
        {"type": "three_phase_source", "name": "G1", "bus": "b1", "neutral": "n", "line_voltage_rms_v": 66000.0, "frequency_hz": 50.0},
        {"type": "pi_line", "name": "LINE1", "buses": ["b1", "b2"], "length_km": 50.0,
         "per_km": {"type": "sequence", "r1_ohm_per_km": 0.03, "l1_h_per_km": 0.0009, "c1_f_per_km": 1.25e-8,
                    "r0_ohm_per_km": 0.2, "l0_h_per_km": 0.003, "c0_f_per_km": 6.0e-9}}
      ]
    }"#;
    load_str(text).unwrap();
    // Without the line, nothing is connected to gnd.
    let alone = text.replace("\"pi_line\"", "\"coupled_rl\"").replace(
        "\"length_km\": 50.0,\n         \"per_km\": {\"type\": \"sequence\", \"r1_ohm_per_km\": 0.03, \"l1_h_per_km\": 0.0009, \"c1_f_per_km\": 1.25e-8,\n                    \"r0_ohm_per_km\": 0.2, \"l0_h_per_km\": 0.003, \"c0_f_per_km\": 6.0e-9}",
        "\"impedance\": {\"type\": \"sequence\", \"r1_ohm\": 1.0, \"l1_h\": 0.01, \"r0_ohm\": 2.0, \"l0_h\": 0.03}",
    );
    assert_eq!(codes(&alone), ["E140"]);
}

#[test]
fn names_that_give_two_columns_of_one_name_are_errors() {
    // I(G1.a) would be the current of phase a of G1 and of this resistor.
    let d = single_code(&THREE_PHASE.replace(
        "{\"type\": \"fault\"",
        "{\"type\": \"resistor\", \"name\": \"G1.a\", \"nodes\": [\"b2.a\", \"gnd\"], \"resistance_ohm\": 1e6},\n    {\"type\": \"fault\"",
    ));
    assert_eq!((d.code, d.path.as_str()), ("E125", "components[4].name"));
    assert!(d.hint.contains("\"G1.\""), "{}", d.hint);
    // A dot after the name of a two-terminal component is free.
    load_str(&VALID.replace("\"C1\"", "\"R1.c\"")).unwrap();

    let d = single_code(&THREE_PHASE.replace(
        "\"outputs\": [",
        "\"outputs\": [{\"type\": \"bus_voltage\", \"bus\": \"b2\"}, ",
    ));
    assert_eq!((d.code, d.path.as_str()), ("E155", "outputs[1]"));

    let scan = THREE_PHASE.replace(
        "\"outputs\": [",
        "\"frequency_scan\": {\"frequencies_hz\": [50.0], \"nodes\": [\"b2.a\", \"b2.a\"], \"buses\": [\"b2\", \"b3\", \"b2\"]},\n  \"outputs\": [",
    );
    let found: Vec<(&str, String)> = errors(&scan)
        .into_iter()
        .map(|d| (d.code, d.path))
        .collect();
    assert_eq!(
        found,
        [
            ("E194", "frequency_scan.nodes[1]".to_string()),
            ("E194", "frequency_scan.buses[2]".to_string())
        ]
    );
}

#[test]
fn the_number_of_steps_is_rounded_and_bounded() {
    let sim = |step: &str, end: &str| {
        VALID
            .replace("\"time_step_s\": 1e-5", &format!("\"time_step_s\": {step}"))
            .replace("\"end_time_s\": 1e-3", &format!("\"end_time_s\": {end}"))
    };
    // 60 / 2.5e-6 is 23999999.999999996 in floating point.
    let loaded = load_str(&sim("2.5e-6", "60")).unwrap();
    assert_eq!(
        openkeito_model::step_count(&loaded.model.simulation),
        24_000_000
    );
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    // Not a multiple: the last whole step, and a warning.
    let loaded = load_str(&sim("3e-5", "1e-3")).unwrap();
    assert_eq!(openkeito_model::step_count(&loaded.model.simulation), 33);
    assert_eq!(loaded.warnings[0].code, "W112");

    let d = single_code(&sim("1e-300", "1"));
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E113", "simulation.time_step_s")
    );
    // Many rows are a warning that names the remedy; thinned out, they are not.
    let loaded = load_str(&sim("1e-9", "1")).unwrap();
    assert_eq!(loaded.warnings[0].code, "W114");
    assert!(loaded.warnings[0].hint.contains("output_interval_steps"));
    let thinned = sim("1e-9", "1")
        .replace("\"version\": \"0.1\"", "\"version\": \"0.2\"")
        .replace(
            "\"end_time_s\": 1",
            "\"end_time_s\": 1, \"output_interval_steps\": 1000",
        );
    assert!(load_str(&thinned).unwrap().warnings.is_empty());
}

#[test]
fn sequence_values_too_far_apart_to_invert_are_errors() {
    // "No zero-sequence path" written as an enormous resistance.
    let d = single_code(&THREE_PHASE.replace("\"r0_ohm\": 1.0", "\"r0_ohm\": 1e30"));
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E139", "components[0].impedance")
    );
    assert!(d.hint.contains("1e6"), "{}", d.hint);
    let d = single_code(&THREE_PHASE.replace("\"r0_ohm_per_km\": 0.2", "\"r0_ohm_per_km\": 1e30"));
    assert_eq!((d.code, d.path.as_str()), ("E139", "components[1].per_km"));
    // Six orders of magnitude are fine.
    load_str(&THREE_PHASE.replace("\"r0_ohm\": 1.0", "\"r0_ohm\": 1e6")).unwrap();
    // Every stage coefficient of the run counts, not the smallest alone.
    // Next to no inductance in the positive sequence and 500 H in the zero
    // sequence: at 2 / time_step_s = 1e5 / s the ratio of R + alpha L
    // between the sequences is 1e8, at 1e6 / time_step_s = 5e10 / s, the
    // coefficient at t = 0+, it is 4.5e13 (a smallest pivot of 4.5 times
    // its reciprocal, 1e-13). The validation used to look at the first
    // only, and the run stopped with E204.
    let inductive = THREE_PHASE
        .replace("\"l1_h\": 0.016", "\"l1_h\": 1e-12")
        .replace("\"l0_h\": 0.048", "\"l0_h\": 500.0");
    let d = single_code(&inductive);
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E139", "components[0].impedance")
    );
    assert!(d.message.contains("alpha = 5.000e10 / s"), "{}", d.message);
    assert!(
        d.hint.contains("the inductances at the largest"),
        "{}",
        d.hint
    );
    // With switching instants within the steps, the parts of a split step
    // have larger coefficients still (up to 3.41e6 / time_step_s): 30 H
    // gives a smallest pivot of 1.65e-12 at 5e10 / s and of 5.9e-13 at
    // 1.707e11 / s, on either side of the limit of 1e-12.
    let split = THREE_PHASE
        .replace("\"version\": \"0.3\"", "\"version\": \"0.5\"")
        .replace(
            "\"end_time_s\": 0.1",
            "\"end_time_s\": 0.1, \"switching\": \"interpolated\"",
        )
        .replace("\"l1_h\": 0.016", "\"l1_h\": 1e-12")
        .replace("\"l0_h\": 0.048", "\"l0_h\": 30.0");
    let d = single_code(&split);
    assert_eq!(d.code, "E139");
    assert!(d.message.contains("alpha = 1.707e11 / s"), "{}", d.message);
    load_str(&split.replace(", \"switching\": \"interpolated\"", "")).unwrap();
}

#[test]
fn a_scan_of_a_model_with_diodes_warns_that_they_are_taken_as_off() {
    let model = |scan: &str| {
        format!(
            r#"{{"format": "openkeito-model", "version": "0.3",
                "simulation": {{"time_step_s": 1e-5, "end_time_s": 1e-3}},
                "components": [
                  {{"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {{"type": "dc", "value": 1.0}}}},
                  {{"type": "diode", "name": "D1", "nodes": ["n1", "n2"]}},
                  {{"type": "resistor", "name": "R1", "nodes": ["n2", "gnd"], "resistance_ohm": 1.0}}
                ]{scan}}}"#
        )
    };
    let scan = r#", "frequency_scan": {"frequencies_hz": [50.0], "nodes": ["n2"]}"#;
    let loaded = load_str(&model(scan)).unwrap();
    let codes: Vec<&str> = loaded.warnings.iter().map(|d| d.code).collect();
    assert_eq!(codes, ["W192"]);
    assert_eq!(loaded.warnings[0].path, "components");
    assert!(loaded.warnings[0].message.ends_with(": D1"));
    // The warning is about the scan: a model without one has none.
    assert!(load_str(&model("")).unwrap().warnings.is_empty());
}

#[test]
fn hints_have_no_runs_of_spaces() {
    let d = single_code(&THREE_PHASE.replace(
        "{\"type\": \"branch_current\", \"component\": \"F1\"}",
        "{\"type\": \"branch_voltage\", \"component\": \"T1\"}",
    ));
    assert_eq!(d.code, "E153");
    assert!(!d.hint.contains("  "), "{}", d.hint);
}

#[test]
fn parse_errors_name_the_kind_of_document_and_its_schema() {
    let batch = openkeito_model::load_batch_str(r#"{"format": "openkeito-batch", "extra": 1}"#)
        .unwrap_err();
    assert!(batch[0]
        .message
        .starts_with("the batch file cannot be read"));
    assert!(batch[0].hint.contains("openkeito schema --batch"));
    let schedule =
        openkeito_model::load_schedule_str(r#"{"format": "openkeito-schedule", "extra": 1}"#)
            .unwrap_err();
    assert!(schedule[0]
        .message
        .starts_with("the schedule cannot be read"));
    assert!(schedule[0].hint.contains("openkeito schema --schedule"));
}

#[test]
fn case_names_are_directory_names_on_every_system() {
    let check = |names: &[&str]| -> Vec<String> {
        openkeito_model::check_case_names(names.iter().copied())
            .into_iter()
            .map(|d| {
                assert_eq!(d.code, "E402");
                d.path
            })
            .collect()
    };
    assert!(check(&["a", "b.1", "t000.fault_b4_abc", "con1", "com10"]).is_empty());
    // Upper and lower case do not tell directories apart everywhere.
    assert_eq!(
        check(&["Case", "case", "x", "CASE"]),
        ["cases[1].name", "cases[3].name"]
    );
    // Outside the output directory, or not a directory at all.
    for bad in [
        "",
        ".",
        "..",
        "a/b",
        r"a\b",
        "C:x",
        "a,b",
        "a.",
        "NUL",
        "nul.txt",
        "Com1",
        "lpt9",
        "batch-summary.json",
        "Batch-Metrics.csv",
    ] {
        assert_eq!(check(&[bad]), ["cases[0].name"], "{bad:?}");
    }
}

#[test]
fn bus_voltage_and_node_voltage_of_the_same_phase_collide_with_e155() {
    let text = THREE_PHASE.replace(
        "{\"type\": \"branch_current\", \"component\": \"F1\"}",
        "{\"type\": \"node_voltage\", \"node\": \"b2.a\"}",
    );
    let d = single_code(&text);
    assert_eq!((d.code, d.path.as_str()), ("E155", "outputs[1]"));
    assert!(d.message.contains("V(b2.a)"), "{}", d.message);
    assert!(d.message.contains("outputs[0]"), "{}", d.message);

    let reversed = THREE_PHASE.replace(
        "\"outputs\": [{\"type\": \"bus_voltage\", \"bus\": \"b2\"}, {\"type\": \"branch_current\", \"component\": \"F1\"}]",
        "\"outputs\": [{\"type\": \"node_voltage\", \"node\": \"b2.a\"}, {\"type\": \"bus_voltage\", \"bus\": \"b2\"}]",
    );
    let d = single_code(&reversed);
    assert_eq!((d.code, d.path.as_str()), ("E155", "outputs[1]"));
    assert!(d.message.contains("V(b2.a)"), "{}", d.message);
    assert!(d.message.contains("outputs[0]"), "{}", d.message);
}

#[test]
fn inline_batch_models_preserve_raw_json_for_duplicate_keys_and_sha256() {
    let text = r#"{
      "format": "openkeito-batch",
      "version": "0.1",
      "cases": [
        {
          "name": "dup",
          "model": {
            "format": "openkeito-model",
            "version": "0.1",
            "simulation": {"time_step_s": 1e-5, "end_time_s": 1e-3, "end_time_s": 2e-3},
            "components": [
              {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
              {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0}
            ]
          }
        }
      ]
    }"#;
    let batch = openkeito_model::load_batch_str(text).unwrap();
    let openkeito_model::ModelRef::Inline(raw) = &batch.cases[0].model else {
        panic!("expected inline model");
    };
    let errs = load_str(raw.get()).unwrap_err();
    assert_eq!(errs[0].code, "E001");
    assert!(
        errs[0].message.contains("duplicate field"),
        "{}",
        errs[0].message
    );
    assert!(errs[0].hint.contains("end_time_s"), "{}", errs[0].hint);
}

#[test]
fn validation_diagnostics_have_no_runs_of_spaces() {
    // E301 (threads: 0) and E290 (converter prefix collision) used to have spaces from missing '\'.
    let threads_zero = VALID
        .replace("\"version\": \"0.1\"", "\"version\": \"0.5\"")
        .replace(
            "\"end_time_s\": 1e-3}",
            "\"end_time_s\": 1e-3, \"threads\": 0}",
        );
    let conv_prefix = r#"{
      "format": "openkeito-model", "version": "0.5",
      "simulation": {"time_step_s": 5e-5, "end_time_s": 1e-3},
      "components": [
        {"type": "three_phase_source", "name": "G1", "bus": "b1", "line_voltage_rms_v": 1000.0, "frequency_hz": 50.0,
         "impedance": {"type": "sequence", "r1_ohm": 0.1, "l1_h": 1e-3, "r0_ohm": 0.2, "l0_h": 2e-3}},
        {"type": "grid_following_converter", "name": "VSC1", "bus": "b1", "rated_power_mva": 1.0,
         "rated_voltage_kv": 1.0, "rated_frequency_hz": 50.0,
         "reactor_resistance_pu": 0.01, "reactor_reactance_pu": 0.1,
         "reference": {"type": "current", "d_pu": 0.5}}
      ],
      "controls": [
        {"type": "constant", "name": "VSC1.extra", "value": 1.0}
      ]
    }"#;
    for case in [&threads_zero, conv_prefix] {
        let diags = load_str(case).unwrap_err();
        assert!(!diags.is_empty());
        for d in diags {
            assert!(!d.message.contains("  "), "{}: {}", d.code, d.message);
            assert!(!d.hint.contains("  "), "{}: {}", d.code, d.hint);
        }
    }
}

#[test]
fn parse_errors_give_actionable_hints_for_untagged_enums_and_common_mistakes() {
    let first = |text: &str| load_str(text).unwrap_err().remove(0);

    // BusRef (untagged enum)
    let d = first(&THREE_PHASE.replace("\"buses\": [\"src\", \"b2\"]", "\"buses\": [5, \"b2\"]"));
    assert_eq!(d.code, "E001");
    assert!(!d.message.contains("BusRef"), "{}", d.message);
    assert!(d.hint.contains("bus name"), "{}", d.hint);

    // StartValue (untagged enum)
    let d = first(&BUCK.replace(
        "\"controls\": [",
        "\"version\": \"0.5\", \"controls\": [{\"type\": \"integrator\", \"name\": \"ix\", \"input\": \"duty\", \"initial_value\": true}, ",
    ).replace("\"version\": \"0.2\",", ""));
    assert_eq!(d.code, "E001");
    assert!(!d.message.contains("StartValue"), "{}", d.message);
    assert!(d.hint.contains("JSON number"), "{}", d.hint);

    // Setpoint (untagged enum)
    let conv_bad = r#"{
      "format": "openkeito-model", "version": "0.5",
      "simulation": {"time_step_s": 5e-5, "end_time_s": 1e-3},
      "components": [
        {"type": "grid_following_converter", "name": "VSC1", "bus": "b1", "rated_power_mva": 1.0,
         "rated_voltage_kv": 1.0, "rated_frequency_hz": 50.0,
         "reactor_resistance_pu": 0.01, "reactor_reactance_pu": 0.1,
         "reference": {"type": "current", "d_pu": true}}
      ]
    }"#;
    let d = first(conv_bad);
    assert_eq!(d.code, "E001");
    assert!(!d.message.contains("Setpoint"), "{}", d.message);
    assert!(d.hint.contains("JSON number"), "{}", d.hint);

    // Quoted number
    let d = first(&VALID.replace("1000.0", "\"1000.0\""));
    assert_eq!(d.code, "E001");
    assert!(d.hint.contains("without quotes"), "{}", d.hint);

    // Wrong sequence length
    let d = first(&VALID.replace("[\"n1\", \"n2\"]", "[\"n1\", \"n2\", \"n3\"]"));
    assert_eq!(d.code, "E001");
    assert!(d.hint.contains("2 elements"), "{}", d.hint);

    // Number out of range
    let d = first(&VALID.replace("1000.0", "1e999"));
    assert_eq!(d.code, "E001");
    assert!(d.hint.contains("IEEE 754"), "{}", d.hint);

    // Negative integer for unsigned field
    let d = first(&THREE_PHASE.replace(
        "\"length_km\": 50.0",
        "\"length_km\": 50.0, \"sections\": -1",
    ));
    assert_eq!(d.code, "E001");
    assert!(d.hint.contains(">= 0"), "{}", d.hint);

    // Duplicate field
    let d = first(&VALID.replace(
        "\"end_time_s\": 1e-3",
        "\"end_time_s\": 1e-3, \"end_time_s\": 2e-3",
    ));
    assert_eq!(d.code, "E001");
    assert!(d.hint.contains("duplicate \"end_time_s\""), "{}", d.hint);

    // Trailing comma
    let d = first(&VALID.replace("\"capacitance_f\": 1e-6}", "\"capacitance_f\": 1e-6,}"));
    assert_eq!(d.code, "E001");
    assert!(d.hint.contains("trailing comma"), "{}", d.hint);

    // Empty input
    let d = first("   ");
    assert_eq!(d.code, "E001");
    assert!(d.hint.contains("empty"), "{}", d.hint);
}

#[test]
fn non_finite_element_coefficients_are_rejected_with_e130_while_large_values_stay_valid() {
    // Subnormal inductance (1e-320 H) -> 1/(alpha*L) overflows to inf.
    let small_l = VALID.replace(
        "{\"type\": \"capacitor\", \"name\": \"C1\", \"nodes\": [\"n2\", \"gnd\"], \"capacitance_f\": 1e-6}",
        "{\"type\": \"inductor\", \"name\": \"L1\", \"nodes\": [\"n2\", \"gnd\"], \"inductance_h\": 1e-320}",
    );
    let d = single_code(&small_l);
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E130", "components[2].inductance_h")
    );

    // Huge capacitance (1e300 F) -> alpha*C overflows to inf.
    let huge_c = VALID.replace("1e-6", "1e300");
    let d = single_code(&huge_c);
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E130", "components[2].capacitance_f")
    );

    // Subnormal resistance (1e-320 ohm) -> 1/R overflows to inf.
    let small_r = VALID.replace("1000.0", "1e-320");
    let d = single_code(&small_r);
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E130", "components[1].resistance_ohm")
    );

    // Subnormal lag time constant (1e-320 s) -> h/(2*tau) overflows to inf.
    let tiny_lag = BUCK
        .replace("\"version\": \"0.2\"", "\"version\": \"0.5\"")
        .replace(
            "\"controls\": [",
            "\"controls\": [{\"type\": \"lag\", \"name\": \"f1\", \"input\": \"duty\", \"time_constant_s\": 1e-320}, ",
        );
    let d = single_code(&tiny_lag);
    assert_eq!(
        (d.code, d.path.as_str()),
        ("E130", "controls[0].time_constant_s")
    );

    // Normal large values like L = 1000 H and R = 1e14 ohm remain valid.
    let large_l = VALID.replace(
        "{\"type\": \"capacitor\", \"name\": \"C1\", \"nodes\": [\"n2\", \"gnd\"], \"capacitance_f\": 1e-6}",
        "{\"type\": \"inductor\", \"name\": \"L1\", \"nodes\": [\"n2\", \"gnd\"], \"inductance_h\": 1000.0}",
    );
    assert!(load_str(&large_l).is_ok());
    let large_r = VALID.replace("1000.0", "1e14");
    assert!(load_str(&large_r).is_ok());
}

#[test]
fn all_reading_errors_and_duplicate_keys_are_reported_together() {
    let broken = r#"{
  "format": "openkeito-model",
  "version": "0.6",
  "simulation": {"time_step_s": 1e-5, "end_time_s": 1e-3, "end_time_s": 2e-3, "extra_sim": 1},
  "components": [
    {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
    {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 100.0, "resistance_ohm": 200.0, "bogus": 3},
    {"type": "capacitor", "name": "C1", "nodes": ["n2", "gnd"]}
  ],
  "controls": [
    {"type": "gain", "name": "K1", "input": "x"}
  ],
  "outputs": [
    {"type": "node_voltage", "node": "n2", "bad_out": true}
  ],
  "frequency_scan": {"start_hz": 1.0, "stop_hz": 10.0, "points": 2, "nodes": ["n2"], "extra_scan": 9}
}"#;
    let errors = load_str(broken).unwrap_err();
    assert!(
        errors.iter().all(|d| d.code == "E001"),
        "expected only E001, got {errors:#?}"
    );
    // 2 duplicate keys + 6 schema errors across simulation, components[1], components[2], controls[0], outputs[0], frequency_scan
    assert_eq!(errors.len(), 8, "{errors:#?}");
    for e in &errors {
        assert!(e.path.starts_with("line "), "bad line/col path: {}", e.path);
        assert!(e.json_path.is_some(), "missing json_path: {e:#?}");
    }
    let jpaths: Vec<&str> = errors
        .iter()
        .filter_map(|e| e.json_path.as_deref())
        .collect();
    for expected in [
        "simulation.end_time_s",
        "simulation.extra_sim",
        "components[1].resistance_ohm",
        "components[1].bogus",
        "components[2]",
        "controls[0]",
        "outputs[0].bad_out",
        "frequency_scan.extra_scan",
    ] {
        assert!(
            jpaths.iter().any(|p| p.contains(expected)),
            "missing {expected} in {jpaths:?}"
        );
    }
}

#[test]
fn version_0_6_fields_are_gated_by_version_while_older_versions_load() {
    for v in ["0.1", "0.2", "0.3", "0.4", "0.5"] {
        let model = VALID.replace("\"0.1\"", &format!("\"{v}\""));
        assert!(load_str(&model).is_ok(), "version {v} failed");
    }

    let ynyn6_on_05 = r#"{
  "format": "openkeito-model",
  "version": "0.5",
  "simulation": {"time_step_s": 1e-4, "end_time_s": 1e-3},
  "components": [
    {"type": "three_phase_source", "name": "G1", "bus": "p", "line_voltage_rms_v": 1000.0, "frequency_hz": 50.0},
    {"type": "three_phase_ideal_transformer", "name": "T1", "buses": ["p", "s"], "rated_voltage_kv": [1.0, 1.0], "phase_shift_deg": 180.0, "zero_sequence_inverted": true}
  ]
}"#;
    assert_eq!(codes(ynyn6_on_05), ["E102"]);
    assert!(load_str(&ynyn6_on_05.replace("\"0.5\"", "\"0.6\"")).is_ok());
}

#[test]
fn piecewise_linear_table_is_validated_and_version_gated() {
    let base = r#"{
  "format": "openkeito-model",
  "version": "0.7",
  "simulation": {"time_step_s": 1e-4, "end_time_s": 1e-3},
  "components": [
    {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
    {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 1.0}
  ],
  "controls": [
    {"type": "constant", "name": "x_in", "value": 0.5},
    {"type": "piecewise_linear", "name": "y_out", "input": "x_in", "x": [0.0, 1.0], "y": [10.0, 20.0]}
  ]
}"#;
    assert!(load_str(base).is_ok());
    // Single-point table is valid.
    assert!(load_str(
        &base
            .replace("[0.0, 1.0]", "[0.0]")
            .replace("[10.0, 20.0]", "[10.0]")
    )
    .is_ok());
    // Version 0.6 rejects piecewise_linear with E102.
    assert_eq!(codes(&base.replace("\"0.7\"", "\"0.6\"")), ["E102"]);

    // Empty x/y, length mismatch, non-finite, equal/decreasing x -> E242.
    assert_eq!(
        codes(
            &base
                .replace("[0.0, 1.0]", "[]")
                .replace("[10.0, 20.0]", "[]")
        ),
        ["E242"]
    );
    assert_eq!(codes(&base.replace("[10.0, 20.0]", "[10.0]")), ["E242"]);
    assert_eq!(codes(&base.replace("[0.0, 1.0]", "[0.0, 0.0]")), ["E242"]);
    assert_eq!(codes(&base.replace("[0.0, 1.0]", "[1.0, 0.0]")), ["E242"]);

    // NaN / Inf in x or y -> E242 via validate().
    let mut m = load_str(base).unwrap().model;
    if let openkeito_model::Control::PiecewiseLinear(ref mut pwl) = m.controls[1] {
        pwl.x[1] = f64::NAN;
    }
    let d = openkeito_model::validate(&m);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].code, "E242");

    let mut m = load_str(base).unwrap().model;
    if let openkeito_model::Control::PiecewiseLinear(ref mut pwl) = m.controls[1] {
        pwl.y[0] = f64::INFINITY;
    }
    let d = openkeito_model::validate(&m);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].code, "E242");
}

#[test]
fn signal_limits_on_lag_pi_and_limiter_are_validated_and_version_gated() {
    let wrap_controls = |controls: &str, version: &str| {
        format!(
            r#"{{
  "format": "openkeito-model",
  "version": "{version}",
  "simulation": {{"time_step_s": 1e-4, "end_time_s": 1e-3}},
  "components": [
    {{"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {{"type": "dc", "value": 1.0}}}},
    {{"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 1.0}}
  ],
  "controls": [{controls}]
}}"#
        )
    };

    let valid_all = r#"
    {"type": "constant", "name": "u", "value": 0.5},
    {"type": "constant", "name": "lo", "value": -1.0},
    {"type": "constant", "name": "hi", "value": 1.0},
    {"type": "lag", "name": "y_lag", "input": "u", "time_constant_s": 0.01, "minimum": "lo", "maximum": "hi"},
    {"type": "pi", "name": "y_pi", "input": "u", "proportional_gain": 1.0, "integral_gain_per_s": 10.0, "output_min": "lo", "output_max": "hi"},
    {"type": "limiter", "name": "y_lim", "input": "u", "minimum": "lo", "maximum": "hi"}
    "#;
    assert!(load_str(&wrap_controls(valid_all, "0.7")).is_ok());

    // Version 0.6 rejects signal limits on each of lag, pi and limiter with E102.
    for block in [
        r#"{"type": "constant", "name": "u", "value": 0.5}, {"type": "lag", "name": "y", "input": "u", "time_constant_s": 0.01, "minimum": "u"}"#,
        r#"{"type": "constant", "name": "u", "value": 0.5}, {"type": "pi", "name": "y", "input": "u", "proportional_gain": 1.0, "integral_gain_per_s": 1.0, "output_max": "u"}"#,
        r#"{"type": "constant", "name": "u", "value": 0.5}, {"type": "limiter", "name": "y", "input": "u", "minimum": "u"}"#,
    ] {
        assert_eq!(codes(&wrap_controls(block, "0.6")), ["E102"]);
        assert!(load_str(&wrap_controls(block, "0.7")).is_ok());
    }

    // Missing limit signal -> E170.
    for block in [
        r#"{"type": "constant", "name": "u", "value": 0.5}, {"type": "lag", "name": "y", "input": "u", "time_constant_s": 0.01, "minimum": "missing"}"#,
        r#"{"type": "constant", "name": "u", "value": 0.5}, {"type": "pi", "name": "y", "input": "u", "proportional_gain": 1.0, "integral_gain_per_s": 1.0, "output_max": "missing"}"#,
        r#"{"type": "constant", "name": "u", "value": 0.5}, {"type": "limiter", "name": "y", "input": "u", "maximum": "missing"}"#,
    ] {
        assert_eq!(codes(&wrap_controls(block, "0.7")), ["E170"]);
    }

    // Inverted constant bounds -> E132.
    for block in [
        r#"{"type": "constant", "name": "u", "value": 0.5}, {"type": "lag", "name": "y", "input": "u", "time_constant_s": 0.01, "minimum": 1.0, "maximum": 0.0}"#,
        r#"{"type": "constant", "name": "u", "value": 0.5}, {"type": "pi", "name": "y", "input": "u", "proportional_gain": 1.0, "integral_gain_per_s": 1.0, "output_min": 2.0, "output_max": 2.0}"#,
        r#"{"type": "constant", "name": "u", "value": 0.5}, {"type": "limiter", "name": "y", "input": "u", "minimum": 1.0, "maximum": -1.0}"#,
    ] {
        assert_eq!(codes(&wrap_controls(block, "0.7")), ["E132"]);
    }

    // Direct loop through Lag.minimum, Pi.output_max, or Limiter.maximum -> E171.
    for block in [
        r#"{"type": "constant", "name": "u", "value": 0.5}, {"type": "lag", "name": "y", "input": "u", "time_constant_s": 0.01, "minimum": "g"}, {"type": "gain", "name": "g", "input": "y", "gain": 1.0}"#,
        r#"{"type": "constant", "name": "u", "value": 0.5}, {"type": "pi", "name": "y", "input": "u", "proportional_gain": 1.0, "integral_gain_per_s": 1.0, "output_max": "g"}, {"type": "gain", "name": "g", "input": "y", "gain": 1.0}"#,
        r#"{"type": "constant", "name": "u", "value": 0.5}, {"type": "limiter", "name": "y", "input": "u", "maximum": "g"}, {"type": "gain", "name": "g", "input": "y", "gain": 1.0}"#,
    ] {
        assert_eq!(codes(&wrap_controls(block, "0.7")), ["E171"]);
    }

    // Loop through Lag.input is allowed even when Lag.minimum reads a valid upstream signal.
    let lag_input_loop = r#"
    {"type": "constant", "name": "lo", "value": -1.0},
    {"type": "lag", "name": "y", "input": "g", "time_constant_s": 0.01, "minimum": "lo"},
    {"type": "gain", "name": "g", "input": "y", "gain": -0.5}
    "#;
    assert!(load_str(&wrap_controls(lag_input_loop, "0.7")).is_ok());

    // topology() lists minimum/maximum/output_min/output_max in inputs when given as signal names.
    let loaded = load_str(&wrap_controls(valid_all, "0.7")).unwrap();
    let topo = openkeito_model::topology(&loaded.model, &[]);
    let inputs_of = |name: &str| -> Vec<(&str, &str)> {
        topo.controls
            .iter()
            .find(|c| c.name == name)
            .unwrap()
            .inputs
            .iter()
            .map(|i| (i.field.as_str(), i.signal.as_str()))
            .collect()
    };
    assert_eq!(
        inputs_of("y_lag"),
        vec![("input", "u"), ("minimum", "lo"), ("maximum", "hi")]
    );
    assert_eq!(
        inputs_of("y_pi"),
        vec![("input", "u"), ("output_min", "lo"), ("output_max", "hi")]
    );
    assert_eq!(
        inputs_of("y_lim"),
        vec![("input", "u"), ("minimum", "lo"), ("maximum", "hi")]
    );
}

#[test]
fn catalog_and_schema_include_version_0_7_additions() {
    let catalog = openkeito_model::component_catalog();
    assert!(catalog.iter().any(|c| c.type_name == "piecewise_linear"));
    for c in catalog.iter().filter(|c| c.category == "circuit") {
        assert!(
            c.parameters.iter().any(|p| p.name == "initialization"),
            "{} lacks initialization parameter",
            c.type_name
        );
    }
    let pi = catalog.iter().find(|c| c.type_name == "pi").unwrap();
    assert!(pi.parameters.iter().any(|p| p.name == "anti_windup"));

    let schema_val = openkeito_model::json_schema();
    let version_desc = schema_val["properties"]["version"]["description"]
        .as_str()
        .unwrap();
    for word in [
        "piecewise_linear",
        "integrator",
        "anti_windup",
        "initialization on components",
    ] {
        assert!(
            version_desc.contains(word),
            "version description lacks {word}: {version_desc}"
        );
    }

    let schema = schema_val.to_string();
    for word in [
        "piecewise_linear",
        "LimitValue",
        "PiAntiWindup",
        "ComponentInitialization",
    ] {
        assert!(schema.contains(word), "schema lacks {word}");
    }
}

#[test]
fn integrator_signal_limits_are_validated_and_version_gated() {
    let wrap_controls = |controls: &str, version: &str| {
        format!(
            r#"{{
  "format": "openkeito-model",
  "version": "{version}",
  "simulation": {{"time_step_s": 1e-5, "end_time_s": 1e-3}},
  "components": [
    {{"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {{"type": "dc", "value": 1.0}}}},
    {{"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0}}
  ],
  "controls": [{controls}]
}}"#
        )
    };

    let valid = r#"
    {"type": "constant", "name": "u", "value": 1.0},
    {"type": "constant", "name": "lo", "value": -1.0},
    {"type": "constant", "name": "hi", "value": 1.0},
    {"type": "integrator", "name": "y_int", "input": "u", "minimum": "lo", "maximum": "hi"}
    "#;
    assert_eq!(codes(&wrap_controls(valid, "0.6")), ["E102"]);
    let loaded = load_str(&wrap_controls(valid, "0.7")).unwrap();
    let topo = openkeito_model::topology(&loaded.model, &[]);
    let inputs: Vec<(&str, &str)> = topo
        .controls
        .iter()
        .find(|c| c.name == "y_int")
        .unwrap()
        .inputs
        .iter()
        .map(|i| (i.field.as_str(), i.signal.as_str()))
        .collect();
    assert_eq!(
        inputs,
        vec![("input", "u"), ("minimum", "lo"), ("maximum", "hi")]
    );

    // Missing limit signal -> E170.
    let missing = r#"
    {"type": "constant", "name": "u", "value": 1.0},
    {"type": "integrator", "name": "y_int", "input": "u", "minimum": "missing"}
    "#;
    assert_eq!(codes(&wrap_controls(missing, "0.7")), ["E170"]);

    // wrap: true with signal limits -> E132.
    let wrapped = r#"
    {"type": "constant", "name": "u", "value": 1.0},
    {"type": "constant", "name": "hi", "value": 6.28},
    {"type": "integrator", "name": "y_int", "input": "u", "minimum": 0.0, "maximum": "hi", "wrap": true}
    "#;
    assert_eq!(codes(&wrap_controls(wrapped, "0.7")), ["E132"]);

    // Direct algebraic loop through minimum or maximum -> E171.
    for field in ["minimum", "maximum"] {
        let loop_json = format!(
            r#"
            {{"type": "constant", "name": "u", "value": 1.0}},
            {{"type": "integrator", "name": "y_int", "input": "u", "{field}": "g"}},
            {{"type": "gain", "name": "g", "input": "y_int", "gain": 1.0}}
            "#
        );
        assert_eq!(codes(&wrap_controls(&loop_json, "0.7")), ["E171"]);
    }
}

#[test]
fn pi_anti_windup_is_validated_and_version_gated() {
    let wrap_pi = |extra: &str, version: &str| {
        format!(
            r#"{{
  "format": "openkeito-model",
  "version": "{version}",
  "simulation": {{"time_step_s": 1e-5, "end_time_s": 1e-3}},
  "components": [
    {{"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {{"type": "dc", "value": 1.0}}}},
    {{"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0}}
  ],
  "controls": [
    {{"type": "constant", "name": "err", "value": 1.0}},
    {{"type": "pi", "name": "y_pi", "input": "err", "proportional_gain": 1.0, "integral_gain_per_s": 10.0, "output_min": -1.0, "output_max": 1.0{extra}}}
  ]
}}"#
        )
    };

    assert!(load_str(&wrap_pi("", "0.2")).is_ok());
    assert!(load_str(&wrap_pi(r#", "anti_windup": "conditional""#, "0.2")).is_ok());
    assert_eq!(
        codes(&wrap_pi(r#", "anti_windup": "state""#, "0.6")),
        ["E102"]
    );
    assert!(load_str(&wrap_pi(r#", "anti_windup": "state""#, "0.7")).is_ok());
}

#[test]
fn component_initialization_exclude_is_validated_and_checks_subcircuit() {
    let wrap_model = |version: &str, components: &str| {
        format!(
            r#"{{
  "format": "openkeito-model",
  "version": "{version}",
  "simulation": {{"time_step_s": 1e-5, "end_time_s": 1e-3, "initialization": "steady_state"}},
  "components": [{components}]
}}"#
        )
    };

    // Version 0.6 rejects initialization: "exclude" with E102.
    let excluded_shunt = r#"
    {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}},
    {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0},
    {"type": "resistor", "name": "R2", "nodes": ["n1", "gnd"], "resistance_ohm": 20.0, "initialization": "exclude"}
    "#;
    assert_eq!(codes(&wrap_model("0.6", excluded_shunt)), ["E102"]);
    assert!(load_str(&wrap_model("0.7", excluded_shunt))
        .unwrap()
        .warnings
        .is_empty());

    // Excluded diode, dc source, controlled source without steady_state, and island resistor
    // skip E180/E182/W240 and do not trigger E183.
    let excluded_non_phasor = r#"
    {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}},
    {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0},
    {"type": "diode", "name": "D1", "nodes": ["n1", "n_dc"], "initialization": "exclude"},
    {"type": "voltage_source", "name": "Edc", "nodes": ["n_dc", "gnd"], "waveform": {"type": "dc", "value": 5.0}, "initialization": "exclude"},
    {"type": "resistor", "name": "R_dc", "nodes": ["n_dc", "gnd"], "resistance_ohm": 5.0, "initialization": "exclude"},
    {"type": "controlled_voltage_source", "name": "Ec", "nodes": ["n_ctrl", "gnd"], "signal": "u_ref", "initialization": "exclude"}
    "#;
    let with_ctrl = format!(
        r#"{{
  "format": "openkeito-model",
  "version": "0.7",
  "simulation": {{"time_step_s": 1e-5, "end_time_s": 1e-3, "initialization": "steady_state"}},
  "components": [{excluded_non_phasor}],
  "controls": [{{"type": "constant", "name": "u_ref", "value": 0.0}}]
}}"#
    );
    let loaded = load_str(&with_ctrl).unwrap();
    assert!(!loaded.warnings.iter().any(|w| w.code == "W240"));

    // Excluding all components -> E183.
    let all_excluded = r#"
    {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}, "initialization": "exclude"},
    {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0, "initialization": "exclude"}
    "#;
    let err = load_str(&wrap_model("0.7", all_excluded)).unwrap_err();
    let err_codes: Vec<&'static str> = err
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.code)
        .collect();
    assert_eq!(err_codes, ["E183"]);

    // Excluding the only grounded path for an included node -> W185 (R_bridge is series) + E183.
    let disconnected = r#"
    {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}},
    {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0},
    {"type": "resistor", "name": "R_bridge", "nodes": ["n1", "n2"], "resistance_ohm": 10.0, "initialization": "exclude"},
    {"type": "resistor", "name": "R_island", "nodes": ["n2", "n3"], "resistance_ohm": 10.0}
    "#;
    assert_eq!(codes(&wrap_model("0.7", disconnected)), ["W185", "E183"]);

    // Excluding a resistor leaves an included node behind a current source floating -> E183.
    let floating = r#"
    {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}},
    {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0},
    {"type": "current_source", "name": "J1", "nodes": ["gnd", "n2"], "waveform": {"type": "sine", "amplitude": 1.0, "frequency_hz": 50.0}},
    {"type": "resistor", "name": "R2", "nodes": ["n2", "gnd"], "resistance_ohm": 10.0, "initialization": "exclude"}
    "#;
    let err = load_str(&wrap_model("0.7", floating)).unwrap_err();
    assert_eq!(err.len(), 1);
    assert_eq!(err[0].code, "E183");
    assert!(err[0].hint.contains("n2"), "{}", err[0].hint);
    assert!(err[0].hint.contains("R2"), "{}", err[0].hint);
}

#[test]
fn w185_warns_when_excluding_series_components_but_not_grounded_shunts() {
    let wrap_model = |components: &str| {
        format!(
            r#"{{
  "format": "openkeito-model",
  "version": "0.7",
  "simulation": {{"time_step_s": 1e-4, "end_time_s": 1e-3, "initialization": "steady_state"}},
  "components": [{components}]
}}"#
        )
    };

    // Single-phase series components (neither node is "gnd") emit W185.
    for (name, series_comp) in [
        (
            "R_ser",
            r#"{"type": "resistor", "name": "R_ser", "nodes": ["n1", "n2"], "resistance_ohm": 5.0, "initialization": "exclude"}"#,
        ),
        (
            "L_ser",
            r#"{"type": "inductor", "name": "L_ser", "nodes": ["n1", "n2"], "inductance_h": 0.01, "initialization": "exclude"}"#,
        ),
        (
            "SW_ser",
            r#"{"type": "switch", "name": "SW_ser", "nodes": ["n1", "n2"], "initially_closed": false, "initialization": "exclude"}"#,
        ),
        (
            "T_1ph",
            r#"{"type": "transformer", "name": "T_1ph", "windings": [["n1", "gnd"], ["n2", "gnd"]], "rated_voltage_kv": [6.6, 0.2], "rated_power_mva": 0.1, "rated_frequency_hz": 50.0, "impedance_percent": 4.0, "resistance_percent": 1.0, "initialization": "exclude"}"#,
        ),
    ] {
        let model = wrap_model(&format!(
            r#"
            {{"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {{"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}}}},
            {{"type": "voltage_source", "name": "E2", "nodes": ["n2", "gnd"], "waveform": {{"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}}}},
            {series_comp}
            "#
        ));
        let loaded = load_str(&model).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let w185: Vec<_> = loaded
            .warnings
            .iter()
            .filter(|w| w.code == "W185")
            .collect();
        assert_eq!(w185.len(), 1, "{name}: {:?}", loaded.warnings);
        assert_eq!(w185[0].severity, Severity::Warning);
        assert_eq!(w185[0].path, "components[2]");
        assert!(w185[0].message.contains(name), "{}", w185[0].message);
    }

    // Three-phase series components (three_phase_transformer, pi_line, bergeron_line, non-gnd coupled_rl) emit W185.
    for (name, series_comp) in [
        (
            "T_3ph",
            r#"{"type": "three_phase_transformer", "name": "T_3ph", "buses": ["b1", "b2"], "vector_group": "YNyn0", "rated_voltage_kv": [66.0, 6.6], "rated_power_mva": 30.0, "rated_frequency_hz": 50.0, "impedance_percent": 10.0, "resistance_percent": 0.5, "initialization": "exclude"}"#,
        ),
        (
            "PI1",
            r#"{"type": "pi_line", "name": "PI1", "buses": ["b1", "b2"], "length_km": 10.0, "per_km": {"type": "sequence", "r1_ohm_per_km": 0.03, "l1_h_per_km": 0.0009, "c1_f_per_km": 1.25e-8, "r0_ohm_per_km": 0.2, "l0_h_per_km": 0.003, "c0_f_per_km": 6.0e-9}, "initialization": "exclude"}"#,
        ),
        (
            "BERG1",
            r#"{"type": "bergeron_line", "name": "BERG1", "buses": ["b1", "b2"], "length_km": 100.0, "per_km": {"type": "sequence", "r1_ohm_per_km": 0.03, "l1_h_per_km": 0.0009, "c1_f_per_km": 1.27e-8, "r0_ohm_per_km": 0.2, "l0_h_per_km": 0.003, "c0_f_per_km": 7.0e-9}, "initialization": "exclude"}"#,
        ),
        (
            "CRL_ser",
            r#"{"type": "coupled_rl", "name": "CRL_ser", "buses": ["b1", "b2"], "impedance": {"type": "sequence", "r1_ohm": 0.5, "l1_h": 0.01, "r0_ohm": 1.0, "l0_h": 0.03}, "initialization": "exclude"}"#,
        ),
    ] {
        let model = wrap_model(&format!(
            r#"
            {{"type": "three_phase_source", "name": "G1", "bus": "b1", "line_voltage_rms_v": 66000.0, "frequency_hz": 50.0, "impedance": {{"type": "sequence", "r1_ohm": 0.5, "l1_h": 0.016, "r0_ohm": 1.0, "l0_h": 0.048}}}},
            {{"type": "three_phase_source", "name": "G2", "bus": "b2", "line_voltage_rms_v": 66000.0, "frequency_hz": 50.0, "impedance": {{"type": "sequence", "r1_ohm": 0.5, "l1_h": 0.016, "r0_ohm": 1.0, "l0_h": 0.048}}}},
            {series_comp}
            "#
        ));
        let loaded = load_str(&model).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let w185: Vec<_> = loaded
            .warnings
            .iter()
            .filter(|w| w.code == "W185")
            .collect();
        assert_eq!(w185.len(), 1, "{name}: {:?}", loaded.warnings);
        assert_eq!(w185[0].path, "components[2]");
        assert!(w185[0].message.contains(name), "{}", w185[0].message);
    }

    // Shunts to "gnd" (1-phase and 3-phase) MUST NOT emit W185.
    let shunts_model = wrap_model(
        r#"
        {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}},
        {"type": "resistor", "name": "R_base", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0},
        {"type": "inductor", "name": "L_shunt", "nodes": ["n1", "gnd"], "inductance_h": 0.05, "initialization": "exclude"},
        {"type": "nonlinear_inductor", "name": "NL_shunt", "nodes": ["n1", "gnd"], "current_a": [1.0, 100.0], "flux_linkage_wb": [30.0, 36.0], "initialization": "exclude"},
        {"type": "resistor", "name": "R_shunt", "nodes": ["n1", "gnd"], "resistance_ohm": 50.0, "initialization": "exclude"},
        {"type": "capacitor", "name": "C_shunt", "nodes": ["n1", "gnd"], "capacitance_f": 1e-6, "initialization": "exclude"},
        {"type": "three_phase_source", "name": "G1", "bus": "b1", "line_voltage_rms_v": 66000.0, "frequency_hz": 50.0, "impedance": {"type": "sequence", "r1_ohm": 0.5, "l1_h": 0.016, "r0_ohm": 1.0, "l0_h": 0.048}},
        {"type": "coupled_rl", "name": "CRL_gnd", "buses": ["b1", "gnd"], "impedance": {"type": "sequence", "r1_ohm": 10.0, "l1_h": 0.05, "r0_ohm": 20.0, "l0_h": 0.1}, "initialization": "exclude"},
        {"type": "coupled_rl", "name": "CRL_gnd_nodes", "buses": ["b1", ["gnd", "gnd", "gnd"]], "impedance": {"type": "sequence", "r1_ohm": 10.0, "l1_h": 0.05, "r0_ohm": 20.0, "l0_h": 0.1}, "initialization": "exclude"},
        {"type": "three_phase_load", "name": "LD_shunt", "bus": "b1", "rated_voltage_kv": 66.0, "rated_frequency_hz": 50.0, "active_power_mw": 5.0, "reactive_power_mvar": 1.0, "initialization": "exclude"}
        "#,
    );
    let loaded = load_str(&shunts_model).unwrap();
    assert!(
        !loaded.warnings.iter().any(|w| w.code == "W185"),
        "unexpected W185: {:?}",
        loaded.warnings
    );
}

#[test]
fn source_frequency_ignores_excluded_sources() {
    let mixed = r#"{
  "format": "openkeito-model",
  "version": "0.7",
  "simulation": {"time_step_s": 1e-5, "end_time_s": 1e-3, "initialization": "steady_state"},
  "components": [
    {"type": "voltage_source", "name": "E_ex", "nodes": ["n_ex", "gnd"], "waveform": {"type": "sine", "amplitude": 50.0, "frequency_hz": 60.0}, "initialization": "exclude"},
    {"type": "resistor", "name": "R_ex", "nodes": ["n_ex", "gnd"], "resistance_ohm": 10.0, "initialization": "exclude"},
    {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}},
    {"type": "capacitor", "name": "C1", "nodes": ["n1", "gnd"], "capacitance_f": 100e-6}
  ]
}"#;
    let loaded = load_str(mixed).unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(openkeito_model::source_frequency(&loaded.model), Some(50.0));

    let all_sines_excluded = r#"{
  "format": "openkeito-model",
  "version": "0.7",
  "simulation": {"time_step_s": 1e-5, "end_time_s": 1e-3, "initialization": "steady_state"},
  "components": [
    {"type": "voltage_source", "name": "E_ex", "nodes": ["n_ex", "gnd"], "waveform": {"type": "sine", "amplitude": 50.0, "frequency_hz": 60.0}, "initialization": "exclude"},
    {"type": "resistor", "name": "R_ex", "nodes": ["n_ex", "gnd"], "resistance_ohm": 10.0, "initialization": "exclude"},
    {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0}
  ]
}"#;
    let loaded = load_str(all_sines_excluded).unwrap();
    assert_eq!(
        loaded.warnings.iter().map(|w| w.code).collect::<Vec<_>>(),
        ["W184"]
    );
    assert_eq!(openkeito_model::source_frequency(&loaded.model), None);
}

#[test]
fn expression_block_is_validated_compiled_and_version_gated() {
    let wrap_controls = |controls: &str, version: &str| {
        format!(
            r#"{{
  "format": "openkeito-model",
  "version": "{version}",
  "simulation": {{"time_step_s": 1e-4, "end_time_s": 1e-3}},
  "components": [
    {{"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {{"type": "dc", "value": 1.0}}}},
    {{"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0}}
  ],
  "controls": [{controls}]
}}"#
        )
    };

    // Version 0.1..0.7 continue to load without expression; 0.7 rejects expression with E102.
    for v in ["0.1", "0.2", "0.3", "0.4", "0.5", "0.6", "0.7"] {
        assert!(
            load_str(&VALID.replace("\"0.1\"", &format!("\"{v}\""))).is_ok(),
            "version {v} failed"
        );
    }

    let basic = r#"
    {"type": "constant", "name": "x", "value": 2.0},
    {"type": "expression", "name": "y_out", "inputs": ["x"], "expression": "2*x^2 + sin(x)"}
    "#;
    assert_eq!(codes(&wrap_controls(basic, "0.7")), ["E102"]);
    assert!(load_str(&wrap_controls(basic, "0.8")).is_ok());

    // Complex valid expression with variables, positional inputs, pi/e, and all 14 functions.
    let rich = r#"
    {"type": "constant", "name": "k", "value": 1.0},
    {"type": "abc_to_dq0", "name": "v", "inputs": ["k", "k", "k"], "angle": "k"},
    {"type": "constant", "name": "e", "value": 3.0},
    {"type": "expression", "name": "y1", "inputs": ["k", "v.q"], "variables": {"vd": "v.d"}, "expression": "hypot(vd, u1) + pow(u0, 2) + pi + e^(-1) + min(1, 2, 3) + max(-1, 0) + sign(vd) + atan2(u1, vd) + sin(0) + cos(0) + tan(0) + atan(0) + abs(-1) + sqrt(4) + exp(0) + ln(1)"},
    {"type": "expression", "name": "y2", "inputs": ["e"], "expression": "2 * e"},
    {"type": "expression", "name": "y_const", "expression": "2 * pi"}
    "#;
    let loaded = load_str(&wrap_controls(rich, "0.8")).unwrap();
    let topo = openkeito_model::topology(&loaded.model, &[]);
    let y1_inputs: Vec<(&str, &str)> = topo
        .controls
        .iter()
        .find(|c| c.name == "y1")
        .unwrap()
        .inputs
        .iter()
        .map(|i| (i.field.as_str(), i.signal.as_str()))
        .collect();
    assert_eq!(
        y1_inputs,
        vec![
            ("inputs[0]", "k"),
            ("inputs[1]", "v.q"),
            ("variables.vd", "v.d")
        ]
    );

    if let openkeito_model::Control::Expression(ref expr_block) = loaded.model.controls[4] {
        let compiled = expr_block.compile().unwrap();
        assert_eq!(compiled.resolved_signals(), ["e"]);
        assert_eq!(compiled.eval(&[3.0]), 6.0);
    } else {
        panic!("expected Expression block");
    }

    // Missing signals in inputs or variables -> E170.
    let missing_input = r#"
    {"type": "expression", "name": "y", "inputs": ["missing"], "expression": "missing"}
    "#;
    assert_eq!(codes(&wrap_controls(missing_input, "0.8")), ["E170"]);

    let missing_var = r#"
    {"type": "expression", "name": "y", "variables": {"vd": "missing.d"}, "expression": "vd"}
    "#;
    assert_eq!(codes(&wrap_controls(missing_var, "0.8")), ["E170"]);

    // Algebraic loops through inputs or variables -> E171.
    let loop_input = r#"
    {"type": "expression", "name": "a", "inputs": ["b"], "expression": "b + 1"},
    {"type": "gain", "name": "b", "input": "a", "gain": 0.5}
    "#;
    assert_eq!(codes(&wrap_controls(loop_input, "0.8")), ["E171"]);

    let loop_var = r#"
    {"type": "expression", "name": "a", "variables": {"vb": "b"}, "expression": "vb + 1"},
    {"type": "gain", "name": "b", "input": "a", "gain": 0.5}
    "#;
    assert_eq!(codes(&wrap_controls(loop_var, "0.8")), ["E171"]);

    // Loop broken by an integrator is valid.
    let loop_with_state = r#"
    {"type": "expression", "name": "err", "inputs": ["x"], "expression": "1 - x"},
    {"type": "integrator", "name": "x", "input": "err"}
    "#;
    assert!(load_str(&wrap_controls(loop_with_state, "0.8")).is_ok());
}

#[test]
fn expression_block_reports_e243_with_paths_and_hints() {
    let wrap_controls = |controls: &str| {
        format!(
            r#"{{
  "format": "openkeito-model",
  "version": "0.8",
  "simulation": {{"time_step_s": 1e-4, "end_time_s": 1e-3}},
  "components": [
    {{"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {{"type": "dc", "value": 1.0}}}},
    {{"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0}}
  ],
  "controls": [{controls}]
}}"#
        )
    };

    let expect_e243 = |controls: &str, expected_path: &str| -> openkeito_model::Diagnostic {
        let errs = load_str(&wrap_controls(controls)).unwrap_err();
        assert_eq!(errs.len(), 1, "unexpected diagnostics: {errs:?}");
        assert_eq!(errs[0].code, "E243");
        assert_eq!(errs[0].path, expected_path);
        assert!(!errs[0].message.contains("  "), "{}", errs[0].message);
        assert!(!errs[0].hint.contains("  "), "{}", errs[0].hint);
        errs.into_iter().next().unwrap()
    };

    // Empty expression.
    expect_e243(
        r#"{"type": "expression", "name": "y", "expression": ""}"#,
        "controls[0].expression",
    );

    // Syntax error.
    expect_e243(
        r#"{"type": "constant", "name": "x", "value": 1.0}, {"type": "expression", "name": "y", "inputs": ["x"], "expression": "x +"}"#,
        "controls[1].expression",
    );

    // Dot in expression points to "variables" or u0.
    let d = expect_e243(
        r#"{"type": "constant", "name": "k", "value": 1.0}, {"type": "abc_to_dq0", "name": "v", "inputs": ["k", "k", "k"], "angle": "k"}, {"type": "expression", "name": "y", "inputs": ["v.d"], "expression": "v.d * 2"}"#,
        "controls[2].expression",
    );
    assert!(d.hint.contains("\"variables\""), "{}", d.hint);
    assert!(d.hint.contains("u0"), "{}", d.hint);

    // Non-finite number literal.
    expect_e243(
        r#"{"type": "expression", "name": "y", "expression": "1e999"}"#,
        "controls[0].expression",
    );

    // Unknown variable & out-of-range positional uN.
    expect_e243(
        r#"{"type": "constant", "name": "x", "value": 1.0}, {"type": "expression", "name": "y", "inputs": ["x"], "expression": "z + 1"}"#,
        "controls[1].expression",
    );
    expect_e243(
        r#"{"type": "constant", "name": "x", "value": 1.0}, {"type": "expression", "name": "y", "inputs": ["x"], "expression": "u1"}"#,
        "controls[1].expression",
    );

    // Unknown function and wrong arity.
    expect_e243(
        r#"{"type": "constant", "name": "x", "value": 1.0}, {"type": "expression", "name": "y", "inputs": ["x"], "expression": "foo(x)"}"#,
        "controls[1].expression",
    );
    for expr_text in ["sin()", "sin(x, x)", "atan2(x)", "pow(x)", "min()", "max()"] {
        expect_e243(
            &format!(
                r#"{{"type": "constant", "name": "x", "value": 1.0}}, {{"type": "expression", "name": "y", "inputs": ["x"], "expression": "{expr_text}"}}"#
            ),
            "controls[1].expression",
        );
    }

    // Invalid variable identifier, builtin name collision, and inputs collision.
    expect_e243(
        r#"{"type": "constant", "name": "x", "value": 1.0}, {"type": "expression", "name": "y", "variables": {"bad.name": "x"}, "expression": "1"}"#,
        "controls[1].variables.bad.name",
    );
    expect_e243(
        r#"{"type": "constant", "name": "x", "value": 1.0}, {"type": "expression", "name": "y", "variables": {"sin": "x"}, "expression": "1"}"#,
        "controls[1].variables.sin",
    );
    expect_e243(
        r#"{"type": "constant", "name": "x", "value": 1.0}, {"type": "constant", "name": "z", "value": 2.0}, {"type": "expression", "name": "y", "inputs": ["x"], "variables": {"x": "z"}, "expression": "x"}"#,
        "controls[2].variables.x",
    );

    // Duplicate inputs.
    expect_e243(
        r#"{"type": "constant", "name": "x", "value": 1.0}, {"type": "expression", "name": "y", "inputs": ["x", "x"], "expression": "x"}"#,
        "controls[1].inputs[1]",
    );

    // Nesting depth boundary: 256 levels succeeds, 257 levels returns E243.
    let at_limit = format!("{}x{}", "(".repeat(256), ")".repeat(256));
    let ok_model = format!(
        r#"{{"type": "constant", "name": "x", "value": 1.0}}, {{"type": "expression", "name": "y", "inputs": ["x"], "expression": "{at_limit}"}}"#
    );
    assert!(load_str(&wrap_controls(&ok_model)).is_ok());

    let over_limit = format!("{}x{}", "(".repeat(257), ")".repeat(257));
    expect_e243(
        &format!(r#"{{"type": "expression", "name": "y", "expression": "{over_limit}"}}"#),
        "controls[0].expression",
    );
}

#[test]
fn catalog_and_schema_include_version_0_8_additions() {
    let catalog = openkeito_model::component_catalog();
    assert_eq!(catalog.len(), 59);
    let circuit_count = catalog.iter().filter(|c| c.category == "circuit").count();
    let control_count = catalog.iter().filter(|c| c.category == "control").count();
    assert_eq!(circuit_count, 27);
    assert_eq!(control_count, 32);

    let fn_pos = catalog
        .iter()
        .position(|c| c.type_name == "function")
        .unwrap();
    let expr_pos = catalog
        .iter()
        .position(|c| c.type_name == "expression")
        .unwrap();
    assert_eq!(expr_pos, fn_pos + 1);

    let expr_info = &catalog[expr_pos];
    let parsed: openkeito_model::Control =
        serde_json::from_value(expr_info.example.clone()).unwrap();
    if let openkeito_model::Control::Expression(e) = parsed {
        assert!(e.compile().is_ok());
    } else {
        panic!("expected Expression");
    }

    let schema_val = openkeito_model::json_schema();
    let version_desc = schema_val["properties"]["version"]["description"]
        .as_str()
        .unwrap();
    assert!(version_desc.contains("0.8"), "{version_desc}");
    assert!(version_desc.contains("expression"), "{version_desc}");

    let schema = schema_val.to_string();
    assert!(schema.contains("expression"), "schema lacks expression");
}

const THYRISTOR: &str = r#"{
  "format": "openkeito-model",
  "version": "0.9",
  "simulation": {"time_step_s": 1e-5, "end_time_s": 1e-3},
  "components": [
    {"type": "voltage_source", "name": "E1", "nodes": ["a", "gnd"], "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}},
    {"type": "thyristor", "name": "T1", "nodes": ["a", "k"], "gate": "fire"},
    {"type": "resistor", "name": "R1", "nodes": ["k", "gnd"], "resistance_ohm": 10.0}
  ],
  "controls": [{"type": "step", "name": "fire", "time_s": 2e-3, "after": 1.0}],
  "outputs": [{"type": "branch_current", "component": "T1"}]
}"#;

#[test]
fn a_thyristor_is_an_addition_of_version_0_9() {
    let loaded = load_str(THYRISTOR).unwrap();
    match &loaded.model.components[1] {
        openkeito_model::Component::Thyristor(t) => {
            assert_eq!(t.gate, "fire");
            assert_eq!(t.on_resistance_ohm, 1e-3);
            assert_eq!(t.off_resistance_ohm, 1e6);
        }
        c => panic!("{c:?}"),
    }
    let e = errors(&THYRISTOR.replace("\"0.9\"", "\"0.8\""));
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].code, "E102");
    assert_eq!(e[0].path, "components[1] (thyristor)");
    assert!(e[0].hint.contains("\"version\": \"0.9\""), "{}", e[0].hint);
    // The gate is a signal, the resistances those of a diode.
    let e = errors(&THYRISTOR.replace("\"gate\": \"fire\"", "\"gate\": \"fir\""));
    assert_eq!(e[0].code, "E170");
    assert_eq!(e[0].path, "components[1].gate");
    let e = errors(&THYRISTOR.replace(
        "\"gate\": \"fire\"",
        "\"gate\": \"fire\", \"on_resistance_ohm\": 1.0, \"off_resistance_ohm\": 0.5",
    ));
    assert_eq!(e[0].code, "E132");
    let catalog = openkeito_model::component_catalog();
    let info = catalog.iter().find(|c| c.type_name == "thyristor").unwrap();
    let parsed: openkeito_model::Component = serde_json::from_value(info.example.clone()).unwrap();
    assert_eq!(parsed.type_name(), "thyristor");
    let schema = openkeito_model::json_schema();
    let version = schema["properties"]["version"]["description"]
        .as_str()
        .unwrap();
    assert!(
        version.contains("0.9") && version.contains("thyristor"),
        "{version}"
    );
}
