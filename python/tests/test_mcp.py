"""The MCP server: every tool through the protocol (in memory), and a session
over stdio with the server as a subprocess."""

from __future__ import annotations

import asyncio
import json
import pickle
import shutil
import sys
from pathlib import Path

import pytest

pytest.importorskip("mcp")
pytest.importorskip("pandapower")
pytest.importorskip("pypowsybl")

from mcp.client import Client  # noqa: E402
from mcp.client.stdio import StdioServerParameters  # noqa: E402

from openkeito.mcp_server import server  # noqa: E402

EXAMPLES = Path(__file__).resolve().parents[2] / "examples"
TOOLS = {"get_schema", "list_components", "validate_model", "get_topology", "run_model", "scan_frequency",
         "query_results", "convert_network", "make_schedule", "run_time_slices", "run_transitions"}


@pytest.fixture
def examples(tmp_path) -> Path:
    """The example models, copied under the root of the server (conftest.py:
    the temporary folder of the test), where the tools may read them."""
    return Path(shutil.copytree(EXAMPLES, tmp_path / "examples"))


def call(name: str, arguments: dict, target=None) -> tuple[bool, object]:
    """(is_error, the tool's JSON document or error text)."""

    async def go():
        async with Client(target if target is not None else server()) as c:
            return await c.call_tool(name, arguments)

    r = asyncio.run(go())
    text = r.content[0].text
    return r.is_error, (text if r.is_error else json.loads(text))


def test_the_tools_are_listed_with_their_instructions():
    async def go():
        async with Client(server()) as c:
            return await c.list_tools(), c.instructions

    tools, instructions = asyncio.run(go())
    assert {t.name for t in tools.tools} == TOOLS
    assert all(t.description for t in tools.tools)


def test_schema_components_and_validation():
    error, schema = call("get_schema", {"kind": "schedule"})
    assert not error and "series" in schema["properties"]
    error, catalog = call("list_components", {})
    assert "three_phase_ideal_transformer" in {c["type"] for c in catalog["components"]}
    model = json.loads((EXAMPLES / "rlc.json").read_text(encoding="utf-8"))
    next(c for c in model["components"] if c["type"] == "resistor")["resistance_ohm"] = -1.0
    error, report = call("validate_model", {"model": model})
    assert not error and report["valid"] is False
    assert report["errors"][0]["code"] == "E130" and report["errors"][0]["hint"]
    error, text = call("validate_model", {})
    assert error and "either model" in text
    # A measurement of a current that the component does not have (a line has two terminals).
    model = json.loads((EXAMPLES / "three_phase_fault.json").read_text(encoding="utf-8"))
    model["version"] = "0.5"
    quantity = {"type": "branch_current", "component": "LINE1", "phase": "a"}
    model["controls"] = [{"type": "measurement", "name": "m", "quantity": quantity}]
    error, report = call("validate_model", {"model": model})
    assert not error and report["valid"] is False
    assert "E173" in [d["code"] for d in report["errors"]]


def test_topology(examples):
    error, doc = call("get_topology", {"path": str(examples / "buck.json")})
    assert not error and doc["command"] == "topology" and doc["valid"] is True
    assert {c["name"] for c in doc["components"]} >= {"E1", "S1", "D1", "L1", "C1", "R1"}
    assert any(s["name"] == "pwm" for s in doc["signals"])
    model = json.loads((EXAMPLES / "rlc.json").read_text(encoding="utf-8"))
    next(c for c in model["components"] if c["type"] == "resistor")["resistance_ohm"] = -1.0
    error, bad = call("get_topology", {"model": model})
    assert not error and bad["valid"] is False and bad["errors"][0]["code"] == "E130"
    assert [c["name"] for c in bad["components"]] == ["E1", "R1", "L1", "C1"]
    error, text = call("get_topology", {})
    assert error and "either model" in text


