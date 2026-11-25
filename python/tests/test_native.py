"""The Python package against the same expectations as the command line:
closed-form solutions, the documents of the command, and batches that give
the same bits in parallel and in sequence."""

from __future__ import annotations

import json
import math
from pathlib import Path

import numpy as np
import pytest

import openkeito

ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = ROOT / "examples"


def rlc_model(end_time_s: float = 0.01) -> dict:
    model = json.loads((EXAMPLES / "rlc.json").read_text(encoding="utf-8"))
    model["simulation"]["end_time_s"] = end_time_s
    return model


def test_version_matches_the_crates():
    cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    assert f'version = "{openkeito.__version__}"' in cargo


def test_run_matches_the_analytic_step_response():
    # Series RLC, 1 V step: v_C = 1 - e^{-a t} (cos wd t + a/wd sin wd t).
    result = openkeito.run(rlc_model())
    assert result.ok and result.status == "completed"
    assert result.columns == ["V(n3)", "I(L1)"]
    t = result.time
    assert len(t) == 10001 and t[-1] == pytest.approx(0.01)
    r, l, c = 10.0, 0.01, 1e-6
    a = r / (2 * l)
    wd = math.sqrt(1 / (l * c) - a * a)
    exact = 1 - np.exp(-a * t) * (np.cos(wd * t) + a / wd * np.sin(wd * t))
    # Second order: the error is about (w_d h)^2 = (1e4 rad/s * 1 us)^2 = 1e-4
    # of the 1 V step (measured 3.0e-5 over the 16 periods of the run).
    assert np.max(np.abs(result["V(n3)"] - exact)) < 1e-4
    # The summary is the document of `openkeito run`, without files.
    assert result.summary["result"]["csv_path"] is None
    assert result.summary["solver"]["linear_solver"] == "dense"
    stats = result.summary["result"]["statistics"][0]
    assert stats["name"] == "V(n3)" and stats["max"] == pytest.approx(result["V(n3)"].max())


def test_run_with_out_writes_the_files_of_the_command(tmp_path):
    out = tmp_path / "rlc"
    result = openkeito.run(EXAMPLES / "rlc.json", out)
    for name in ["result.csv", "events.csv", "summary.json"]:
        assert (out / name).exists(), name
    on_disk = json.loads((out / "summary.json").read_text(encoding="utf-8"))
    assert on_disk["result"]["rows"] == result.summary["result"]["rows"] == 10001
    # The CSV holds the same values as the arrays (shortest exact decimals).
    rows = (out / "result.csv").read_text(encoding="utf-8").splitlines()
    assert rows[0] == "time_s,V(n3),I(L1)"
    last = [float(v) for v in rows[-1].split(",")]
    assert last == [result.time[-1], result["V(n3)"][-1], result["I(L1)"][-1]]
    # A second run into the same directory is refused, as by the command.
    with pytest.raises(openkeito.OpenKeitoError) as e:
        openkeito.run(EXAMPLES / "rlc.json", out)
    assert e.value.diagnostics[0]["code"] == "E300"
    openkeito.run(EXAMPLES / "rlc.json", out, force=True)


def test_invalid_models_raise_with_diagnostics():
    model = rlc_model()
    model["components"][1]["resistance_ohm"] = -1
    report = openkeito.validate(model)
    assert report["valid"] is False
    assert report["errors"][0]["code"] == "E130"
    with pytest.raises(openkeito.OpenKeitoError) as e:
        openkeito.run(model)
    d = e.value.diagnostics[0]
    assert d["code"] == "E130" and d["path"] == "components[1].resistance_ohm" and d["hint"]
    failed = openkeito.run(model, check=False)
    assert failed.status == "failed" and failed.code == 2
    # A number without its exponent letter is a parse error with the fix.
    text = json.dumps(rlc_model()).replace("1e-06", "1.1141-6")
    report = openkeito.validate(text)
    assert report["errors"][0]["code"] == "E001"
    assert "1.1141e-6" in report["errors"][0]["hint"]


def test_scan_columns_and_values():
    result = openkeito.scan(EXAMPLES / "three_phase_fault.json")
    assert result.columns[:2] == ["Zmag(b2.a)", "Zdeg(b2.a)"]
    assert "Zmag(b2.a;b3.a)" in result
    assert len(result.frequency) == 250
    assert result.frequency[0] == pytest.approx(10.0) and result.frequency[-1] == pytest.approx(2500.0)
    assert np.all(result["Zmag(b2.a)"] > 0)


def test_steady_state_run_reports_the_phasors():
    result = openkeito.run(EXAMPLES / "three_phase_fault.json")
    init = result.summary["initialization"]
    assert init["frequency_hz"] == 50.0
    names = {p["name"] for p in init["phasors"]}
    assert {"V(b2.a)", "I(T1.0.a)"} <= names


