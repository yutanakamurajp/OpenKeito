"""The MCP tools read the short-circuit attributes of a CGMES file as
``from_cgmes`` does (see test_powsybl_short_circuit.py for the data and what
the converter makes of them)."""

from __future__ import annotations

import json
import zipfile

import pytest

pytest.importorskip("mcp")
pytest.importorskip("pypowsybl")

import pypowsybl.network as pn  # noqa: E402

from openkeito.powsybl import from_cgmes, from_powsybl  # noqa: E402
from openkeito.schedule import Schedule  # noqa: E402

from test_mcp import call  # noqa: E402
from test_powsybl_short_circuit import cgmes_with_short_circuit_attributes  # noqa: E402

TIMES = ["2026-01-01T00:00:00+09:00", "2026-01-01T01:00:00+09:00"]
LEFT_OUT = "short-circuit attributes of the CGMES file left out"


def with_a_zigzag_winding(path, tmp_path):
    """The file with its delta windings as grounded zigzag ones, which the converter
    does not represent."""
    other = tmp_path / "be_zigzag.zip"
    with zipfile.ZipFile(path) as archive, zipfile.ZipFile(other, "w", zipfile.ZIP_DEFLATED) as out:
        for name in archive.namelist():
            text = archive.read(name).decode("utf-8")
            out.writestr(name, text.replace("WindingConnection.D\"", "WindingConnection.Zn\""))
    return other


def load_plan(net, tmp_path):
    """Two times with the first load at its power and 2 % above."""
    loads = net.get_loads()
    lid, p = loads.index[0], float(loads["p0"].iloc[0])
    plan = Schedule.from_values(TIMES, {(lid, "active_power_mw"): [p, 1.02 * p]})
    plan.save(tmp_path / "plan.json")
    return str(tmp_path / "plan.json")


def test_convert_network_reads_the_short_circuit_attributes_of_a_cgmes_file(tmp_path):
    path = cgmes_with_short_circuit_attributes(tmp_path)
    arguments = {"source": "powsybl", "network": str(path), "model_path": str(tmp_path / "be.json")}
    error, conv = call("convert_network", arguments)
    assert not error, conv
    direct = from_cgmes(path)
    mapping = json.loads((tmp_path / "be.mapping.json").read_text(encoding="utf-8"))
    used = mapping["short_circuit"]
    assert used and used == json.loads(json.dumps(direct.mapping["short_circuit"]))
    assert any("transformer" in key.split("/")[0] for key in used)
    assert {kind.split("/")[0] for kind in used} >= {"generator", "line"}
    assert all(entry["sources"] == ["CGMES"] for entry in used.values())
    assert json.loads((tmp_path / "be.json").read_text(encoding="utf-8")) == json.loads(json.dumps(direct.model))
    assert conv["warnings"] == direct.warnings
    assert not any(w.startswith(("no zero-sequence data", "transformer connections unknown")) for w in conv["warnings"])

    # Data of the caller win over the file and are named.
    lid = next(key for key in used if key.startswith("line/")).split("/", 1)[1]
    error, conv = call("convert_network", {**arguments, "force": True,
                                           "options": {"short_circuit_data": {lid: {"rz": 7.0}}}})
    assert not error, conv
    entry = json.loads((tmp_path / "be.mapping.json").read_text(encoding="utf-8"))["short_circuit"][f"line/{lid}"]
    assert entry["sources"] == ["CGMES", "short_circuit_data"] and entry["data"]["rz"] == 7.0
    assert entry["data"]["xz"] == used[f"line/{lid}"]["data"]["xz"]

    # Turned off, the file converts with the assumptions, as before.
    error, conv = call("convert_network", {**arguments, "force": True, "short_circuit_profile": False})
    assert not error, conv
    plain = from_powsybl(pn.load(str(path)))
    assert json.loads((tmp_path / "be.json").read_text(encoding="utf-8")) == json.loads(json.dumps(plain.model))
    assert json.loads((tmp_path / "be.mapping.json").read_text(encoding="utf-8"))["short_circuit"] == {}
    assert conv["warnings"] == plain.warnings
    assert any(w.startswith("transformer connections unknown") for w in conv["warnings"])