def test_run_and_query(tmp_path, examples):
    out = tmp_path / "rlc"
    error, summary = call("run_model", {"path": str(examples / "rlc.json"), "out": str(out)})
    assert not error and summary["status"] == "completed"
    assert (out / "result.csv").exists()
    error, q = call("query_results", {"out": str(out), "columns": ["I(L1)"], "start": 0.0, "stop": 0.005,
                                      "max_rows": 50})
    assert not error and q["columns"] == ["I(L1)"] and q["axis"] == "time_s"
    assert len(q["rows"]) <= 51 and q["rows"][-1][0] <= 0.005
    assert q["statistics"]["I(L1)"]["max_abs"] > 0
    # The same directory again is refused (E300), as by the command.
    error, again = call("run_model", {"path": str(examples / "rlc.json"), "out": str(out)})
    assert not error and again["status"] != "completed"


def test_convert_schedule_and_time_slices(tmp_path):
    error, conv = call("convert_network", {"source": "pandapower", "network": "case9",
                                           "model_path": str(tmp_path / "case9.json"),
                                           "options": {"end_time_s": 0.02}})
    assert not error and conv["components"] > 0
    assert Path(conv["mapping_path"]).name == "case9.mapping.json"
    error, summary = call("run_model", {"path": conv["model_path"], "out": str(tmp_path / "case9")})
    assert summary["status"] == "completed"

    error, conv = call("convert_network", {"source": "powsybl", "network": "micro_grid_be",
                                           "model_path": str(tmp_path / "be.json")})
    assert not error and any("slack" in w for w in conv["warnings"])
    error, text = call("convert_network", {"source": "powsybl", "network": "four_substations_node_breaker_network",
                                           "model_path": str(tmp_path / "x.json")})
    assert error and "hvdc_lines" in text

    inputs = {"format": "openkeito-schedule", "version": "0.1",
              "times": ["2026-11-21T00:00:00+09:00", "2026-11-21T01:00:00+09:00"],
              "series": [{"target": "load/0", "quantity": "active_power_mw", "values": [70.0, 90.0]}]}
    (tmp_path / "inputs.json").write_text(json.dumps(inputs), encoding="utf-8")
    error, plan = call("make_schedule", {"network": "case9", "inputs_path": str(tmp_path / "inputs.json"),
                                         "schedule_path": str(tmp_path / "plan.json")})
    assert not error and plan["times"] == 2 and plan["series"] > 1
    error, run = call("run_time_slices", {
        "source": "pandapower", "network": "case9", "schedule_path": plan["schedule_path"],
        "out": str(tmp_path / "slices"), "jobs": 2, "options": {"end_time_s": 0.03},
        "disturbances": [{"name": "f7", "components": [
            {"type": "fault", "name": "F7", "bus": 7, "phases": ["a"], "ground": True,
             "start_time_s": 0.01, "clear_time_s": 0.02}]}]})
    assert not error and run["status"] == "completed"
    assert [c["case"] for c in run["cases"]] == ["t000.f7", "t001.f7"]
    assert all(c["max_abs_dv_pu"] < 1e-6 for c in run["cases"])
    assert (tmp_path / "slices" / "t001.f7" / "summary.json").exists()


def test_a_session_over_stdio(tmp_path, examples):
    # A name outside ASCII goes through whatever the encoding of the console is.
    out = tmp_path / "結果 é"
    params = StdioServerParameters(command=sys.executable, args=["-m", "openkeito.mcp_server", "--root", str(tmp_path)])
    error, summary = call("run_model", {"path": str(examples / "rlc.json"), "out": str(out)}, target=params)
    assert not error and summary["status"] == "completed"
    assert Path(summary["output_directory"]) == out.resolve() and (out / "summary.json").exists()


# What a client can get wrong, and how large an answer can be --------------------

INPUTS = {"format": "openkeito-schedule", "version": "0.1",
          "times": ["2026-11-21T00:00:00+09:00", "2026-11-21T01:00:00+09:00"],
          "series": [{"target": "load/0", "quantity": "active_power_mw", "values": [70.0, 90.0]}]}


