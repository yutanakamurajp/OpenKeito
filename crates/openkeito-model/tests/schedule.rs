//! Schedule files: loading and every diagnostic.

use openkeito_model::{load_schedule_str, Diagnostic, Interpolation, ScheduledQuantity};
use serde_json::{json, Value};

fn good() -> Value {
    json!({
        "format": "openkeito-schedule", "version": "0.1", "name": "day",
        "times": ["2026-11-21T00:00:00+09:00", "2026-11-21T01:00:00+09:00", "2026-11-21T02:00:00+09:00"],
        "series": [
            {"target": "gen/0", "quantity": "active_power_mw", "values": [250.0, 230.0, 210.0]},
            {"target": "gen/0", "quantity": "voltage_pu", "values": [1.02, 1.01, 1.0]},
            {"target": "trafo/3", "quantity": "tap_position", "values": [0.0, -1.0, 2.0]},
            {"target": "line/7", "quantity": "in_service", "values": [1.0, 0.0, 1.0]}
        ]
    })
}

fn errors(v: &Value) -> Vec<Diagnostic> {
    load_schedule_str(&v.to_string()).expect_err("must be rejected")
}

fn only(v: &Value) -> Diagnostic {
    let e = errors(v);
    assert_eq!(e.len(), 1, "{e:#?}");
    e.into_iter().next().unwrap()
}

#[test]
fn a_schedule_loads() {
    let s = load_schedule_str(&good().to_string()).unwrap();
    assert_eq!(s.interpolation, Interpolation::Step);
    assert_eq!(s.series[1].quantity, ScheduledQuantity::VoltagePu);
    assert_eq!(s.series[2].values, vec![0.0, -1.0, 2.0]);
}

#[test]
fn format_version_and_empty_lists() {
    let mut v = good();
    v["format"] = json!("openkeito-model");
    v["version"] = json!("0.2");
    let e = errors(&v);
    assert_eq!(
        e.iter()
            .map(|d| (d.code, d.path.as_str()))
            .collect::<Vec<_>>(),
        [("E500", "format"), ("E500", "version")]
    );
    let mut v = good();
    v["times"] = json!([]);
    v["series"] = json!([]);
    let e = errors(&v);
    assert_eq!(e[0].code, "E501");
    assert_eq!(e[1].code, "E501");
}

#[test]
fn times_are_date_times_in_ascending_order() {
    let mut v = good();
    v["times"][1] = json!("2026-11-21 01:00");
    let d = only(&v);
    assert_eq!((d.code, d.path.as_str()), ("E502", "times[1]"));
    assert!(d.hint.contains("+09:00"), "{}", d.hint);
    // The same instant written with another offset is a duplicate.
    let mut v = good();
    v["times"][1] = json!("2026-11-20T15:00:00Z");
    let d = only(&v);
    assert_eq!((d.code, d.path.as_str()), ("E503", "times[1]"));
}

#[test]
fn series_lengths_duplicates_targets_and_values() {
    let mut v = good();
    v["series"][0]["values"] = json!([1.0, 2.0]);
    let d = only(&v);
    assert_eq!((d.code, d.path.as_str()), ("E504", "series[0].values"));

    let mut v = good();
    v["series"][1]["quantity"] = json!("active_power_mw");
    let d = only(&v);
    assert_eq!((d.code, d.path.as_str()), ("E505", "series[1]"));

    let mut v = good();
    v["series"][3]["target"] = json!(" ");
    let d = only(&v);
    assert_eq!((d.code, d.path.as_str()), ("E507", "series[3].target"));

    for (k, j, bad) in [(1, 2, json!(0.0)), (2, 0, json!(0.5)), (3, 1, json!(2.0))] {
        let mut v = good();
        v["series"][k]["values"][j] = bad;
        let d = only(&v);
        assert_eq!(
            (d.code, d.path.clone()),
            ("E506", format!("series[{k}].values[{j}]"))
        );
    }
}

#[test]
fn an_unknown_quantity_is_a_parse_error() {
    let mut v = good();
    v["series"][0]["quantity"] = json!("p_mw");
    let d = only(&v);
    assert!(d.message.contains("p_mw"), "{}", d.message);
}
