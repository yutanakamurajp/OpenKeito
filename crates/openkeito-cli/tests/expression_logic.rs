//! The comparison and logical operators of `expression` (format 0.10) in a
//! run: the example `expression_logic.json` gives byte for byte the files of
//! the same gate written with `sign` and `abs`, and of the same gate built
//! from two comparators and a product; its switch acts at the closed-form
//! instants; and `acos` beyond 1 stops the run with E251.

use std::path::Path;

use openkeito_cli::{load_text, run_texts};
use serde_json::{json, Value};

fn example() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/expression_logic.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The text of `result.csv` and `events.csv` of a run.
fn files(model: &Value) -> (String, String) {
    let (loaded, sha256) = load_text(&model.to_string()).unwrap_or_else(|d| panic!("{d:#?}"));
    let out = run_texts("<inline>", &loaded, &sha256, None);
    assert_eq!(out.code, 0, "{}", out.summary);
    assert_eq!(out.summary["status"], "completed");
    assert_eq!(out.summary["warnings"], json!([]));
    let text = |name: &str| {
        out.texts
            .iter()
            .find(|(n, _)| *n == name)
            .unwrap_or_else(|| panic!("no {name}"))
            .1
            .clone()
    };
    (text("result.csv"), text("events.csv"))
}

/// The example with another gate: `controls` after the sine wave `x`.
fn with_gate(gate: Value) -> Value {
    let mut m = example();
    let controls = m["controls"].as_array_mut().unwrap();
    assert_eq!(controls[0]["name"], "x");
    assert_eq!(controls[1]["name"], "gate");
    controls.truncate(1);
    controls.extend(gate.as_array().unwrap().iter().cloned());
    m
}

fn with_switching(mut m: Value, switching: &str) -> Value {
    m["simulation"]["switching"] = switching.into();
    m
}

#[test]
fn the_gate_from_comparisons_equals_the_gates_from_sign_and_abs_and_from_comparators() {
    let logic = example();
    assert_eq!(logic["controls"][1]["expression"], "(x > 0.3) && (x < 0.8)");
    // x > a as (sign(x - a) + abs(sign(x - a)))/2 and p && q as
    // abs(sign(p))*abs(sign(q)): 1 for true and +0 for false, as the
    // operators give.
    let p = "(sign(x - 0.3) + abs(sign(x - 0.3)))/2";
    let q = "(sign(0.8 - x) + abs(sign(0.8 - x)))/2";
    let sign_abs = with_gate(json!([{
        "type": "expression", "name": "gate", "inputs": ["x"],
        "expression": format!("abs(sign({p}))*abs(sign({q}))")
    }]));
    let comparators = with_gate(json!([
        {"type": "constant", "name": "low", "value": 0.3},
        {"type": "constant", "name": "high", "value": 0.8},
        {"type": "comparator", "name": "above", "inputs": ["x", "low"]},
        {"type": "comparator", "name": "below", "inputs": ["high", "x"]},
        {"type": "product", "name": "gate", "inputs": ["above", "below"]},
    ]));
    let mut by_switching = Vec::new();
    for switching in ["grid", "interpolated"] {
        let expected = files(&with_switching(logic.clone(), switching));
        assert!(
            expected.1.lines().count() > 8,
            "{switching}: {}",
            expected.1
        );
        for (name, other) in [("sign and abs", &sign_abs), ("comparators", &comparators)] {
            let got = files(&with_switching(other.clone(), switching));
            assert!(
                got.0 == expected.0,
                "{switching}, {name}: result.csv differs"
            );
            assert!(
                got.1 == expected.1,
                "{switching}, {name}: events.csv differs"
            );
        }
        by_switching.push(expected);
    }
    // The default of version 0.10 is interpolated, and it moves the events.
    assert!(files(&logic) == by_switching[1]);
    assert!(by_switching[0].1 != by_switching[1].1);
}