class _Marker:
    """Unpickling this writes a file: the sign that a pickle was loaded."""

    def __init__(self, path: str):
        self.path = path

    def __reduce__(self):
        return (Path.write_text, (Path(self.path), "loaded"))


def test_pickle_files_are_not_loaded(tmp_path):
    marker = tmp_path / "marker.txt"
    for name in ("net.p", "net.pkl", "NET.PICKLE"):
        (tmp_path / name).write_bytes(pickle.dumps(_Marker(str(marker))))
        error, text = call("convert_network", {"source": "pandapower", "network": str(tmp_path / name),
                                               "model_path": str(tmp_path / "m.json")})
        assert error and "to_json" in text and name in text
        assert not marker.exists() and not (tmp_path / "m.json").exists()
    error, text = call("make_schedule", {"network": str(tmp_path / "net.p"), "inputs_path": "x.json",
                                         "schedule_path": str(tmp_path / "plan.json")})
    assert error and "to_json" in text and not marker.exists()


def test_a_pandapower_json_file_is_read_and_names_no_other_modules(tmp_path):
    import pandapower
    import pandapower.networks

    file = tmp_path / "Case9.JSON"
    pandapower.to_json(pandapower.networks.case9(), str(file))
    error, conv = call("convert_network", {"source": "pandapower", "network": str(file),
                                           "model_path": str(tmp_path / "m.json")})
    assert not error and conv["buses"] == 9

    # pandapower imports the modules that a file names; only those it writes are let through.
    (tmp_path / "okt_review_probe.py").write_text("X = 1\n", encoding="utf-8")
    data = json.loads(file.read_text(encoding="utf-8"))
    hidden = json.dumps({"_module": "okt_review_probe", "_class": "X", "_object": "1"})
    for label, value in (("plain", json.loads(hidden)), ("nested", json.dumps([hidden.replace("_m", "_\\u006d")]))):
        data["_object"]["user"] = value
        crafted = tmp_path / f"{label}.json"
        crafted.write_text(json.dumps(data), encoding="utf-8")
        sys.path.insert(0, str(tmp_path))
        try:
            error, text = call("convert_network", {"source": "pandapower", "network": str(crafted),
                                                   "model_path": str(tmp_path / "m2.json")})
        finally:
            sys.path.remove(str(tmp_path))
        assert error and "okt_review_probe" in text and crafted.name in text, label
        assert "okt_review_probe" not in sys.modules, label

    # S1: disallowed classes in otherwise allowed modules (builtins, pandas, numpy)
    # must be rejected before pandapower.from_json is reached.
    def fail_from_json(*args, **kwargs):
        raise AssertionError("pandapower.from_json should not be reached")

    for module, cls in (("builtins", "eval"), ("pandas", "read_pickle"), ("numpy", "load")):
        bad = json.loads(file.read_text(encoding="utf-8"))
        bad["_object"]["user"] = {"_module": module, "_class": cls, "_object": "x"}
        crafted = tmp_path / f"bad_{module}_{cls}.json"
        crafted.write_text(json.dumps(bad), encoding="utf-8")
        orig = pandapower.from_json
        pandapower.from_json = fail_from_json
        try:
            error, text = call("convert_network", {"source": "pandapower", "network": str(crafted),
                                                   "model_path": str(tmp_path / "m3.json")})
        finally:
            pandapower.from_json = orig
        assert error and f"{module}.{cls}" in text and crafted.name in text, (module, cls, text)