def test_batch_runs_in_parallel_with_the_same_bits_as_in_sequence(tmp_path):
    cases = {f"rlc_{k}": rlc_model(0.002 + 0.001 * k) for k in range(6)}
    cases["fault"] = str(EXAMPLES / "three_phase_fault.json")
    one = openkeito.run_batch(cases, jobs=1, keep_rows=True)
    many = openkeito.run_batch(cases, tmp_path / "out", jobs=4, keep_rows=True)
    assert one.ok and many.ok
    assert one.summary["counts"] == {"completed": 7, "failed": 0, "invalid": 0}
    assert many.summary["jobs"] == 4
    for name in cases:
        a, b = one[name], many[name]
        assert np.array_equal(a.time, b.time)
        for column in a.columns:
            assert np.array_equal(a[column], b[column]), (name, column)
    # Files of the batch, as `openkeito batch`.
    out = tmp_path / "out"
    assert (out / "batch-summary.json").exists() and (out / "batch-metrics.csv").exists()
    assert (out / "rlc_3" / "result.csv").exists()


def measured_line_current(**choice):
    """The three-phase example with a measurement of a current of its line."""
    model = json.loads((EXAMPLES / "three_phase_fault.json").read_text(encoding="utf-8"))
    model["version"] = "0.5"
    quantity = {"type": "branch_current", "component": "LINE1", **choice}
    model["controls"] = [{"type": "measurement", "name": "m", "quantity": quantity}]
    return model


def test_validate_finds_a_measured_current_that_the_component_does_not_have():
    # A line has the currents of two terminals: found without building the circuit.
    report = openkeito.validate(measured_line_current(phase="a"))
    errors = report["errors"]
    assert report["valid"] is False and [d["code"] for d in errors] == ["E173"]
    assert errors[0]["path"] == "controls[0].quantity" and "I(LINE1.0.a)" in errors[0]["hint"]
    assert openkeito.validate(measured_line_current(terminal=0, phase="a"))["valid"] is True


def test_a_model_that_is_not_valid_keeps_errors_and_warnings_apart():
    # The example starts from the steady state with controls (W183, a warning). With a mistake
    # beside it, the warning stays in `warnings`: in the document of validate, in the summary of a
    # run and in the exception.
    valid = openkeito.validate(measured_line_current(terminal=0, phase="a"))
    assert [d["code"] for d in valid["warnings"]] == ["W183"]
    model = measured_line_current(phase="a")
    report = openkeito.validate(model)
    assert [(d["code"], d["severity"]) for d in report["errors"]] == [("E173", "error")]
    assert report["warnings"] == valid["warnings"]
    failed = openkeito.run(model, check=False)
    assert failed.status == "failed"
    assert [d["code"] for d in failed.summary["errors"]] == ["E173"]
    assert failed.warnings == valid["warnings"]
    with pytest.raises(openkeito.OpenKeitoError) as e:
        openkeito.run(model)
    assert [d["code"] for d in e.value.diagnostics] == ["E173"]
    assert e.value.warnings == valid["warnings"]
    assert "W183" not in str(e.value)


def test_batch_reports_invalid_cases_without_stopping_the_others():
    bad = rlc_model()
    bad["simulation"]["time_step_s"] = 0
    result = openkeito.run_batch({"good": rlc_model(0.001), "bad": bad}, jobs=2)
    assert not result.ok
    assert result.summary["counts"] == {"completed": 1, "failed": 0, "invalid": 1}
    assert result["good"].ok
    assert result["bad"].summary["errors"][0]["code"] == "E110"


def test_schema_and_components():
    s = openkeito.schema()
    assert "three_phase_source" in json.dumps(s)
    assert "cases" in json.dumps(openkeito.schema(batch=True))
    types = [c["type"] for c in openkeito.components()]
    assert "resistor" in types and "fault" in types


def test_to_pandas():
    pd = pytest.importorskip("pandas")
    frame = openkeito.run(rlc_model(0.001)).to_pandas()
    assert isinstance(frame, pd.DataFrame)
    assert list(frame.columns) == ["V(n3)", "I(L1)"]
    assert frame.index.name == "time_s"


def test_run_threads_argument_and_schedule_validate_hint():
    one = openkeito.run(EXAMPLES / "line_areas.json", threads=1)
    two = openkeito.run(EXAMPLES / "line_areas.json", threads=2)
    assert one.ok and two.ok
    for col in one.columns:
        assert np.array_equal(one[col], two[col])
    sched = '{"format": "openkeito-schedule", "version": "0.1", "times": [], "series": []}'
    report = openkeito.validate(sched)
    assert report["valid"] is False
    assert "validate_schedule" in report["errors"][0]["hint"]
