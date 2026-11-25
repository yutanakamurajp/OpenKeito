//! End-to-end checks of the `openkeito` command.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

fn openkeito(args: &[&str]) -> (Value, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_openkeito"))
        .args(args)
        .output()
        .expect("openkeito starts");
    let text = String::from_utf8(out.stdout).expect("output is UTF-8");
    let json = serde_json::from_str(&text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"));
    (json, out.status.code().expect("exit code"))
}

fn example(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
        .display()
        .to_string()
}

/// A fresh directory under the build's temporary folder.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn run_writes_a_csv_that_matches_the_analytic_solution() {
    let dir = scratch("run_rlc");
    let out = dir.join("out");
    let (summary, code) = openkeito(&["run", &example("rlc.json"), "--out", out.to_str().unwrap()]);
    assert_eq!(code, 0, "{summary}");
    assert_eq!(summary["status"], "completed");
    assert_eq!(summary["result"]["rows"], 10001);
    assert_eq!(summary["result"]["columns"][0]["name"], "V(n3)");
    assert_eq!(summary["result"]["columns"][1]["unit"], "A");
    assert_eq!(summary["solver"]["factorizations"], 2);
    assert_eq!(summary["model"]["sha256"].as_str().unwrap().len(), 64);
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("summary.json")).unwrap()).unwrap();
    assert_eq!(on_disk, summary);

    let csv = std::fs::read_to_string(out.join("result.csv")).unwrap();
    let mut lines = csv.lines();
    assert_eq!(lines.next(), Some("time_s,V(n3),I(L1)"));
    let (r, l, c): (f64, f64, f64) = (10.0, 0.01, 1e-6);
    let a = r / (2.0 * l);
    let wd = (1.0 / (l * c) - a * a).sqrt();
    let mut rows = 0;
    let mut worst: f64 = 0.0;
    for line in lines {
        let v: Vec<f64> = line.split(',').map(|s| s.parse().unwrap()).collect();
        assert_eq!(v.len(), 3);
        let t = v[0];
        let exact = 1.0 - (-a * t).exp() * ((wd * t).cos() + a / wd * (wd * t).sin());
        worst = worst.max((v[1] - exact).abs());
        rows += 1;
    }
    assert_eq!(rows, 10001);
    assert!(worst < 5e-4, "largest deviation {worst}");

    // A second run into the same directory is refused.
    let (again, code) = openkeito(&["run", &example("rlc.json"), "--out", out.to_str().unwrap()]);
    assert_eq!(code, 2);
    assert_eq!(again["errors"][0]["code"], "E300");
}