def test_existing_files_are_not_overwritten_without_force(tmp_path, examples):
    model, mapping = tmp_path / "case9.json", tmp_path / "case9.mapping.json"
    arguments = {"source": "pandapower", "network": "case9", "model_path": str(model)}
    model.write_text("precious", encoding="utf-8")
    error, text = call("convert_network", arguments)
    assert error and str(model) in text and "force" in text
    assert model.read_text(encoding="utf-8") == "precious" and not mapping.exists()
    model.unlink()
    mapping.write_text("precious", encoding="utf-8")
    error, text = call("convert_network", arguments)
    assert error and str(mapping) in text
    assert mapping.read_text(encoding="utf-8") == "precious" and not model.exists()
    error, conv = call("convert_network", {**arguments, "force": True})
    assert not error and json.loads(model.read_text(encoding="utf-8"))["format"] == "openkeito-model"
    assert "buses" in json.loads(mapping.read_text(encoding="utf-8"))

    inputs, plan = tmp_path / "inputs.json", tmp_path / "plan.json"
    inputs.write_text(json.dumps(INPUTS), encoding="utf-8")
    arguments = {"network": "case9", "inputs_path": str(inputs), "schedule_path": str(plan)}
    plan.write_text("precious", encoding="utf-8")
    error, text = call("make_schedule", arguments)
    assert error and str(plan) in text and "force" in text
    assert plan.read_text(encoding="utf-8") == "precious"
    error, made = call("make_schedule", {**arguments, "force": True})
    assert not error and made["times"] == 2

    # Runs: the native layer refuses, and the answer says why.
    out = tmp_path / "scan"
    error, summary = call("scan_frequency", {"path": str(examples / "three_phase_fault.json"), "out": str(out)})
    assert not error and summary["status"] == "completed"
    kept = (out / "scan.csv").read_bytes()
    error, summary = call("scan_frequency", {"path": str(examples / "three_phase_fault.json"), "out": str(out)})
    assert not error and summary["errors"][0]["code"] == "E300" and (out / "scan.csv").read_bytes() == kept
    error, summary = call("run_model", {"path": str(examples / "rlc.json"), "out": str(out)})
    assert not error and summary["errors"][0]["code"] == "E300" and not (out / "result.csv").exists()
    error, summary = call("scan_frequency", {"path": str(examples / "three_phase_fault.json"), "out": str(out),
                                             "force": True})
    assert not error and summary["status"] == "completed"

    arguments = {"source": "pandapower", "network": "case9", "schedule_path": str(plan),
                 "out": str(tmp_path / "slices"), "options": {"end_time_s": 0.005}}
    error, run = call("run_time_slices", arguments)
    assert not error and run["status"] == "completed"
    kept = (tmp_path / "slices" / "t000" / "result.csv").read_bytes()
    # A batch result is refused as a whole, before any case runs.
    error, text = call("run_time_slices", arguments)
    assert error and "E300" in text and "force" in text
    assert (tmp_path / "slices" / "t000" / "result.csv").read_bytes() == kept
    error, run = call("run_time_slices", {**arguments, "force": True})
    assert not error and run["status"] == "completed"


def test_network_names_are_examples_only(tmp_path):
    for name in ("os", "__dict__", "deepcopy", "from_json", "create_bus", "create_dickert_lv_feeders", "nope"):
        error, text = call("convert_network", {"source": "pandapower", "network": name,
                                               "model_path": str(tmp_path / "m.json")})
        assert error and repr(name) in text and "case9" in text, name
    error, text = call("convert_network", {"source": "pandapower", "network": "missing.json",
                                           "model_path": str(tmp_path / "m.json")})
    assert error and "missing.json" in text
    for name in ("load_bay", "__class__", "load", "nope"):
        error, text = call("convert_network", {"source": "powsybl", "network": name,
                                               "model_path": str(tmp_path / "m.json")})
        assert error and repr(name) in text and "ieee14" in text, name
    error, text = call("convert_network", {"source": "matpower", "network": "case9",
                                           "model_path": str(tmp_path / "m.json")})
    assert error and "'pandapower' or 'powsybl'" in text
    assert not (tmp_path / "m.json").exists()
    error, conv = call("convert_network", {"source": "powsybl", "network": "create_ieee14",
                                           "model_path": str(tmp_path / "ieee14.json")})
    assert not error and conv["buses"] == 14


