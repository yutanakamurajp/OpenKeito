//! The comparison and logical operators and the functions acos, asin, floor,
//! ceil, round and trunc of `expression` (format 0.10): validation by the
//! version that a model declares.

use openkeito_model::{load_str, parse_str};
use serde_json::{json, Value};

fn model(version: &str, controls: Value) -> String {
    json!({
        "format": "openkeito-model",
        "version": version,
        "simulation": {"time_step_s": 1e-5, "end_time_s": 1e-4},
        "components": [
            {"type": "voltage_source", "name": "V1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0}
        ],
        "controls": controls,
    })
    .to_string()
}

fn sources() -> Vec<Value> {
    vec![
        json!({"type": "step", "name": "x", "before": 0.0, "after": 1.0, "time_s": 5e-5}),
        json!({"type": "step", "name": "round", "before": 0.4, "after": 2.5, "time_s": 5e-5}),
    ]
}

fn with(block: Value) -> Value {
    let mut v = sources();
    v.push(block);
    Value::Array(v)
}

fn codes(version: &str, block: Value) -> Vec<&'static str> {
    let (_, d) = parse_str(&model(version, with(block))).expect("the model can be read");
    d.iter().map(|d| d.code).collect()
}

#[test]
fn the_operators_and_functions_are_valid_in_0_10() {
    for text in [
        "(x > 0.5) && !(x >= 2) || x == 0",
        "acos(min(max(x, -1), 1)) + asin(0.5*x)",
        "floor(x) + ceil(x) + trunc(-x) + round(2.5*x)",
        "x != 1",
    ] {
        let block = json!({"type": "expression", "name": "y", "inputs": ["x"], "expression": text});
        assert_eq!(codes("0.10", block), Vec::<&str>::new(), "{text}");
    }
}

#[test]
fn an_older_version_gets_e102_for_the_operators_and_functions_of_0_10() {
    for (text, what) in [
        ("(x > 0.5) * 2", "('>' at character 4)"),
        ("!x", "('!' at character 1)"),
        ("x + (x || 1)", "('||' at character 8)"),
        ("2 * floor(x)", "(floor at character 5)"),
        ("acos(x)", "(acos at character 1)"),
    ] {
        let block = json!({"type": "expression", "name": "y", "inputs": ["x"], "expression": text});
        for version in ["0.8", "0.9"] {
            let e = load_str(&model(version, with(block.clone()))).unwrap_err();
            assert_eq!(e.len(), 1, "{text} in {version}: {e:?}");
            assert_eq!(e[0].code, "E102", "{text} in {version}");
            assert_eq!(
                e[0].path,
                format!("controls[2].expression {what}"),
                "{text} in {version}"
            );
            assert!(
                e[0].hint.starts_with("Set \"version\": \"0.10\"."),
                "{}",
                e[0].hint
            );
            assert!(
                e[0].hint.contains(
                    "the comparison and logical operators of expression and the functions \
                     acos, asin, floor, ceil, round and trunc of expression."
                ),
                "{}",
                e[0].hint
            );
        }
    }
    // An error of the syntax is E243 in every version.
    let block =
        json!({"type": "expression", "name": "y", "inputs": ["x"], "expression": "0 < x < 1"});
    assert_eq!(codes("0.9", block.clone()), ["E243"]);
    assert_eq!(codes("0.10", block), ["E243"]);
}

#[test]
fn a_signal_named_round_still_works_in_an_older_version() {
    // A block of 0.8 that reads a signal named after a function of 0.10.
    let block = json!({"type": "expression", "name": "y", "inputs": ["round", "x"], "expression": "2*round - x"});
    let text = model("0.9", with(block.clone()));
    let loaded = load_str(&text).unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(codes("0.8", block.clone()), Vec::<&str>::new());
    // So does a variable named trunc.
    let alias = json!({"type": "expression", "name": "y", "variables": {"trunc": "round"}, "expression": "trunc + 1"});
    assert_eq!(codes("0.9", alias.clone()), Vec::<&str>::new());
    // From 0.10 on, the names are those of the functions.
    let (_, d) = parse_str(&model("0.10", with(block))).unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].code, "E243");
    assert_eq!(d[0].path, "controls[2].expression");
    assert!(
        d[0].message.contains("\"round\" used without parentheses"),
        "{}",
        d[0].message
    );
    let (_, d) = parse_str(&model("0.10", with(alias))).unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].code, "E243");
    assert_eq!(d[0].path, "controls[2].variables.trunc");
    assert!(d[0].message.contains("collides"), "{}", d[0].message);
}
