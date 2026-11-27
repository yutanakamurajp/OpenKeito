"""Requirement files: a set of grid-connection tests run as a batch and
judged (openkeito.requirements, #55)."""

from __future__ import annotations

import json
import math
from pathlib import Path

import pytest

import openkeito
from openkeito.requirements import (Requirements, RequirementsError, load_requirements, run_suite,
                                    validate_requirements)

ROOT = Path(__file__).resolve().parents[2]
EXAMPLE = ROOT / "examples" / "grid_test" / "requirements.json"
DEVICE = ROOT / "examples" / "grid_test" / "converter_device.json"


def example(**changes) -> dict:
    doc = json.loads(EXAMPLE.read_text(encoding="utf-8"))
    doc["device"]["model"] = str(DEVICE)
    doc.update(changes)
    return doc


def codes(doc, base=None) -> list[str]:
    return [d["code"] for d in validate_requirements(doc, base)]


def test_the_example_runs_18_cases_and_the_table_does_not_depend_on_the_jobs(tmp_path):
    suites = {jobs: run_suite(EXAMPLE, jobs=jobs, out=tmp_path / f"jobs{jobs}") for jobs in (8, 1)}
    for name in ("suite.csv", "suite.json"):
        assert (tmp_path / "jobs8" / name).read_bytes() == (tmp_path / "jobs1" / name).read_bytes()
    suite = suites[8]
    assert suite.counts["cases"] == 18
    assert [c["name"] for c in suite.cases][:4] == ["scr10.fault", "scr10.jump", "scr10.dip", "scr5.fault"]
    assert all(c["status"] == "completed" for c in suite.cases)
    assert len(suite.rows) == 18 * 4
    assert (tmp_path / "jobs8" / "cases" / "scr1.2.dip" / "result.csv").exists()
    # At SCR 1.2 the converter cannot deliver 0.8 per unit with 10 % less voltage: the
    # active power does not return, and the reason says so with the source of the limit.
    failed = {c["name"]: c for c in suite.cases if not c["passed"]}
    assert list(failed) == ["scr1.2.dip"]
    (f,) = failed["scr1.2.dip"]["failures"]
    assert f["criterion"] == "p_recovery" and f["value"] == float("inf") and f["max"] == 0.5
    assert "user value" in f["source"] and "at the end of the run" in f["reason"]
    doc = json.loads((tmp_path / "jobs8" / "suite.json").read_text(encoding="utf-8"))
    assert doc["counts"] == {"cases": 18, "passed": 17, "failed": 1}
    assert doc["cases"][-1]["failures"][0]["value"] is None  # inf is not JSON
    header = (tmp_path / "jobs8" / "suite.csv").read_text(encoding="utf-8").splitlines()[0]
    assert header == "case,scr,disturbance,criterion,metric,quantity,value,min,max,result,reason,source"
    with pytest.raises(FileExistsError):
        run_suite(EXAMPLE, out=tmp_path / "jobs8")


def test_an_unstable_converter_fails_with_the_reasons():
    # A phase-locked loop of 80 Hz (default 20 Hz) is unstable behind SCR 1.2.
    device = json.loads(DEVICE.read_text(encoding="utf-8"))
    device["components"][0]["pll_natural_frequency_hz"] = 80.0
    doc = example(scr=[10, 1.2])
    doc["device"]["model"] = device
    suite = run_suite(doc)
    passed = {c["name"]: c["passed"] for c in suite.cases}
    assert passed == {"scr10.fault": True, "scr10.jump": True, "scr10.dip": True,
                      "scr1.2.fault": False, "scr1.2.jump": False, "scr1.2.dip": False}
    for case in suite.cases[3:]:
        stable = next(f for f in case["failures"] if f["criterion"] == "stable")
        assert stable["metric"] == "swing" and stable["value"] > 0.1 and stable["max"] == 0.01
        assert stable["source"].startswith("user value:")
        assert "swing of S(PV1.p) is" in stable["reason"] and "at most 0.01" in stable["reason"]


def test_a_time_step_with_w290_fails_every_case():
    doc = example(simulation={"end_time_s": 1.0, "time_step_s": 4e-4}, scr=[10, 1.2])
    suite = run_suite(doc)
    assert not any(c["passed"] for c in suite.cases)
    for case in suite.cases:
        assert "W290" in case["warnings"]
        assert any(f["criterion"] == "warning W290" and "time constant of the current control" in f["reason"]
                   for f in case["failures"])