def test_usage_mistakes_reach_the_client_with_their_reason(tmp_path, examples):
    missing = str(tmp_path / "nope.json")
    garbage = tmp_path / "garbage.json"
    garbage.write_text("{not json", encoding="utf-8")
    binary = tmp_path / "binary.json"
    binary.write_bytes(b"\xff\xfe\x00{")
    inputs = tmp_path / "inputs.json"
    inputs.write_text(json.dumps(INPUTS), encoding="utf-8")
    rlc = str(examples / "rlc.json")

    def message(name: str, arguments: dict) -> str:
        error, text = call(name, arguments)
        assert error, (name, arguments)
        return text

    # Model files.
    for name in ("validate_model", "get_topology", "run_model", "scan_frequency"):
        arguments = {} if name in ("validate_model", "get_topology") else {"out": str(tmp_path / "out")}
        assert missing in message(name, {**arguments, "path": missing})
        assert "UTF-8" in message(name, {**arguments, "path": str(binary)})
        assert str(tmp_path) in message(name, {**arguments, "path": str(tmp_path)})
        assert "not both" in message(name, {**arguments, "path": rlc, "model": {}})
    assert not (tmp_path / "out").exists()
    # A file that is not a model is a diagnostic, not a usage mistake.
    for name in ("validate_model", "get_topology"):
        error, report = call(name, {"path": str(garbage)})
        assert not error and report["errors"][0]["code"] == "E001"

    # Network files.
    convert = {"source": "pandapower", "model_path": str(tmp_path / "m.json")}
    assert "garbage.json" in message("convert_network", {**convert, "network": str(garbage)})
    assert "garbage.json" in message("convert_network", {**convert, "source": "powsybl", "network": str(garbage)})
    assert "unexpected keyword" in message("convert_network", {**convert, "network": "case9",
                                                               "options": {"nope": 1}})
    assert str(tmp_path) in message("convert_network", {"source": "pandapower", "network": "case9",
                                                        "model_path": str(tmp_path)})

    # Schedules.
    plan = {"network": "case9", "schedule_path": str(tmp_path / "plan.json")}
    assert missing in message("make_schedule", {**plan, "inputs_path": missing})
    text = message("make_schedule", {**plan, "inputs_path": str(garbage)})
    assert "garbage.json" in text and "JSON" in text
    assert "'ac' or 'dc'" in message("make_schedule", {**plan, "inputs_path": str(inputs), "opf": "x"})
    text = message("make_schedule", {**plan, "inputs_path": rlc})
    assert "rlc.json" in text and "E001" in text and not (tmp_path / "plan.json").exists()
    slices = {"source": "pandapower", "network": "case9", "out": str(tmp_path / "slices"),
              "options": {"end_time_s": 0.005}}
    assert missing in message("run_time_slices", {**slices, "schedule_path": missing})
    text = message("run_time_slices", {**slices, "schedule_path": str(garbage)})
    assert "garbage.json" in text and "JSON" in text
    assert "UTF-8" in message("run_time_slices", {**slices, "schedule_path": str(binary)})
    text = message("run_time_slices", {**slices, "schedule_path": str(inputs),
                                       "times": ["2026-11-21T05:00:00+09:00"]})
    assert "outside the schedule" in text
    assert "unexpected keyword" in message("run_time_slices", {**slices, "schedule_path": str(inputs),
                                                               "options": {"nope": 1}})
    assert "jobs" in message("run_time_slices", {**slices, "schedule_path": str(inputs), "jobs": -1})
    assert "jobs" in message("run_time_slices", {**slices, "schedule_path": str(inputs), "jobs": 0})
    assert "jobs" in message("run_transitions", {"source": "pandapower", "network": "case9",
                                                "schedule_path": str(inputs), "out": str(tmp_path / "tr_j0"),
                                                "jobs": 0})
    assert not (tmp_path / "slices").exists()

    # Results.
    assert "run_model" in message("query_results", {"out": missing})
    (tmp_path / "empty").mkdir()
    (tmp_path / "empty" / "result.csv").write_text("", encoding="utf-8")
    assert "result.csv" in message("query_results", {"out": str(tmp_path / "empty")})
    (tmp_path / "text").mkdir()
    (tmp_path / "text" / "result.csv").write_text("time_s,V(a)\n0.0,1.0\n0.1,abc\n", encoding="utf-8")
    assert "result.csv" in message("query_results", {"out": str(tmp_path / "text")})
    text = message("query_results", {"out": str(tmp_path / "text"), "columns": ["V(b)"]})
    assert "V(b)" in text and "V(a)" in text
    assert "max_rows" in message("query_results", {"out": str(tmp_path / "text"), "max_rows": 0})
    assert "'model', 'batch' or 'schedule'" in message("get_schema", {"kind": "x"})


