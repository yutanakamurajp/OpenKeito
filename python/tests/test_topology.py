"""`openkeito.topology`: the connections of a model as a document.

The document is the one of `openkeito topology` (docs/model.md, "Topology").
Here: its shape, that the terminals it reports are the nodes of the circuit
the solver builds (for the examples and for the models the converters make of
pandapower and PowSyBl networks), and that the command prints the same
document for the same text.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest

import openkeito
from openkeito import _native

ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = sorted((ROOT / "examples").glob("*.json"))


def mismatches(model) -> list[str]:
    """Where the terminals of `topology` differ from the circuit the solver
    builds of the model."""
    return _native.topology_mismatches(json.dumps(model))


def test_the_document_of_a_model():
    t = openkeito.topology(ROOT / "examples" / "rlc.json")
    assert t["command"] == "topology" and t["valid"] is True
    assert t["errors"] == [] and t["diagnostics"] == []
    assert [c["name"] for c in t["components"]] == ["E1", "R1", "L1", "C1"]
    r1 = t["components"][1]
    assert r1["type"] == "resistor" and r1["path"] == "components[1]"
    assert r1["terminals"] == [
        {"name": "nodes[0]", "path": "components[1].nodes[0]", "nodes": ["n1"]},
        {"name": "nodes[1]", "path": "components[1].nodes[1]", "nodes": ["n2"]},
    ]
    assert [n["name"] for n in t["nodes"]] == ["n1", "gnd", "n2", "n3"]
    assert t["nodes"][1] == {"name": "gnd", "ground": True, "components": ["E1", "C1"]}
    assert "path" not in t["model"]


def test_a_dict_a_text_and_a_path_give_the_same_document():
    path = ROOT / "examples" / "three_phase_fault.json"
    text = path.read_text(encoding="utf-8")
    of_text = openkeito.topology(text)
    assert openkeito.topology(path) == of_text
    of_dict = openkeito.topology(json.loads(text))
    # The text of a dict is written anew: only its checksum differs.
    assert of_dict["model"]["sha256"] != of_text["model"]["sha256"]
    of_dict["model"] = of_text["model"]
    assert of_dict == of_text
    assert [b["name"] for b in of_text["buses"]] == ["src", "b2", "b3"]


def test_a_model_that_is_not_valid_keeps_its_connections():
    model = json.loads((ROOT / "examples" / "rlc.json").read_text(encoding="utf-8"))
    model["components"][1]["resistance_ohm"] = -1.0
    model["components"].append({"type": "capacitor", "name": "C2", "nodes": ["x", "y"], "capacitance_f": 1e-6})
    t = openkeito.topology(model)
    assert t["valid"] is False
    assert len(t["components"]) == 5
    report = openkeito.validate(model)
    listed = report["errors"] + report["warnings"]
    assert [(m["code"], m["path"]) for m in t["diagnostics"]] == [(d["code"], d["path"]) for d in listed]
    by_code = {m["code"]: m for m in t["diagnostics"]}
    assert by_code["E130"]["components"] == ["R1"]
    assert by_code["E141"]["nodes"] == ["x", "y"]
    # A text that is no model: the error, and nothing to draw.
    t = openkeito.topology("{ not JSON")
    assert t["valid"] is False and t["errors"][0]["code"] == "E001" and "components" not in t


@pytest.mark.parametrize("path", EXAMPLES, ids=lambda p: p.stem)
def test_the_terminals_of_the_examples_are_the_nodes_of_the_circuit(path):
    assert len(EXAMPLES) >= 21
    assert mismatches(json.loads(path.read_text(encoding="utf-8"))) == []


def test_a_model_that_is_not_valid_has_no_circuit_to_compare_with():
    with pytest.raises(ValueError, match="E160"):
        mismatches({"format": "openkeito-model", "version": "0.1",
                    "simulation": {"time_step_s": 1e-6, "end_time_s": 1e-3}, "components": []})


@pytest.mark.parametrize("case", ["case9", "case39", "case118"])
def test_the_terminals_of_a_pandapower_network_are_the_nodes_of_the_circuit(case):
    pn = pytest.importorskip("pandapower.networks")
    from openkeito.pandapower import from_pandapower

    model = from_pandapower(getattr(pn, case)(), end_time_s=0.02).model
    assert mismatches(model) == []
    t = openkeito.topology(model)
    assert t["valid"] and len(t["components"]) == len(model["components"])
    assert len(t["buses"]) >= int(case[4:])
    # Every node of a component is in the list of nodes, and the other way.
    named = {n for c in t["components"] for term in c["terminals"] for n in term["nodes"]}
    assert named == {n["name"] for n in t["nodes"]}


@pytest.mark.parametrize("make", ["create_ieee14", "create_ieee118", "create_micro_grid_be_network",
                                  "create_eurostag_tutorial_example1_network"])
def test_the_terminals_of_a_powsybl_network_are_the_nodes_of_the_circuit(make):
    pn = pytest.importorskip("pypowsybl.network")
    from openkeito.powsybl import from_powsybl

    model = from_powsybl(getattr(pn, make)(), end_time_s=0.02).model
    assert mismatches(model) == []
    t = openkeito.topology(model)
    assert t["valid"] and len(t["components"]) == len(model["components"])


def test_the_terminals_of_a_transition_are_the_nodes_of_the_circuit():
    """A model with machines, their controls and switched loads, as
    `openkeito.pandapower.transition` writes it."""
    model = json.loads((ROOT / "examples" / "transition_three_times.json").read_text(encoding="utf-8"))
    assert mismatches(model) == []
    t = openkeito.topology(model)
    signals = {s["name"]: s for s in t["signals"]}
    # Every signal that a component reads has the block that gives it.
    for c in t["components"]:
        for s in c.get("signals", []):
            assert signals[s["signal"]]["from"] is not None
            assert {"component": c["name"], "field": s["field"]} in signals[s["signal"]]["to"]


def _command() -> Path | None:
    """The command built from this checkout, when it is there and of the
    version of the package."""
    for name in ("openkeito.exe", "openkeito"):
        exe = ROOT / "target" / "release" / name
        if exe.exists():
            said = subprocess.run([str(exe), "--version"], capture_output=True, text=True).stdout.split()
            if said and said[-1] == openkeito.__version__:
                return exe
    return None


def test_the_command_prints_the_same_document():
    exe = _command()
    if exe is None:
        pytest.skip("target/release/openkeito is not built (cargo build --release -p openkeito-cli)")
    for path in EXAMPLES:
        printed = subprocess.run([str(exe), "topology", str(path)], capture_output=True, check=True).stdout
        assert json.loads(printed.decode("utf-8")) == openkeito.topology(path), path.name