def test_a_run_that_fails_fails_its_case():
    doc = example(scr=[3])
    doc["criteria"][0]["signal"] = "PV9.p"
    suite = run_suite(doc)
    for case in suite.cases:
        assert case["status"] == "failed" and not case["passed"]
        assert case["failures"][0]["criterion"] == "run"
        assert "did not complete" in case["failures"][0]["reason"]


def test_criteria_apply_to_their_disturbances_and_name_documents():
    doc = example(scr=[3])
    doc["criteria"] = [{"name": "rise", "metric": "rise_time", "signal": "PV1.q", "max": 1.0,
                        "disturbances": ["dip"],
                        "source": {"kind": "document", "document": "Test document", "edition": "test",
                                   "clause": "test clause"}},
                       {"name": "vmin", "metric": "minimum", "bus": "poi", "min": 0.1, "from": "start",
                        "disturbances": ["fault"], "source": {"kind": "user", "note": "test value"}}]
    suite = run_suite(doc)
    assert [(r["case"], r["criterion"]) for r in suite.rows] == [("scr3.fault", "vmin"), ("scr3.dip", "rise")]
    assert suite.rows[1]["source"] == "Test document, test, test clause"
    # The fault leaves 0.2 per unit with the grid alone; the converter holds a little more.
    assert 0.15 < suite.rows[0]["value"] < 0.3


def test_a_threshold_without_a_source_is_an_error():
    doc = example()
    del doc["criteria"][0]["source"]
    assert codes(doc) == ["E515"]
    for source in ({}, {"kind": "document", "document": "A", "edition": "", "clause": "1"},
                   {"kind": "document", "document": "A", "edition": "1"}, {"kind": "user", "note": " "},
                   {"kind": "grid code"}):
        doc["criteria"][0]["source"] = source
        assert "E515" in codes(doc), source
    with pytest.raises(RequirementsError) as e:
        load_requirements(doc)
    assert e.value.diagnostics[0]["code"] == "E515" and "ships no thresholds" in e.value.diagnostics[0]["hint"]


def test_other_errors_have_their_codes_and_hints(tmp_path):
    assert codes(example(format="openkeito-model")) == ["E510"]
    assert codes(example(extra=1)) == ["E511"]
    assert codes(example(scr=[3, 3.0])) == ["E511"]
    doc = example()
    doc["criteria"][0]["metric"] = "rms"
    assert codes(doc) == ["E513"]
    doc = example()
    doc["criteria"][1]["band"] = 0.1
    assert codes(doc) == ["E514"]
    doc = example()
    del doc["criteria"][1]["max"]
    assert codes(doc) == ["E514"]
    doc = example()
    doc["criteria"][1]["disturbances"] = ["nothing"]
    assert codes(doc) == ["E514"]
    doc = example()
    doc["disturbances"][1]["type"] = "phase_step"
    assert codes(doc) == ["E512"]
    doc = example()
    doc["disturbances"][0]["time_s"] = 0.01
    assert codes(doc) == ["E512"]
    assert codes(example(fail_on_warnings=["290"])) == ["E516"]
    doc = example(device={"model": "converter_device.json", "bus": "b1"})
    assert codes(doc, EXAMPLE.parent) == ["E517"]
    doc = example(device={"model": "missing.json", "bus": "poi"})
    assert codes(doc, tmp_path) == ["E517"]
    for d in validate_requirements(example(format="x", extra=1)):
        assert d["hint"] and d["path"] and d["severity"] == "error"