def _write_result(directory: Path, columns: int, rows: int) -> list[str]:
    """A result.csv with the value row * columns + column in every cell, 1 ms apart."""
    names = [f"V(n{k})" for k in range(columns)]
    directory.mkdir()
    with (directory / "result.csv").open("w", encoding="utf-8", newline="") as f:
        f.write(",".join(["time_s"] + names) + "\n")
        for r in range(rows):
            f.write(",".join([repr(r * 1e-3)] + [repr(float(r * columns + k)) for k in range(columns)]) + "\n")
    return names


def test_query_results_are_bounded(tmp_path):
    names = _write_result(tmp_path / "wide", 300, 2001)
    out = str(tmp_path / "wide")
    last = 2000 * 1e-3

    # The defaults, and a huge max_rows, stay within the limit and say so.
    for arguments in ({}, {"max_rows": 1000000}):
        error, q = call("query_results", {"out": out, **arguments})
        assert not error and 0 < sum(len(r) for r in q["rows"]) <= 20000
        assert q["truncated"] is True and q["rows_in_range"] == 2001 and q["rows_returned"] == len(q["rows"])
        assert "columns" in q["note"] and "result.csv" in q["note"]
        assert q["rows"][0][0] == 0.0 and q["rows"][-1][0] == last
        assert q["rows"][1][0] == q["every"] * 1e-3
        assert q["statistics"]["V(n299)"]["last"] == 2000 * 300 + 299.0
        assert len(json.dumps(q)) < 600_000
    # Never more rows than max_rows, the last row included.
    for limit in (1, 2, 3, 7, 500):
        error, q = call("query_results", {"out": out, "columns": names[:2], "max_rows": limit})
        assert not error and len(q["rows"]) <= limit and q["rows"][-1][0] == last, limit
        assert q["truncated"] is True and q["columns_total"] == 300
    # A small selection is exact.
    error, q = call("query_results", {"out": out, "columns": ["V(n1)", "V(n0)"], "start": 0.4995, "stop": 0.6005,
                                      "max_rows": 1000})
    assert not error and q["truncated"] is False and "note" not in q and q["every"] == 1
    assert [r[0] for r in q["rows"]] == [r * 1e-3 for r in range(500, 601)]
    assert q["rows"][0] == [0.5, 500 * 300 + 1.0, 500 * 300.0]
    assert q["statistics"]["V(n0)"] == {"min": 150000.0, "max": 180000.0, "max_abs": 180000.0, "last": 180000.0}
    error, q = call("query_results", {"out": out, "columns": ["V(n0)"], "start": 5.0})
    assert not error and q["rows"] == [] and q["rows_in_range"] == 0 and q["truncated"] is False

    # More columns than one answer holds: the first ones by default; a longer selection is refused.
    names = _write_result(tmp_path / "wider", 700, 3)
    error, q = call("query_results", {"out": str(tmp_path / "wider")})
    assert not error and q["columns"] == names[:500] and q["columns_total"] == 700 and q["truncated"] is True
    assert len(q["statistics"]) == 500 and len(q["rows"]) == 3 and "500" in q["note"]
    error, text = call("query_results", {"out": str(tmp_path / "wider"), "columns": names})
    assert error and "500" in text