def test_attributes_that_are_not_used_are_named_in_the_warnings(tmp_path):
    path = with_a_zigzag_winding(cgmes_with_short_circuit_attributes(tmp_path), tmp_path)
    error, conv = call("convert_network", {"source": "powsybl", "network": str(path),
                                           "model_path": str(tmp_path / "z.json")})
    assert not error, conv
    note = next(w for w in conv["warnings"] if w.startswith(LEFT_OUT))
    assert "winding connection Zn is not represented" in note

    plan = load_plan(pn.load(str(path)), tmp_path)
    slices = {"source": "powsybl", "network": str(path), "schedule_path": plan, "out": str(tmp_path / "slices"),
              "times": TIMES[:1], "options": {"end_time_s": 0.02}}
    error, run = call("run_time_slices", slices)
    assert not error and run["status"] == "completed", run
    assert run["warnings"][0] == note and run["cases"][0]["max_abs_dv_pu"] < 1e-9
    error, run = call("run_time_slices", {**slices, "out": str(tmp_path / "plain"), "short_circuit_profile": False})
    assert not error and run["status"] == "completed"
    assert not any(w.startswith(LEFT_OUT) for w in run.get("warnings", []))

    moves = {"source": "powsybl", "network": str(path), "schedule_path": plan, "out": str(tmp_path / "moves"),
             "options": {"start_time_s": 0.05, "ramp_time_s": 0.05, "settle_time_s": 0.05, "generators": "source"}}
    error, run = call("run_transitions", moves)
    assert not error and run["status"] == "completed", run
    assert run["warnings"][0] == note
    error, run = call("run_transitions", {**moves, "out": str(tmp_path / "moves_plain"),
                                          "short_circuit_profile": False})
    assert not error and run["status"] == "completed"
    assert not any(w.startswith(LEFT_OUT) for w in run["warnings"])


def test_the_time_slices_of_a_cgmes_file_use_its_short_circuit_attributes(tmp_path, monkeypatch):
    import openkeito.powsybl as module

    path = cgmes_with_short_circuit_attributes(tmp_path)
    seen = []
    convert = module.from_powsybl

    def spy(network, **options):
        seen.append(options.get("short_circuit_data"))
        return convert(network, **options)

    monkeypatch.setattr(module, "from_powsybl", spy)
    plan = load_plan(pn.load(str(path)), tmp_path)
    arguments = {"source": "powsybl", "network": str(path), "schedule_path": plan, "times": TIMES[:1],
                 "options": {"end_time_s": 0.02}}
    error, run = call("run_time_slices", {**arguments, "out": str(tmp_path / "with")})
    assert not error and run["status"] == "completed"
    assert not any(w.startswith(LEFT_OUT) for w in run.get("warnings", [])), run
    assert len(seen) == 1 and seen[0] and any("direct_sub_trans_x" in row for row in seen[0].values())
    error, run = call("run_time_slices", {**arguments, "out": str(tmp_path / "without"),
                                          "short_circuit_profile": False})
    assert not error and run["status"] == "completed"
    assert seen[1] is None


def test_networks_without_the_attributes_convert_as_before(tmp_path):
    # An example by name, and a file that is not CGMES.
    error, conv = call("convert_network", {"source": "powsybl", "network": "micro_grid_be",
                                           "model_path": str(tmp_path / "be.json")})
    assert not error, conv
    plain = from_powsybl(pn.create_micro_grid_be_network())
    assert json.loads((tmp_path / "be.json").read_text(encoding="utf-8")) == json.loads(json.dumps(plain.model))
    assert conv["warnings"] == plain.warnings

    net = pn.create_eurostag_tutorial_example1_network()
    net.save(str(tmp_path / "eurostag.xiidm"), format="XIIDM")
    plain = from_powsybl(pn.load(str(tmp_path / "eurostag.xiidm")))
    for name, more in (("on", {}), ("off", {"short_circuit_profile": False})):
        error, conv = call("convert_network", {"source": "powsybl", "network": str(tmp_path / "eurostag.xiidm"),
                                               "model_path": str(tmp_path / f"{name}.json"), **more})
        assert not error, conv
        assert json.loads((tmp_path / f"{name}.json").read_text(encoding="utf-8")) == json.loads(
            json.dumps(plain.model))
        assert conv["warnings"] == plain.warnings
        assert json.loads((tmp_path / f"{name}.mapping.json").read_text(encoding="utf-8")) == json.loads(
            json.dumps(plain.mapping))