def test_a_bus_criterion_needs_the_disturbance_two_periods_after_the_start():
    # The voltage of a bus is the DFT over one period: it has values from the end of
    # the first period, so the mean over the period before the disturbance needs two.
    doc = example(scr=[10], simulation={"end_time_s": 0.5})
    doc["disturbances"] = [{"name": "dip", "type": "voltage_step", "time_s": 0.02, "change_pu": -0.1}]
    doc["criteria"] = [
        {"name": "v_settle", "metric": "settling_time", "bus": "poi", "tolerance": 0.01, "target": "pre",
         "from": "start", "max": 0.5, "source": {"kind": "user", "note": "test value"}},
        {"name": "v_rise", "metric": "rise_time", "bus": "poi", "max": 0.5, "source": {"kind": "user", "note": "t"}},
        {"name": "v_over", "metric": "overshoot", "bus": "poi", "max": 0.5, "source": {"kind": "user", "note": "t"}},
        {"name": "p_settle", "metric": "settling_time", "signal": "PV1.p", "tolerance": 0.05, "max": 0.5,
         "source": {"kind": "user", "note": "test value"}}]
    diags = validate_requirements(doc)
    assert [(d["code"], d["path"]) for d in diags] == [("E512", "disturbances[0].time_s")]
    assert "0.04 s or later" in diags[0]["hint"]
    # A signal alone is defined from the first row: one period is enough.
    assert codes(dict(doc, criteria=doc["criteria"][3:])) == []
    # A bus criterion that does not apply to the disturbance does not ask for it.
    other = {"name": "late", "type": "phase_jump", "time_s": 0.2, "angle_deg": 10.0}
    crits = [dict(cr, disturbances=["late"]) for cr in doc["criteria"][:3]]
    assert codes(dict(doc, disturbances=doc["disturbances"] + [other], criteria=crits)) == []
    # Judged without the validation, the criteria cannot be evaluated (no NaN, no
    # exception): the case fails with the reason.
    req = Requirements(doc=doc, device=json.loads(DEVICE.read_text(encoding="utf-8")))
    suite = run_suite(req)
    (case,) = suite.cases
    assert case["status"] == "completed" and not case["passed"]
    reasons = {f["criterion"]: f["reason"] for f in case["failures"]}
    assert sorted(reasons) == ["v_over", "v_rise", "v_settle"]
    for reason in reasons.values():
        assert reason.startswith("cannot be evaluated: the quantity has no finite values over the period")
    # From 0.04 s the bus criteria are evaluated. The voltage stays at 0.9 per unit, so it
    # does not settle back to the value before the step, 1.0 (not NaN).
    doc["disturbances"][0]["time_s"] = 0.04
    assert codes(doc) == []
    (case,) = run_suite(doc).cases
    values = case["values"]
    assert all(math.isfinite(values[k]) for k in ("v_rise", "v_over", "p_settle"))
    assert values["v_settle"] == math.inf
    (f,) = case["failures"]
    assert f["criterion"] == "v_settle" and "it is not within 0.01 of 1.0000" in f["reason"]


def test_thd_of_a_bus_and_names_that_the_device_does_not_have_are_errors():
    doc = example()
    doc["criteria"][3] = dict(doc["criteria"][3], bus="poi")
    del doc["criteria"][3]["column"]
    diags = validate_requirements(doc)
    assert [(d["code"], d["path"]) for d in diags] == [("E514", "criteria[3].bus")]
    assert '"column": "V(<bus>.a)"' in diags[0]["hint"]
    # The bus and the columns of a criterion are checked against the device model.
    doc = example()
    doc["criteria"][2]["bus"] = "b9"
    doc["criteria"][3]["column"] = "V(b8.a)"
    assert [(d["code"], d["path"]) for d in validate_requirements(doc, EXAMPLE.parent)] == [
        ("E517", "criteria[2].bus"), ("E517", "criteria[3].column")]
    doc["criteria"][2]["bus"] = "grid_src"  # added by attach
    doc["criteria"][3]["column"] = "I(PV9.a)"
    diags = validate_requirements(doc, EXAMPLE.parent)
    assert [(d["code"], d["path"]) for d in diags] == [("E517", "criteria[3].column")]
    assert "PV1" in diags[0]["hint"]
    for column in ("V(poi.b)", "I(grid_z.a)", "I(grid_fault.a)", "S(PV1.p)", "V(n3)"):
        doc["criteria"][3]["column"] = column
        assert codes(doc, EXAMPLE.parent) == [], column