#[test]
fn run_reports_switch_events() {
    let dir = scratch("run_breaker");
    let out = dir.join("out");
    let (summary, code) = openkeito(&[
        "run",
        &example("rl_breaker.json"),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{summary}");
    let events = summary["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["action"], "close");
    assert_eq!(events[0]["component"], "CB1");
    assert_eq!(events[1]["action"], "open");
    let opened = events[1]["time_s"].as_f64().unwrap();
    assert!(opened > 0.052 && opened < 0.063, "{opened}");
}

#[test]
fn validate_explains_what_to_fix() {
    let dir = scratch("validate");
    let bad = dir.join("bad.json");
    let text = std::fs::read_to_string(example("rlc.json"))
        .unwrap()
        .replace("\"capacitance_f\": 1e-6", "\"capacitance_f\": 1.1141-6");
    std::fs::write(&bad, text).unwrap();
    let (report, code) = openkeito(&["validate", bad.to_str().unwrap()]);
    assert_eq!(code, 2);
    assert_eq!(report["valid"], false);
    assert_eq!(report["errors"][0]["code"], "E001");
    assert!(report["errors"][0]["hint"]
        .as_str()
        .unwrap()
        .contains("exponent letter"));

    let (report, code) = openkeito(&["validate", &example("rlc.json")]);
    assert_eq!(code, 0);
    assert_eq!(report["valid"], true);
    assert_eq!(report["steps"], 10000);
}

#[test]
fn validate_finds_a_measurement_of_a_current_the_component_does_not_have() {
    // Found without building the circuit: the same report as that of a run.
    let dir = scratch("validate_measurement");
    let mut model: Value =
        serde_json::from_str(&std::fs::read_to_string(example("three_phase_fault.json")).unwrap())
            .unwrap();
    model["version"] = "0.5".into();
    model["controls"] = serde_json::json!([
        {"type": "measurement", "name": "m",
         "quantity": {"type": "branch_current", "component": "LINE1", "phase": "a"}},
    ]);
    let path = dir.join("model.json");
    std::fs::write(&path, model.to_string()).unwrap();
    let (report, code) = openkeito(&["validate", path.to_str().unwrap()]);
    assert_eq!(code, 2);
    assert_eq!(report["valid"], false);
    // The report lists the warnings with the errors.
    let errors: Vec<&Value> = report["errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["severity"] == "error")
        .collect();
    assert_eq!(errors.len(), 1, "{report}");
    assert_eq!(errors[0]["code"], "E173");
    assert_eq!(errors[0]["path"], "controls[0].quantity");
    let hint = errors[0]["hint"].as_str().unwrap();
    assert!(
        hint.contains("I(LINE1.0.a)") && !hint.contains("  "),
        "{hint}"
    );
    let out = dir.join("out");
    let (summary, code) = openkeito(&[
        "run",
        path.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 2);
    assert_eq!(summary["errors"], report["errors"]);
}

#[test]
fn the_summary_counts_the_passes_of_closed_control_loops() {
    let dir = scratch("control_loops");
    let run = |name: &str| {
        let out = dir.join(name);
        let (summary, code) = openkeito(&[
            "run",
            &example(&format!("{name}.json")),
            "--out",
            out.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "{summary}");
        let solver = &summary["solver"];
        (
            solver["steps"].as_u64().unwrap(),
            solver["control_loop_iterations"].as_u64().unwrap(),
        )
    };
    // The loop of the PLL (phase detector, PI, integrator) settles in two
    // or three further passes at every grid point.
    let (steps, passes) = run("pll");
    assert!(
        passes >= steps && passes <= 4 * steps,
        "{passes} in {steps}"
    );
    // Blocks in a chain are evaluated once.
    assert_eq!(run("control_blocks").1, 0);
}

#[test]
fn a_failed_run_keeps_a_summary_with_the_cause() {
    let dir = scratch("failed");
    let model = dir.join("hanging.json");
    std::fs::write(
        &model,
        r#"{"format": "openkeito-model", "version": "0.1",
            "simulation": {"time_step_s": 1e-5, "end_time_s": 1e-3},
            "components": [
              {"type": "current_source", "name": "J1", "nodes": ["gnd", "n1"], "waveform": {"type": "dc", "value": 1.0}},
              {"type": "switch", "name": "SW1", "nodes": ["n1", "n2"], "initially_closed": true,
               "open_time_s": 1e-5, "open_at_current_zero": false},
              {"type": "resistor", "name": "R1", "nodes": ["n2", "gnd"], "resistance_ohm": 1.0}
            ]}"#,
    )
    .unwrap();
    let out = dir.join("out");
    let (summary, code) = openkeito(&[
        "run",
        model.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    // Validation only warns of a node that floats after an opening (W143);
    // one that floats from the start is rejected before the run (E143).
    assert_eq!(code, 1);
    assert_eq!(summary["status"], "failed");
    assert_eq!(summary["errors"][0]["code"], "E200");
    assert_eq!(summary["errors"][0]["path"], "V(n1)");
    assert_eq!(summary["warnings"][0]["code"], "W143");
    assert!(out.join("summary.json").exists());
}

#[test]
fn schema_and_components_are_machine_readable() {
    let (schema, code) = openkeito(&["schema"]);
    assert_eq!(code, 0);
    assert!(schema["properties"]["simulation"].is_object());
    let (components, code) = openkeito(&["components"]);
    assert_eq!(code, 0);
    let of = |category: &str| -> Vec<String> {
        components
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["category"] == category)
            .map(|c| c["type"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        of("circuit"),
        [
            "resistor",
            "inductor",
            "capacitor",
            "voltage_source",
            "current_source",
            "switch",
            "diode",
            "controlled_switch",
            "three_phase_source",
            "coupled_rl",
            "pi_line",
            "transformer",
            "three_phase_transformer",
            "fault",
            "three_phase_load",
            "three_phase_current_source",
            "three_phase_ideal_transformer",
            "nonlinear_resistor",
            "nonlinear_inductor",
            "bergeron_line",
            "controlled_voltage_source",
            "controlled_current_source",
            "three_phase_controlled_source",
            "three_phase_controlled_current_source",
            "synchronous_machine",
            "grid_following_converter"
        ]
    );
    assert_eq!(
        of("control"),
        [
            "constant",
            "gain",
            "sum",
            "triangle_wave",
            "comparator",
            "pi",
            "measurement",
            "transfer_function",
            "integrator",
            "lag",
            "lead_lag",
            "delay",
            "limiter",
            "rate_limiter",
            "dead_band",
            "piecewise_linear",
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
            "dq_limiter",
            "pi_dq"
        ]
    );
}

#[test]
fn a_switching_run_writes_all_events_to_a_csv_and_a_summary_of_them() {
    let dir = scratch("run_buck");
    let out = dir.join("out");
    let (summary, code) =
        openkeito(&["run", &example("buck.json"), "--out", out.to_str().unwrap()]);
    assert_eq!(code, 0, "{summary}");
    assert_eq!(summary["status"], "completed");
    assert_eq!(summary["warnings"].as_array().unwrap().len(), 0);
    // 200 periods: S1 and D1 change twice in each.
    let count = summary["event_count"].as_u64().unwrap();
    assert!(count >= 800, "{count}");
    assert_eq!(summary["events"].as_array().unwrap().len(), 100);
    assert_eq!(summary["events_truncated"], true);
    assert!(summary["event_counts"]["S1"]["close"].as_u64().unwrap() >= 200);
    assert!(summary["event_counts"]["D1"]["open"].as_u64().unwrap() >= 199);
    let solver = &summary["solver"];
    assert!(solver["resolves"].as_u64().unwrap() > 0);
    assert!(solver["max_resolves_per_step"].as_u64().unwrap() <= 2);
    assert_eq!(solver["cycle_fallbacks"], 0);
    for key in ["build_s", "factorize_s", "solve_s", "output_s", "run_s"] {
        assert!(summary["timing"][key].as_f64().unwrap() >= 0.0, "{key}");
    }
    let csv = std::fs::read_to_string(out.join("events.csv")).unwrap();
    let mut lines = csv.lines();
    assert_eq!(
        lines.next(),
        Some("time_s,component,action,interrupted_current_a")
    );
    assert_eq!(lines.count() as u64, count);
    assert_eq!(summary["result"]["columns"][4]["name"], "S(pwm)");
    assert_eq!(summary["result"]["columns"][4]["kind"], "signal");
}

#[test]
fn output_interval_steps_thins_the_rows() {
    let dir = scratch("run_buck_pi");
    let out = dir.join("out");
    let (summary, code) = openkeito(&[
        "run",
        &example("buck_pi.json"),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{summary}");
    assert_eq!(summary["simulation"]["output_interval_steps"], 10);
    // 1e5 steps, one row every 10 steps and the row at t = 0.
    assert_eq!(summary["result"]["rows"], 10001);
    let csv = std::fs::read_to_string(out.join("result.csv")).unwrap();
    let second: Vec<f64> = csv
        .lines()
        .nth(2)
        .unwrap()
        .split(',')
        .map(|s| s.parse().unwrap())
        .collect();
    assert!((second[0] - 1e-5).abs() < 1e-15);
    // The PI holds 30 V: the average over the last 20 ms.
    let tail: Vec<f64> = csv
        .lines()
        .skip(1)
        .map(|l| {
            l.split(',')
                .map(|s| s.parse::<f64>().unwrap())
                .collect::<Vec<_>>()
        })
        .filter(|v| v[0] >= 0.08 - 1e-12 && v[0] < 0.1 - 1e-12)
        .map(|v| v[1])
        .collect();
    let mean = tail.iter().sum::<f64>() / tail.len() as f64;
    assert!((mean - 30.0).abs() < 0.1, "{mean}");
}

#[test]
fn a_three_phase_run_names_the_phase_columns_and_fault_events() {
    let dir = scratch("run_three_phase");
    let out = dir.join("out");
    let (summary, code) = openkeito(&[
        "run",
        &example("three_phase_fault.json"),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{summary}");
    assert_eq!(summary["status"], "completed");
    let columns: Vec<&str> = summary["result"]["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        columns,
        [
            "V(b2.a)",
            "V(b2.b)",
            "V(b2.c)",
            "V(b3.a)",
            "V(b3.b)",
            "V(b3.c)",
            "I(F1.a)",
            "I(F1.g)",
            "I(T1.0.a)",
            "I(T1.0.b)",
            "I(T1.0.c)",
            "I(T1.1.a)",
            "I(T1.1.b)",
            "I(T1.1.c)"
        ]
    );
    let csv = std::fs::read_to_string(out.join("result.csv")).unwrap();
    assert!(csv.starts_with("time_s,V(b2.a),V(b2.b),V(b2.c),V(b3.a)"));
    // The fault phase closes at 0.1 s and opens at its first current zero
    // after the command at 0.2 s, within half a period.
    let events = summary["events"].as_array().unwrap();
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(
        (
            events[0]["component"].as_str(),
            events[0]["action"].as_str()
        ),
        (Some("F1.a"), Some("close"))
    );
    assert_eq!(events[0]["time_s"].as_f64(), Some(0.1));
    assert_eq!(events[1]["action"], "open");
    let opened = events[1]["time_s"].as_f64().unwrap();
    assert!(opened > 0.2 && opened < 0.211, "{opened}");
}

#[test]
fn a_run_from_the_steady_state_reports_it_and_writes_the_phasors() {
    let dir = scratch("run_steady_state");
    let out = dir.join("out");
    let (summary, code) = openkeito(&[
        "run",
        &example("three_phase_fault.json"),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{summary}");
    assert_eq!(summary["simulation"]["initialization"], "steady_state");
    let init = &summary["initialization"];
    assert_eq!(init["frequency_hz"], 50.0);
    // Compared until the fault starts at 0.1 s.
    assert_eq!(init["checked_until_s"], 0.1);
    assert!(init["relative_voltage_deviation"].as_f64().unwrap() < 1e-4);
    assert!(init["relative_current_deviation"].as_f64().unwrap() < 1e-4);
    let phasors = std::fs::read_to_string(out.join("phasors.csv")).unwrap();
    let mut lines = phasors.lines();
    assert_eq!(lines.next(), Some("quantity,unit,rms,angle_deg,real,imag"));
    assert!(phasors.contains("\nV(b2.a),V,"));
    assert!(phasors.contains("\nI(T1.0.a),A,"));
}

#[test]
fn scan_writes_the_impedances_over_frequency() {
    let dir = scratch("scan");
    let out = dir.join("out");
    let (summary, code) = openkeito(&[
        "scan",
        &example("three_phase_fault.json"),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{summary}");
    assert_eq!(summary["status"], "completed");
    assert_eq!(summary["command"], "scan");
    assert_eq!(summary["result"]["rows"], 250);
    assert_eq!(summary["frequency_scan"]["first_hz"], 10.0);
    assert_eq!(summary["frequency_scan"]["last_hz"], 2500.0);
    let columns: Vec<&str> = summary["result"]["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        columns[..4],
        ["Zmag(b2.a)", "Zdeg(b2.a)", "Zmag(b3.a)", "Zdeg(b3.a)"]
    );
    assert!(columns.contains(&"Z0mag(b2)"));
    assert_eq!(summary["result"]["columns"][0]["unit"], "ohm");
    assert_eq!(summary["result"]["columns"][1]["unit"], "deg");
    assert!(columns.contains(&"Zmag(b2.a;b3.a)"));
    let csv = std::fs::read_to_string(out.join("scan.csv")).unwrap();
    assert!(csv.starts_with("frequency_hz,Zmag(b2.a),Zdeg(b2.a),"));
    assert_eq!(csv.lines().count(), 251);
    // The header splits into as many fields as every row.
    let fields: Vec<usize> = csv.lines().map(|l| l.split(',').count()).collect();
    assert!(
        fields.iter().all(|&n| n == columns.len() + 1),
        "{:?}",
        &fields[..3]
    );

    // A model without frequency_scan cannot be scanned.
    let (summary, code) = openkeito(&[
        "scan",
        &example("rlc.json"),
        "--out",
        dir.join("none").to_str().unwrap(),
    ]);
    assert_eq!(code, 2);
    assert_eq!(summary["errors"][0]["code"], "E190");
}

/// A batch of three cases: a model file, the same model written out with
/// another end time, and a case whose model is invalid.
fn write_batch(dir: &Path) -> PathBuf {
    let rlc: Value =
        serde_json::from_str(&std::fs::read_to_string(example("rlc.json")).unwrap()).unwrap();
    let mut short = rlc.clone();
    short["simulation"]["end_time_s"] = serde_json::json!(0.005);
    let mut broken = rlc.clone();
    broken["components"][1]["resistance_ohm"] = serde_json::json!(-1.0);
    std::fs::copy(example("rlc.json"), dir.join("rlc.json")).unwrap();
    let batch = serde_json::json!({
        "format": "openkeito-batch", "version": "0.1", "name": "three cases",
        "cases": [
            {"name": "from_file", "model": "rlc.json"},
            {"name": "inline", "model": short},
            {"name": "broken", "model": broken}
        ]
    });
    let path = dir.join("batch.json");
    std::fs::write(&path, batch.to_string()).unwrap();
    path
}

#[test]
fn batch_runs_every_case_into_its_own_directory() {
    let dir = scratch("batch");
    let batch = write_batch(&dir);
    let out = dir.join("out");
    let (summary, code) = openkeito(&[
        "batch",
        batch.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--jobs",
        "2",
    ]);
    // One invalid case: the batch fails, the other cases complete.
    assert_eq!(code, 1, "{summary}");
    assert_eq!(summary["status"], "failed");
    assert_eq!(summary["jobs"], 2);
    assert_eq!(
        summary["counts"],
        serde_json::json!({"completed": 2, "failed": 0, "invalid": 1})
    );
    let cases = summary["cases"].as_array().unwrap();
    assert_eq!(cases[0]["name"], "from_file");
    assert_eq!(cases[0]["status"], "completed");
    assert_eq!(cases[2]["status"], "invalid");
    assert_eq!(cases[2]["errors"][0]["code"], "E130");
    for case in ["from_file", "inline"] {
        let s: Value = serde_json::from_str(
            &std::fs::read_to_string(out.join(case).join("summary.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(s["status"], "completed");
        let model_json: Value = serde_json::from_str(
            &std::fs::read_to_string(out.join(case).join("model.json")).unwrap(),
        )
        .unwrap();
        assert!(model_json["components"].is_array());
    }
    // The inline case ran to 5 ms at 1 us steps: 5001 rows.
    let inline: Value = serde_json::from_str(
        &std::fs::read_to_string(out.join("inline").join("summary.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(inline["result"]["rows"], 5001);
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("batch-summary.json")).unwrap())
            .unwrap();
    assert_eq!(on_disk, summary);
    // Metrics: one row per case and column; the invalid case has none.
    let metrics = std::fs::read_to_string(out.join("batch-metrics.csv")).unwrap();
    let mut lines = metrics.lines();
    assert_eq!(
        lines.next(),
        Some("case,status,column,unit,min,max,max_abs,last")
    );
    let rows: Vec<&str> = lines.collect();
    assert_eq!(rows.len(), 2 + 2 + 1, "{metrics}");
    assert!(rows[0].starts_with("from_file,completed,V(n3),V,"));
    assert_eq!(rows[4], "broken,invalid,,,,,,");
    // The capacitor of the RLC step response overshoots 1 V.
    let max: f64 = rows[0].split(',').nth(5).unwrap().parse().unwrap();
    assert!(max > 1.0 && max < 2.0, "{max}");

    // A second batch into the same directory is refused.
    let (again, code) = openkeito(&[
        "batch",
        batch.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 2);
    assert_eq!(again["errors"][0]["code"], "E300");
}

#[test]
fn parallel_and_sequential_batches_give_the_same_bits() {
    let dir = scratch("batch_parallel");
    let batch = write_batch(&dir);
    let run = |jobs: &str| {
        let out = dir.join(format!("jobs{jobs}"));
        let (summary, _) = openkeito(&[
            "batch",
            batch.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--jobs",
            jobs,
        ]);
        assert_eq!(summary["jobs"].as_u64().unwrap().to_string(), jobs);
        out
    };
    let (one, four) = (run("1"), run("4"));
    for case in ["from_file", "inline"] {
        for file in ["result.csv", "events.csv"] {
            let a = std::fs::read(one.join(case).join(file)).unwrap();
            let b = std::fs::read(four.join(case).join(file)).unwrap();
            assert!(a == b, "{case}/{file} differs");
        }
    }
    assert_eq!(
        std::fs::read(one.join("batch-metrics.csv")).unwrap(),
        std::fs::read(four.join("batch-metrics.csv")).unwrap()
    );
}

#[test]
fn a_batch_file_is_checked_and_described() {
    let dir = scratch("batch_invalid");
    let path = dir.join("batch.json");
    std::fs::write(
        &path,
        r#"{"format": "openkeito-batch", "version": "0.1", "cases": [
            {"name": "a b", "model": "x.json"}, {"name": "c", "model": "x.json"}, {"name": "c", "model": "x.json"}]}"#,
    )
    .unwrap();
    let (summary, code) = openkeito(&[
        "batch",
        path.to_str().unwrap(),
        "--out",
        dir.join("out").to_str().unwrap(),
    ]);
    assert_eq!(code, 2);
    let codes: Vec<&str> = summary["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["E402", "E402"]);
    let (schema, code) = openkeito(&["schema", "--batch"]);
    assert_eq!(code, 0);
    assert!(
        schema.to_string().contains("openkeito-batch") || schema["properties"]["cases"].is_object()
    );
}

#[test]
fn run_summaries_carry_column_statistics() {
    let dir = scratch("statistics");
    let out = dir.join("out");
    let (summary, code) = openkeito(&["run", &example("rlc.json"), "--out", out.to_str().unwrap()]);
    assert_eq!(code, 0);
    let stats = &summary["result"]["statistics"];
    assert_eq!(stats[0]["name"], "V(n3)");
    // The first row is t = 0+ (see docs/numerical-method.md).
    assert!(stats[0]["min"].as_f64().unwrap().abs() < 1e-12);
    let last = stats[0]["last"].as_f64().unwrap();
    assert!((last - 1.0).abs() < 0.05, "{last}");
}

#[test]
fn validate_checks_schedule_files() {
    let dir = scratch("schedule");
    let path = dir.join("day.json");
    let schedule = serde_json::json!({
        "format": "openkeito-schedule", "version": "0.1",
        "times": ["2026-11-21T00:00:00+09:00", "2026-11-21T01:00:00+09:00"],
        "series": [{"target": "gen/0", "quantity": "active_power_mw", "values": [250.0, 230.0]}]
    });
    std::fs::write(&path, schedule.to_string()).unwrap();
    let (report, code) = openkeito(&["validate", path.to_str().unwrap()]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["valid"], true);
    assert_eq!(
        (report["times"].as_u64(), report["series"].as_u64()),
        (Some(2), Some(1))
    );

    let mut bad = schedule.clone();
    bad["times"][1] = Value::from("2026-11-21T01:00:00");
    std::fs::write(&path, bad.to_string()).unwrap();
    let (report, code) = openkeito(&["validate", path.to_str().unwrap()]);
    assert_eq!(code, 2, "{report}");
    assert_eq!(report["errors"][0]["code"], "E502");
    assert_eq!(report["errors"][0]["path"], "times[1]");

    let (schema, code) = openkeito(&["schema", "--schedule"]);
    assert_eq!(code, 0);
    assert!(schema["properties"]["series"].is_object(), "{schema}");
}

fn file_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn force_replaces_the_whole_result_and_nothing_else() {
    let dir = scratch("force");
    let out = dir.join("out");
    let out_arg = out.to_str().unwrap();
    // A run from the steady state leaves phasors.csv.
    let (summary, code) = openkeito(&["run", &example("three_phase_fault.json"), "--out", out_arg]);
    assert_eq!(code, 0, "{summary}");
    assert!(out.join("phasors.csv").exists());
    std::fs::write(out.join("notes.txt"), "mine").unwrap();

    // Without --force nothing is touched.
    let (summary, code) = openkeito(&["run", &example("rlc.json"), "--out", out_arg]);
    assert_eq!(code, 2);
    assert_eq!(summary["errors"][0]["code"], "E300");
    assert_eq!(summary["errors"][0]["path"], out_arg);
    assert!(out.join("phasors.csv").exists());

    // With --force the old result goes, also the files the new one lacks.
    let (summary, code) = openkeito(&["run", &example("rlc.json"), "--out", out_arg, "--force"]);
    assert_eq!(code, 0, "{summary}");
    assert_eq!(
        file_names(&out),
        ["events.csv", "notes.txt", "result.csv", "summary.json"]
    );
    assert_eq!(
        std::fs::read_to_string(out.join("notes.txt")).unwrap(),
        "mine"
    );
}

#[test]
fn a_failed_run_reports_the_events_before_the_failure() {
    let dir = scratch("failed_events");
    let model = dir.join("opens.json");
    std::fs::write(
        &model,
        r#"{"format": "openkeito-model", "version": "0.1",
            "simulation": {"time_step_s": 1e-4, "end_time_s": 2e-3},
            "components": [
              {"type": "current_source", "name": "J1", "nodes": ["gnd", "n1"], "waveform": {"type": "dc", "value": 1.0}},
              {"type": "switch", "name": "SW1", "nodes": ["n1", "n2"], "initially_closed": true,
               "open_time_s": 5e-4, "open_at_current_zero": false},
              {"type": "resistor", "name": "R1", "nodes": ["n2", "gnd"], "resistance_ohm": 1.0}
            ]}"#,
    )
    .unwrap();
    let out = dir.join("out");
    let (summary, code) = openkeito(&[
        "run",
        model.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 1);
    assert_eq!(summary["status"], "failed");
    assert_eq!(summary["errors"][0]["code"], "E200");
    assert_eq!(summary["event_count"], 1);
    assert_eq!(summary["events"][0]["component"], "SW1");
    assert_eq!(summary["solver"]["steps"], 5);
    let events = std::fs::read_to_string(out.join("events.csv")).unwrap();
    assert_eq!(events.lines().count(), 2);
}

#[test]
fn a_mistake_in_a_written_out_model_stays_with_its_case() {
    let dir = scratch("batch_inline_mistake");
    let rlc: Value =
        serde_json::from_str(&std::fs::read_to_string(example("rlc.json")).unwrap()).unwrap();
    let mut misspelled = rlc.clone();
    misspelled["simulation"]["time_step"] = serde_json::json!(1e-6);
    let batch = serde_json::json!({
        "format": "openkeito-batch", "version": "0.1",
        "cases": [{"name": "good", "model": rlc}, {"name": "misspelled", "model": misspelled}]
    });
    let path = dir.join("batch.json");
    std::fs::write(&path, batch.to_string()).unwrap();
    let (path_arg, out) = (path.to_str().unwrap(), dir.join("out"));
    let out_arg = out.to_str().unwrap();
    let (summary, code) = openkeito(&["batch", path_arg, "--out", out_arg, "--jobs", "0"]);
    assert_eq!(code, 1, "{summary}");
    // --jobs 0 runs on one thread and says so.
    assert_eq!(summary["jobs"], 1);
    assert_eq!(summary["counts"]["completed"], 1);
    assert_eq!(summary["counts"]["invalid"], 1);
    let error = &summary["cases"][1]["errors"][0];
    assert_eq!(error["code"], "E001");
    assert!(
        error["message"].as_str().unwrap().contains("time_step"),
        "{error}"
    );

    // `validate` reads a batch file as one, with the models of its cases.
    let (report, code) = openkeito(&["validate", path_arg]);
    assert_eq!(code, 2);
    assert_eq!(report["valid"], false);
    assert_eq!(report["errors"][0]["code"], "E001");
    let at = report["errors"][0]["path"].as_str().unwrap();
    assert!(at.starts_with("case \"misspelled\": "), "{at}");
    let good = serde_json::json!({
        "format": "openkeito-batch", "version": "0.1", "name": "one",
        "cases": [{"name": "good", "model": rlc}]
    });
    std::fs::write(&path, good.to_string()).unwrap();
    let (report, code) = openkeito(&["validate", path_arg]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["valid"], true);
    assert_eq!(report["cases"], 1);
    assert_eq!(report["batch"]["name"], "one");

    // A second batch into the same directory needs --force.
    let (summary, code) = openkeito(&["batch", path_arg, "--out", out_arg]);
    assert_eq!(code, 2);
    assert_eq!(summary["errors"][0]["code"], "E300");
    let (summary, code) = openkeito(&["batch", path_arg, "--out", out_arg, "--force"]);
    assert_eq!(code, 0, "{summary}");
}

#[test]
fn case_names_that_would_share_a_directory_are_refused() {
    let dir = scratch("batch_names");
    let path = dir.join("batch.json");
    std::fs::write(
        &path,
        r#"{"format": "openkeito-batch", "version": "0.1", "cases": [
            {"name": "Case", "model": "x.json"}, {"name": "case", "model": "x.json"},
            {"name": "batch-summary.json", "model": "x.json"}, {"name": "ok", "model": 3}]}"#,
    )
    .unwrap();
    let (summary, code) = openkeito(&[
        "batch",
        path.to_str().unwrap(),
        "--out",
        dir.join("out").to_str().unwrap(),
    ]);
    assert_eq!(code, 2);
    let found: Vec<(&str, &str)> = summary["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["code"].as_str().unwrap(), e["path"].as_str().unwrap()))
        .collect();
    assert_eq!(
        found,
        [
            ("E403", "cases[3].model"),
            ("E402", "cases[1].name"),
            ("E402", "cases[2].name")
        ]
    );
    assert!(!dir.join("out").exists());
}

#[test]
fn a_step_source_is_an_event() {
    let dir = scratch("step_event");
    let model = dir.join("step.json");
    std::fs::write(
        &model,
        r#"{"format": "openkeito-model", "version": "0.1",
            "simulation": {"time_step_s": 1e-4, "end_time_s": 2e-3},
            "components": [
              {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
               "waveform": {"type": "step", "time_s": 1e-3, "before": 0.0, "after": 100.0}},
              {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 1.0}
            ],
            "outputs": [{"type": "node_voltage", "node": "n1"}]}"#,
    )
    .unwrap();
    let out = dir.join("out");
    let (summary, code) = openkeito(&[
        "run",
        model.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{summary}");
    assert_eq!(summary["events"][0]["action"], "step");
    assert_eq!(summary["events"][0]["component"], "E1");
    let csv = std::fs::read_to_string(out.join("result.csv")).unwrap();
    let value_at = |row: usize| -> f64 {
        let line = csv.lines().nth(row + 1).unwrap();
        line.split(',').nth(1).unwrap().parse().unwrap()
    };
    // The row at the time of the step still holds the value before it.
    assert_eq!((value_at(10), value_at(11)), (0.0, 100.0));
}

#[test]
fn the_summary_reports_the_parts_and_the_threads_of_a_divided_circuit() {
    let dir = scratch("line_areas");
    let run = |out: &str, more: &[&str]| {
        let out = dir.join(out);
        let mut args = vec!["run".to_string(), example("line_areas.json")];
        args.extend(["--out".to_string(), out.to_str().unwrap().to_string()]);
        args.extend(more.iter().map(|a| a.to_string()));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let (summary, code) = openkeito(&args);
        assert_eq!(code, 0, "{summary}");
        (summary, std::fs::read(out.join("result.csv")).unwrap())
    };
    // Three areas joined by two lines; the model asks for two threads, and
    // the solver takes one for parts this small.
    let (summary, rows) = run("model", &[]);
    assert_eq!(summary["simulation"]["partitioning"], "every_line");
    assert_eq!(summary["simulation"]["threads"], 2);
    assert_eq!(summary["solver"]["parts"], 3);
    assert_eq!(summary["solver"]["links"], 2);
    assert_eq!(summary["solver"]["threads"], 1);
    let unknowns = summary["solver"]["unknowns"].as_u64().unwrap();
    assert!(summary["solver"]["largest_part_unknowns"].as_u64().unwrap() < unknowns);
    assert_eq!(
        summary["warnings"].as_array().unwrap().len(),
        0,
        "{summary}"
    );
    // --threads replaces the number of the model; the rows do not change.
    let (summary, with_one) = run("one", &["--threads", "1"]);
    assert_eq!(summary["simulation"]["threads"], 1);
    assert!(rows == with_one);
    let (_, with_eight) = run("eight", &["--threads", "8"]);
    assert!(rows == with_eight);

    // Threads for a circuit that is one part: a warning that says why.
    let model = dir.join("one_part.json");
    let text = std::fs::read_to_string(example("three_phase_fault.json")).unwrap();
    let mut m: Value = serde_json::from_str(&text).unwrap();
    m["version"] = "0.5".into();
    m["simulation"]["threads"] = 4.into();
    std::fs::write(&model, m.to_string()).unwrap();
    let out = dir.join("one_part");
    let (summary, code) = openkeito(&[
        "run",
        model.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{summary}");
    assert_eq!(summary["solver"]["parts"], 1);
    assert_eq!(summary["solver"]["threads"], 1);
    let codes: Vec<&str> = summary["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"W301"), "{summary}");
    // threads: 0 is refused.
    m["simulation"]["threads"] = 0.into();
    std::fs::write(&model, m.to_string()).unwrap();
    let (report, code) = openkeito(&["validate", model.to_str().unwrap()]);
    assert_eq!(code, 2);
    assert_eq!(report["errors"][0]["code"], "E301");
}

/// A run in memory (for hosts without files, such as the browser) returns
/// the files of a run into a directory, byte for byte.
#[test]
fn a_run_in_memory_returns_the_text_of_the_files() {
    let dir = scratch("run_texts");
    // With events, with phasors (started from the steady state) and a scan.
    for name in ["buck.json", "three_phase_fault.json"] {
        let path = example(name);
        let (loaded, sha256) = openkeito_cli::load_file(Path::new(&path)).expect("valid");

        let out = dir.join(name);
        let (summary, code) = openkeito(&["run", &path, "--out", out.to_str().unwrap()]);
        assert_eq!(code, 0, "{summary}");
        let memory = openkeito_cli::run_texts(&path, &loaded, &sha256, None);
        assert_eq!(memory.code, 0);
        assert_eq!(memory.summary["result"]["csv_path"], Value::Null);
        assert_eq!(memory.summary["event_count"], summary["event_count"]);
        let mut expected = vec!["result.csv"];
        if summary["initialization"].is_object() {
            expected.push("phasors.csv");
        }
        expected.push("events.csv");
        let names: Vec<&str> = memory.texts.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, expected, "{name}");
        for (file, text) in &memory.texts {
            let written = std::fs::read_to_string(out.join(file)).unwrap();
            assert!(*text == written, "{name}: {file} differs");
        }

        if loaded.model.frequency_scan.is_some() {
            let out = dir.join(format!("{name}-scan"));
            let (summary, code) = openkeito(&["scan", &path, "--out", out.to_str().unwrap()]);
            assert_eq!(code, 0, "{summary}");
            let memory = openkeito_cli::scan_texts(&path, &loaded, &sha256);
            assert_eq!(memory.texts.len(), 1);
            assert_eq!(memory.texts[0].0, "scan.csv");
            let written = std::fs::read_to_string(out.join("scan.csv")).unwrap();
            assert!(memory.texts[0].1 == written, "{name}: scan.csv differs");
        }
    }
    // Into a directory, nothing is kept as text.
    let (loaded, sha256) = openkeito_cli::load_file(Path::new(&example("rlc.json"))).unwrap();
    let opts = openkeito_cli::RunOptions {
        out: Some(dir.join("files")),
        ..Default::default()
    };
    let output = openkeito_cli::run_loaded("rlc", &loaded, &sha256, &opts);
    assert!(output.texts.is_empty());
}

#[test]
fn validate_run_and_scan_warn_alike_of_a_scan_with_diodes() {
    let dir = scratch("same_warning");
    // A scan of a circuit with a diode: the scan takes the diode as off.
    // `validate` says so before the scan is run (W192), and `run` and `scan`
    // repeat the warning of the validation, once.
    let model = dir.join("diode_scan.json");
    let text = std::fs::read_to_string(example("rectifier.json")).unwrap();
    let mut m: Value = serde_json::from_str(&text).unwrap();
    m["version"] = "0.5".into();
    m["frequency_scan"] = serde_json::json!({"frequencies_hz": [50.0, 150.0], "nodes": ["p"]});
    std::fs::write(&model, m.to_string()).unwrap();
    let w192 = |document: &Value| -> Vec<Value> {
        document["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|w| w["code"] == "W192")
            .cloned()
            .collect()
    };
    let (report, code) = openkeito(&["validate", model.to_str().unwrap()]);
    assert_eq!(code, 0, "{report}");
    let of_validate = w192(&report);
    assert_eq!(of_validate.len(), 1, "{report}");
    for command in ["run", "scan"] {
        let out = dir.join(command);
        let (summary, code) = openkeito(&[
            command,
            model.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "{summary}");
        assert_eq!(summary["status"], "completed");
        assert_eq!(w192(&summary), of_validate, "{command}: {summary}");
    }
}

#[test]
fn validate_and_run_refuse_alike_an_impedance_that_cannot_be_solved() {
    let dir = scratch("same_error");
    // A series impedance that the stage coefficient at t = 0+ cannot
    // invert: `validate` and `run` give the same error (E139), and the run
    // does not start and stop with E204.
    let model = dir.join("uneven.json");
    let text = std::fs::read_to_string(example("three_phase_fault.json")).unwrap();
    let mut m: Value = serde_json::from_str(&text).unwrap();
    let source = m["components"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|c| c["type"] == "three_phase_source")
        .expect("the example has a source");
    source["impedance"] = serde_json::json!({"type": "sequence", "r1_ohm": 1.0, "l1_h": 1e-12,
                                             "r0_ohm": 1.0, "l0_h": 1e5});
    std::fs::write(&model, m.to_string()).unwrap();
    let (report, code) = openkeito(&["validate", model.to_str().unwrap()]);
    assert_eq!(code, 2, "{report}");
    assert_eq!(report["errors"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(report["errors"][0]["code"], "E139");
    let out = dir.join("uneven");
    let (summary, code) = openkeito(&[
        "run",
        model.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 2, "{summary}");
    assert_eq!(summary["errors"], report["errors"], "{summary}");
}

#[test]
fn a_model_that_is_not_valid_keeps_errors_and_warnings_apart() {
    let dir = scratch("errors_and_warnings");
    // A converter with controls that starts from the steady state (W183, a
    // warning) and an output of a component that does not exist (E151).
    let model = dir.join("both.json");
    let text = std::fs::read_to_string(example("converter.json")).unwrap();
    let mut m: Value = serde_json::from_str(&text).unwrap();
    m["outputs"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"type": "branch_current", "component": "NO_SUCH"}));
    std::fs::write(&model, m.to_string()).unwrap();
    let codes = |list: &Value| -> Vec<(String, String)> {
        list.as_array()
            .unwrap_or_else(|| panic!("a list: {list}"))
            .iter()
            .map(|d| {
                let text = |k: &str| d[k].as_str().unwrap().to_string();
                (text("code"), text("severity"))
            })
            .collect()
    };
    let check = |document: &Value| {
        let errors = codes(&document["errors"]);
        let warnings = codes(&document["warnings"]);
        assert!(!errors.is_empty(), "{document}");
        assert!(errors.iter().all(|(_, s)| s == "error"), "{document}");
        assert!(warnings.iter().all(|(_, s)| s == "warning"), "{document}");
        assert!(warnings.iter().any(|(c, _)| c == "W183"), "{document}");
        (errors, warnings)
    };
    // The warnings are those of the model without the mistake.
    let (valid, code) = openkeito(&["validate", &example("converter.json")]);
    assert_eq!(code, 0);
    let (report, code) = openkeito(&["validate", model.to_str().unwrap()]);
    assert_eq!(code, 2);
    assert_eq!(report["valid"], false);
    let of_validate = check(&report);
    assert_eq!(of_validate.1, codes(&valid["warnings"]));
    for command in ["run", "scan"] {
        let out = dir.join(command);
        let (summary, code) = openkeito(&[
            command,
            model.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ]);
        assert_eq!(code, 2, "{summary}");
        assert_eq!(summary["status"], "failed");
        let (errors, warnings) = check(&summary);
        if command == "run" {
            assert_eq!((errors, warnings), of_validate, "{summary}");
        }
    }
    // A batch: the case carries its errors, and counts its warnings.
    let batch = dir.join("batch.json");
    std::fs::write(
        &batch,
        serde_json::json!({"format": "openkeito-batch", "version": "0.1",
            "cases": [{"name": "both", "model": "both.json"}]})
        .to_string(),
    )
    .unwrap();
    let (report, code) = openkeito(&["validate", batch.to_str().unwrap()]);
    assert_eq!(code, 2, "{report}");
    check(&report);
    let (summary, _) = openkeito(&[
        "batch",
        batch.to_str().unwrap(),
        "--out",
        dir.join("batch").to_str().unwrap(),
    ]);
    let case = &summary["cases"][0];
    assert!(
        codes(&case["errors"]).iter().all(|(_, s)| s == "error"),
        "{summary}"
    );
    assert_eq!(case["warnings"], of_validate.1.len(), "{summary}");
}

#[test]
fn broken_pipe_on_stdout_exits_cleanly_without_panicking() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_openkeito"))
        .arg("components")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("openkeito starts");
    let mut stdout = child.stdout.take().unwrap();
    let mut buf = [0u8; 1];
    assert_eq!(stdout.read(&mut buf).unwrap(), 1);
    drop(stdout);
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn force_removes_stale_results_even_when_model_is_invalid() {
    let dir = scratch("force_invalid");
    let out = dir.join("out");
    let out_arg = out.to_str().unwrap();
    let (summary, code) = openkeito(&["run", &example("rlc.json"), "--out", out_arg]);
    assert_eq!(code, 0, "{summary}");
    std::fs::write(out.join("notes.txt"), "keep").unwrap();

    let bad = dir.join("bad.json");
    std::fs::write(&bad, r#"{"format": "openkeito-model"}"#).unwrap();

    // Without --force, existing results are untouched.
    let (_, code) = openkeito(&["run", bad.to_str().unwrap(), "--out", out_arg]);
    assert_eq!(code, 2);
    assert!(out.join("summary.json").exists());

    // With --force, stale result files are removed while other files stay.
    let (summary, code) = openkeito(&["run", bad.to_str().unwrap(), "--out", out_arg, "--force"]);
    assert_eq!(code, 2, "{summary}");
    assert_eq!(summary["status"], "failed");
    assert_eq!(file_names(&out), ["notes.txt"]);

    // Also when scan fails after loading (no frequency_scan -> E190).
    let scan_out = dir.join("scan_out");
    let scan_arg = scan_out.to_str().unwrap();
    let (_, code) = openkeito(&[
        "scan",
        &example("three_phase_fault.json"),
        "--out",
        scan_arg,
    ]);
    assert_eq!(code, 0);
    assert!(scan_out.join("summary.json").exists());
    assert!(scan_out.join("scan.csv").exists());
    let (summary, code) = openkeito(&["scan", &example("rlc.json"), "--out", scan_arg, "--force"]);
    assert_eq!(code, 2, "{summary}");
    assert_eq!(summary["errors"][0]["code"], "E190");
    assert!(!scan_out.join("summary.json").exists());
    assert!(!scan_out.join("scan.csv").exists());
}

#[test]
fn passing_batch_or_schedule_to_run_or_scan_gives_e300() {
    let dir = scratch("wrong_format");
    let batch = dir.join("batch.json");
    std::fs::write(
        &batch,
        r#"{"format": "openkeito-batch", "version": "0.1", "cases": []}"#,
    )
    .unwrap();
    let schedule = dir.join("schedule.json");
    std::fs::write(
        &schedule,
        r#"{"format": "openkeito-schedule", "version": "0.1", "times": [], "series": []}"#,
    )
    .unwrap();

    for (cmd, file, expected_hint) in [
        ("run", &batch, "openkeito batch"),
        ("scan", &batch, "openkeito batch"),
        ("run", &schedule, "openkeito validate"),
        ("scan", &schedule, "openkeito validate"),
    ] {
        let out = dir.join(format!(
            "{cmd}_{}",
            file.file_stem().unwrap().to_str().unwrap()
        ));
        let (summary, code) =
            openkeito(&[cmd, file.to_str().unwrap(), "--out", out.to_str().unwrap()]);
        assert_eq!(code, 2, "{summary}");
        assert_eq!(summary["errors"][0]["code"], "E300", "{summary}");
        assert!(
            summary["errors"][0]["hint"]
                .as_str()
                .unwrap()
                .contains(expected_hint),
            "{summary}"
        );
    }
}

#[test]
fn failed_batch_document_has_batch_shape_and_inline_catches_duplicate_keys() {
    let dir = scratch("batch_shape_dup");
    let empty_batch = dir.join("empty.json");
    std::fs::write(
        &empty_batch,
        r#"{"format": "openkeito-batch", "version": "0.1", "cases": []}"#,
    )
    .unwrap();
    let (summary, code) = openkeito(&[
        "batch",
        empty_batch.to_str().unwrap(),
        "--out",
        dir.join("out_empty").to_str().unwrap(),
    ]);
    assert_eq!(code, 2, "{summary}");
    assert_eq!(summary["command"], "batch");
    assert_eq!(summary["status"], "failed");
    assert_eq!(summary["batch"]["path"], empty_batch.to_str().unwrap());
    assert!(summary.get("model").is_none(), "{summary}");
    assert_eq!(summary["errors"][0]["code"], "E401");

    // Inline model with duplicate keys must fail with E001 just like a file model.
    let dup_batch = dir.join("dup.json");
    std::fs::write(
        &dup_batch,
        r#"{"format": "openkeito-batch", "version": "0.1", "cases": [{
            "name": "dup",
            "model": {
                "format": "openkeito-model", "version": "0.1",
                "simulation": {"time_step_s": 1e-5, "end_time_s": 1e-3, "end_time_s": 2e-3},
                "components": [
                    {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
                    {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 1.0}
                ]
            }
        }]}"#,
    )
    .unwrap();
    let (summary, code) = openkeito(&[
        "batch",
        dup_batch.to_str().unwrap(),
        "--out",
        dir.join("out_dup").to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "{summary}");
    assert_eq!(summary["cases"][0]["status"], "invalid");
    assert_eq!(summary["cases"][0]["errors"][0]["code"], "E001");
    assert!(summary["cases"][0]["errors"][0]["message"]
        .as_str()
        .unwrap()
        .contains("duplicate field"));
}

#[test]
fn topology_prints_the_connections_and_writes_them_with_out() {
    let (document, code) = openkeito(&["topology", &example("three_phase_fault.json")]);
    assert_eq!(code, 0, "{document}");
    assert_eq!(document["command"], "topology");
    assert_eq!(document["valid"], true);
    assert_eq!(document["buses"].as_array().unwrap().len(), 3);
    let text = std::fs::read_to_string(example("three_phase_fault.json")).unwrap();
    assert_eq!(document, openkeito_cli::topology_report(&text).0);

    let dir = scratch("topology");
    let out = dir.join("out");
    let args = [
        "topology",
        &example("rlc.json"),
        "--out",
        out.to_str().unwrap(),
    ];
    let (document, code) = openkeito(&args);
    assert_eq!(code, 0, "{document}");
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("topology.json")).unwrap()).unwrap();
    assert_eq!(on_disk, document);
    // The file is not overwritten unless asked.
    let (refused, code) = openkeito(&args);
    assert_eq!(code, 2);
    assert_eq!(refused["errors"][0]["code"], "E300");
    assert!(refused.get("components").is_none());
    let mut forced = args.to_vec();
    forced.push("--force");
    assert_eq!(openkeito(&forced), (document, 0));

    // A model that is not valid: exit code 2, and still its connections.
    let bad = dir.join("bad.json");
    std::fs::write(
        &bad,
        text.replace("\"length_km\": 50.0", "\"length_km\": -1.0"),
    )
    .unwrap();
    let (document, code) = openkeito(&["topology", bad.to_str().unwrap()]);
    assert_eq!(code, 2, "{document}");
    assert_eq!(document["valid"], false);
    assert_eq!(document["diagnostics"][0]["components"][0], "LINE1");
    let (missing, code) = openkeito(&["topology", dir.join("none.json").to_str().unwrap()]);
    assert_eq!(
        (code, &missing["errors"][0]["code"]),
        (2, &serde_json::json!("E300"))
    );
}

#[test]
fn validate_topology_and_run_support_v07_piecewise_linear_and_signal_limits() {
    let dir = scratch("v07_controls");
    let model_path = dir.join("v07.json");
    let out = dir.join("out");
    let model = serde_json::json!({
        "format": "openkeito-model",
        "version": "0.7",
        "simulation": {"time_step_s": 1e-4, "end_time_s": 2e-3},
        "components": [
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 1.0},
        ],
        "controls": [
            {"type": "ramp", "name": "u", "slope_per_s": 1000.0},
            {"type": "piecewise_linear", "name": "lut", "input": "u", "x": [0.0, 1.0, 2.0], "y": [0.0, 2.0, 1.0]},
            {"type": "constant", "name": "hi", "value": 1.5},
            {"type": "limiter", "name": "lim", "input": "lut", "maximum": "hi"},
            {"type": "lag", "name": "lag", "input": "lim", "gain": 1.0, "time_constant_s": 1e-3, "maximum": "hi", "initial_input": 0.0},
            {"type": "pi", "name": "pi", "input": "lim", "proportional_gain": 1.0, "integral_gain_per_s": 100.0, "output_max": "hi"},
        ],
        "outputs": [
            {"type": "signal", "signal": "lut"},
            {"type": "signal", "signal": "lim"},
            {"type": "signal", "signal": "lag"},
            {"type": "signal", "signal": "pi"},
        ],
    });
    std::fs::write(&model_path, serde_json::to_string_pretty(&model).unwrap()).unwrap();

    let (val, code) = openkeito(&["validate", model_path.to_str().unwrap()]);
    assert_eq!(code, 0, "{val}");
    assert_eq!(val["valid"], true);

    let (topo, code) = openkeito(&["topology", model_path.to_str().unwrap()]);
    assert_eq!(code, 0, "{topo}");
    assert_eq!(topo["valid"], true);

    let (summary, code) = openkeito(&[
        "run",
        model_path.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{summary}");
    assert_eq!(summary["status"], "completed");
    assert_eq!(summary["result"]["rows"], 21);
}

#[test]
fn initialization_exclude_checks_connectivity_and_runs_cleanly() {
    let dir = scratch("init_exclude");

    // 1) Excluding the only path to ground leaves an E183 connectivity error.
    let bad_path = dir.join("bad_exclude.json");
    let bad_model = serde_json::json!({
        "format": "openkeito-model",
        "version": "0.7",
        "simulation": {"time_step_s": 1e-4, "end_time_s": 2e-3, "initialization": "steady_state"},
        "components": [
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
             "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 10.0, "initialization": "exclude"},
            {"type": "resistor", "name": "R2", "nodes": ["n2", "n3"], "resistance_ohm": 10.0},
            {"type": "resistor", "name": "R3", "nodes": ["n3", "gnd"], "resistance_ohm": 10.0, "initialization": "exclude"}
        ]
    });
    std::fs::write(&bad_path, serde_json::to_string_pretty(&bad_model).unwrap()).unwrap();
    let (val_bad, code_bad) = openkeito(&["validate", bad_path.to_str().unwrap()]);
    assert_ne!(code_bad, 0);
    assert_eq!(val_bad["valid"], false);
    assert!(
        val_bad["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "E183"),
        "{val_bad}"
    );

    // 2) Valid "exclude" model passes validate and run cleanly, with 0 A in phasors.csv.
    let good_path = dir.join("good_exclude.json");
    let out = dir.join("out");
    let good_model = serde_json::json!({
        "format": "openkeito-model",
        "version": "0.7",
        "simulation": {"time_step_s": 1e-4, "end_time_s": 2e-3, "initialization": "steady_state"},
        "components": [
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
             "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0},
            {"type": "inductor", "name": "L_ex", "nodes": ["n1", "gnd"], "inductance_h": 0.05, "initialization": "exclude"}
        ],
        "outputs": [
            {"type": "branch_current", "component": "L_ex"}
        ]
    });
    std::fs::write(
        &good_path,
        serde_json::to_string_pretty(&good_model).unwrap(),
    )
    .unwrap();

    let (val_good, code_val) = openkeito(&["validate", good_path.to_str().unwrap()]);
    assert_eq!(code_val, 0, "{val_good}");
    assert_eq!(val_good["valid"], true);

    let (summary, code_run) = openkeito(&[
        "run",
        good_path.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code_run, 0, "{summary}");
    assert_eq!(summary["status"], "completed");
    let phasors = std::fs::read_to_string(out.join("phasors.csv")).unwrap();
    assert!(phasors.contains("I(L_ex),A,0e0,0e0,0e0,0e0"), "{phasors}");
}

#[test]
fn validate_warns_w185_on_excluded_series_component_but_not_grounded_shunt() {
    let dir = scratch("validate_w185");

    // Excluded series resistor between n1 and n2 -> valid=true, W185 warning.
    let series_path = dir.join("series_exclude.json");
    let series_model = serde_json::json!({
        "format": "openkeito-model",
        "version": "0.7",
        "simulation": {"time_step_s": 1e-4, "end_time_s": 1e-3, "initialization": "steady_state"},
        "components": [
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
             "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}},
            {"type": "voltage_source", "name": "E2", "nodes": ["n2", "gnd"],
             "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}},
            {"type": "resistor", "name": "R_ser", "nodes": ["n1", "n2"], "resistance_ohm": 10.0, "initialization": "exclude"}
        ]
    });
    std::fs::write(
        &series_path,
        serde_json::to_string_pretty(&series_model).unwrap(),
    )
    .unwrap();
    let (val_series, code_series) = openkeito(&["validate", series_path.to_str().unwrap()]);
    assert_eq!(code_series, 0, "{val_series}");
    assert_eq!(val_series["valid"], true);
    let warnings = val_series["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w["code"] == "W185" && w["path"] == "components[2]"),
        "{val_series}"
    );

    // Excluded grounded shunt inductor -> valid=true, no W185 warning.
    let shunt_path = dir.join("shunt_exclude.json");
    let shunt_model = serde_json::json!({
        "format": "openkeito-model",
        "version": "0.7",
        "simulation": {"time_step_s": 1e-4, "end_time_s": 1e-3, "initialization": "steady_state"},
        "components": [
            {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"],
             "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": 50.0}},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0},
            {"type": "inductor", "name": "L_shunt", "nodes": ["n1", "gnd"], "inductance_h": 0.05, "initialization": "exclude"}
        ]
    });
    std::fs::write(
        &shunt_path,
        serde_json::to_string_pretty(&shunt_model).unwrap(),
    )
    .unwrap();
    let (val_shunt, code_shunt) = openkeito(&["validate", shunt_path.to_str().unwrap()]);
    assert_eq!(code_shunt, 0, "{val_shunt}");
    assert_eq!(val_shunt["valid"], true);
    assert!(
        !val_shunt["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "W185"),
        "{val_shunt}"
    );
}