#[test]
fn the_switch_acts_where_the_sine_wave_crosses_the_band() {
    let (_, events) = files(&example());
    let w = 2.0 * std::f64::consts::PI * 50.0;
    let (low, high) = (0.3_f64.asin() / w, 0.8_f64.asin() / w);
    let half = std::f64::consts::PI / w;
    let period = 0.02;
    let mut expected = Vec::new();
    for k in 0..2 {
        let t0 = k as f64 * period;
        expected.extend([
            (t0 + low, "close"),
            (t0 + high, "open"),
            (t0 + half - high, "close"),
            (t0 + half - low, "open"),
        ]);
    }
    let got: Vec<(f64, String)> = events
        .lines()
        .skip(1)
        .map(|l| l.split(',').collect::<Vec<_>>())
        .filter(|f| f[1] == "S1")
        .map(|f| (f[0].parse().unwrap(), f[2].to_string()))
        .collect();
    assert_eq!(got.len(), expected.len(), "{events}");
    for ((t, action), (t_exact, action_exact)) in got.iter().zip(&expected) {
        assert_eq!(action, action_exact, "at {t}");
        assert!(
            (t - t_exact).abs() < 1e-12,
            "{action} at {t:e} s, exact {t_exact:e} s"
        );
    }
}

#[test]
fn acos_beyond_one_stops_the_run_with_e251_and_keeps_the_rows_before() {
    let model = json!({
        "format": "openkeito-model",
        "version": "0.10",
        "simulation": {"time_step_s": 1e-4, "end_time_s": 2e-3},
        "components": [
            {"type": "controlled_voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "signal": "angle"},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 1.0},
        ],
        "controls": [
            {"type": "ramp", "name": "x", "slope_per_s": 900.0},
            {"type": "expression", "name": "angle", "inputs": ["x"], "expression": "acos(x)"},
        ],
        "outputs": [{"type": "signal", "signal": "angle"}],
    });
    let (loaded, sha256) = load_text(&model.to_string()).unwrap();
    let out = run_texts("<inline>", &loaded, &sha256, None);
    assert_ne!(out.code, 0);
    assert_eq!(out.summary["status"], "failed");
    let error = &out.summary["errors"][0];
    assert_eq!(error["code"], "E251", "{}", out.summary);
    assert_eq!(error["path"], "angle");
    assert!(
        error["message"].as_str().unwrap().contains("(expression)"),
        "{error}"
    );
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .contains("min(max(x, -1), 1)"),
        "{error}"
    );
    // x = 900 t passes 1 at 1.11 ms: the rows up to 1.1 ms are kept.
    let csv = &out
        .texts
        .iter()
        .find(|(n, _)| *n == "result.csv")
        .expect("the rows before the failure")
        .1;
    let rows: Vec<&str> = csv.lines().skip(1).collect();
    assert_eq!(rows.len(), 12, "{csv}");
    let last: f64 = rows[11].split(',').nth(1).unwrap().parse().unwrap();
    assert_eq!(last.to_bits(), (900.0_f64 * 11.0 * 1e-4).acos().to_bits());
}

#[test]
fn a_model_of_0_9_with_a_signal_named_round_still_runs() {
    let model = |version: &str| {
        json!({
            "format": "openkeito-model",
            "version": version,
            "simulation": {"time_step_s": 1e-4, "end_time_s": 1e-3},
            "components": [
                {"type": "controlled_voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "signal": "y"},
                {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 1.0},
            ],
            "controls": [
                {"type": "ramp", "name": "round", "slope_per_s": 1000.0},
                {"type": "expression", "name": "y", "inputs": ["round"], "expression": "2*round"},
            ],
            "outputs": [{"type": "signal", "signal": "y"}],
        })
    };
    let (result, _) = files(&model("0.9"));
    let last = result.lines().last().unwrap();
    assert_eq!(last.split(',').nth(1).unwrap().parse::<f64>().unwrap(), 2.0);
    // In 0.10 the name is that of the function.
    let e = load_text(&model("0.10").to_string()).err().unwrap();
    assert_eq!(e[0].code, "E243");
}