def test_the_summary_of_a_large_model_is_trimmed(tmp_path, examples):
    error, conv = call("convert_network", {"source": "pandapower", "network": "case39",
                                           "model_path": str(tmp_path / "case39.json"),
                                           "options": {"end_time_s": 0.005}})
    assert not error
    out = tmp_path / "case39"
    error, summary = call("run_model", {"path": conv["model_path"], "out": str(out)})
    assert not error and summary["status"] == "completed"
    full = json.loads((out / "summary.json").read_text(encoding="utf-8"))
    count = len(full["result"]["columns"])
    assert count > 50
    assert summary["result"]["column_count"] == count
    assert summary["result"]["column_names"] == [c["name"] for c in full["result"]["columns"]]
    assert "columns" not in summary["result"] and "statistics" not in summary["result"]
    assert "phasors" not in summary["initialization"]
    assert summary["initialization"]["phasor_count"] == len(full["initialization"]["phasors"])
    assert set(summary["omitted"]) == {"result.columns", "result.statistics", "initialization.phasors"}
    assert Path(summary["summary_path"]) == out / "summary.json" and "query_results" in summary["note"]
    assert len(json.dumps(summary)) < len(json.dumps(full)) / 4
    # A small model's summary is whole.
    error, summary = call("run_model", {"path": str(examples / "rlc.json"), "out": str(tmp_path / "rlc")})
    full = json.loads((tmp_path / "rlc" / "summary.json").read_text(encoding="utf-8"))
    assert "omitted" not in summary and summary["result"] == full["result"]


def test_many_time_slices_are_listed_in_part(tmp_path, monkeypatch):
    import openkeito.mcp_server as module

    monkeypatch.setattr(module, "MAX_CASES", 3)
    (tmp_path / "inputs.json").write_text(json.dumps(INPUTS), encoding="utf-8")
    fault = {"type": "fault", "name": "F", "bus": 7, "phases": ["a"], "ground": True, "start_time_s": 0.002}
    disturbances = [{"name": "ok1", "components": [fault]},
                    {"name": "bad", "components": [{"type": "no_such_component", "name": "X"}]},
                    {"name": "ok2", "components": [fault]}]
    error, run = call("run_time_slices", {
        "source": "pandapower", "network": "case9", "schedule_path": str(tmp_path / "inputs.json"),
        "out": str(tmp_path / "slices"), "options": {"end_time_s": 0.005}, "disturbances": disturbances})
    assert not error and run["cases_total"] == 6 and run["cases_truncated"] is True
    # The cases that did not complete come first, each with its diagnostics.
    assert [c["case"] for c in run["cases"]] == ["t000.bad", "t001.bad", "t000.ok1"]
    assert run["cases"][0]["errors"][0]["hint"]
    assert "batch-summary.json" in run["note"]


def test_warnings_are_included_and_truncation_reported(tmp_path, monkeypatch):
    import openkeito.mcp_server as module

    monkeypatch.setattr(module, "MAX_WARNINGS", 2)
    (tmp_path / "inputs.json").write_text(json.dumps(INPUTS), encoding="utf-8")
    error, run = call("run_time_slices", {
        "source": "pandapower", "network": "case9", "schedule_path": str(tmp_path / "inputs.json"),
        "out": str(tmp_path / "slices_w"), "options": {"end_time_s": 0.005, "generators": "machine"}})
    assert not error and run["status"] == "completed"
    assert len(run["warnings"]) == 2
    assert run["warnings_truncated"] is True and run["warnings_total"] > 2

    error, tr_run = call("run_transitions", {
        "source": "pandapower", "network": "case9", "schedule_path": str(tmp_path / "inputs.json"),
        "out": str(tmp_path / "tr_w"), "options": {"start_time_s": 0.05, "ramp_time_s": 0.05, "settle_time_s": 0.05}})
    assert not error and tr_run["status"] == "completed"
    assert len(tr_run["warnings"]) == 2
    assert tr_run["warnings_truncated"] is True and tr_run["warnings_total"] > 2