def test_names_of_components_and_buses_may_hold_dots():
    # The catalog allows '.' in names: the converter inv.PV1 at the bus site.poi gives the
    # columns I(inv.PV1.a) and V(site.poi.a), which a criterion may name.
    text = DEVICE.read_text(encoding="utf-8").replace('"PV1', '"inv.PV1').replace('"poi"', '"site.poi"')
    device = json.loads(text)
    assert openkeito.validate(device)["errors"] == []
    doc = json.loads(EXAMPLE.read_text(encoding="utf-8").replace('"PV1', '"inv.PV1').replace("(PV1", "(inv.PV1")
                     .replace('"poi"', '"site.poi"'))
    doc["device"]["model"] = device
    assert doc["criteria"][3]["column"] == "I(inv.PV1.a)" and doc["device"]["bus"] == "site.poi"
    doc["criteria"].append({"name": "v_a", "metric": "maximum", "column": "V(site.poi.a)", "max": 1e4,
                            "source": {"kind": "user", "note": "test value"}})
    assert codes(doc, EXAMPLE.parent) == []
    # A part of a dotted name is not a component or a bus.
    for column in ("I(inv.a)", "I(inv)", "I(inv.PV9.a)", "V(site.a)", "V(poi.a)"):
        doc["criteria"][-1]["column"] = column
        assert [(d["code"], d["path"]) for d in validate_requirements(doc, EXAMPLE.parent)] == [
            ("E517", "criteria[4].column")], column
    doc["criteria"][-1]["column"] = "V(site.poi.a)"
    doc.update(scr=[10], simulation={"end_time_s": 0.4}, disturbances=doc["disturbances"][2:])
    (case,) = run_suite(doc).cases
    assert case["passed"] and math.isfinite(case["values"]["current_thd"])
    assert 5000 < case["values"]["v_a"] < 6000  # the peak of 6.6 kV / sqrt(3) and more


def test_node_voltage_columns_whose_node_ends_in_a_phase_letter_are_not_buses():
    # V(<node>) of node_voltage is not split at its last '.': the nodes n1.x.b (of a
    # two-terminal component) and n2.y.c (of a node_voltage output) are nodes, not buses.
    device = json.loads(DEVICE.read_text(encoding="utf-8"))
    device["components"].append({"type": "resistor", "name": "RX", "nodes": ["n1.x.b", "gnd"],
                                 "resistance_ohm": 1e6})
    device["outputs"] = device.get("outputs", []) + [{"type": "node_voltage", "node": "n2.y.c"}]
    doc = example()
    doc["device"]["model"] = device
    for column in ("V(n1.x.b)", "V(n2.y.c)"):
        doc["criteria"][3]["column"] = column
        assert codes(doc, EXAMPLE.parent) == [], column
    doc["criteria"][3]["column"] = "V(n9.x.b)"
    assert [(d["code"], d["path"]) for d in validate_requirements(doc, EXAMPLE.parent)] == [
        ("E517", "criteria[3].column")]


def test_criteria_without_pre_judge_a_bus_from_one_period_after_the_start():
    # Only settling_time (to "pre"), rise_time and overshoot use the mean over the period
    # before the disturbance; the other metrics need the voltage of a bus only after the
    # disturbance, and it has values from the end of the first period.
    doc = example(scr=[10], simulation={"end_time_s": 0.3})
    doc["disturbances"] = [{"name": "dip", "type": "voltage_step", "time_s": 0.02, "change_pu": -0.1}]
    src = {"kind": "user", "note": "test value"}
    doc["criteria"] = [
        {"name": "v_min", "metric": "minimum", "bus": "poi", "min": 0.85, "source": src},
        {"name": "v_max", "metric": "maximum", "bus": "poi", "max": 1.1, "source": src},
        {"name": "v_env", "metric": "envelope", "bus": "poi", "points": [[0.0, 0.8]], "max": 0.0, "source": src},
        {"name": "v_over", "metric": "overvoltage_duration", "bus": "poi", "threshold": 1.1, "max": 0.0,
         "source": src},
        {"name": "v_swing", "metric": "swing", "bus": "poi", "max": 0.01, "source": src},
        {"name": "v_settle", "metric": "settling_time", "bus": "poi", "target": 0.9, "tolerance": 0.01,
         "max": 0.2, "source": src}]
    assert codes(doc, EXAMPLE.parent) == []
    (case,) = run_suite(doc).cases
    assert case["passed"], case["failures"]
    values = case["values"]
    assert all(math.isfinite(v) for v in values.values())
    assert values["v_min"] == pytest.approx(0.9, abs=1e-3)  # 10 % below the voltage before the step
    assert values["v_max"] == pytest.approx(1.0, abs=1e-3)  # the voltage at the start of the step
    # settling_time to "pre" asks for the second period.
    doc["criteria"][-1]["target"] = "pre"
    assert [(d["code"], d["path"]) for d in validate_requirements(doc)] == [("E512", "disturbances[0].time_s")]
