"""Diagnostics of wrong models: code, place, message and what to change.

A table for the design claim that the input is made for people and AI
agents alike: a model with a mistake comes back with a code, the path of
the field, the values involved and a hint that says what to change, and a
model with several mistakes gets all of them at once. The mistakes are made
on purpose in the shipped examples; the texts are those of
`openkeito.validate`, unedited (cut to a length for the table; whole in the
CSV).

Two limits are visible in the table and are not hidden: a mistake that
keeps the JSON document from being read into the model structure (a number
without its exponent letter, an unknown field, an unknown type) is reported
alone, with line and column, before the checks of the model run; the checks
of a model that was read report everything they find together.
"""

from __future__ import annotations

import copy
import json

import openkeito

import lib

TITLE = "Diagnostics of deliberately wrong models: code, place, message, hint"


def cases() -> list[tuple[str, object]]:
    """(what was done wrong, model or JSON text)."""
    rlc = lib.example("rlc.json")
    out: list[tuple[str, object]] = []

    text = json.dumps(rlc).replace('"capacitance_f": 1e-06', '"capacitance_f": 1.0-6')
    assert "1.0-6" in text
    out.append(("A number without the exponent letter (1.0-6 for 1.0e-6)", text))

    m = copy.deepcopy(rlc)
    m["components"][1]["resistance"] = m["components"][1].pop("resistance_ohm")
    out.append(("A field without its unit (resistance for resistance_ohm)", m))

    m = copy.deepcopy(rlc)
    m["simulation"]["end_time_s"] = -1
    m["components"][1]["resistance_ohm"] = -10
    m["components"][3]["name"] = "R1"
    m["outputs"][0]["node"] = "n9"
    out.append(("Four mistakes in one model: negative end time, negative resistance, a name used twice, "
                "an output of a node that does not exist", m))

    m = copy.deepcopy(rlc)
    m["components"].append({"type": "voltage_source", "name": "E2", "nodes": ["n1", "gnd"],
                            "waveform": {"type": "dc", "value": 2.0}})
    out.append(("Two ideal voltage sources in parallel", m))

    m = lib.example("line_energization.json")
    m["simulation"]["time_step_s"] = 5e-4
    out.append(("A time step longer than the travel time of a travelling-wave line", m))

    m = lib.example("synchronous_machine.json")
    m["components"][0]["xd_transient_pu"] = 0.2
    out.append(("A transient reactance below the subtransient one", m))

    m = lib.example("converter.json")
    m["simulation"]["time_step_s"] = 5e-4
    out.append(("A time step of half the time constant of a converter's current control", m))
    return out


def cut(text: str, n: int) -> str:
    return text if len(text) <= n else text[:n - 3].rstrip() + "..."


def make(ctx: lib.Context) -> dict:
    rows, table, counts = [], [], {}
    for what, m in cases():
        document = openkeito.validate(m)
        found = list(document.get("errors", [])) + list(document.get("warnings", []))
        assert found, what
        counts[what] = len(found)
        for k, d in enumerate(found):
            severity = d.get("severity") or ("error" if d["code"].startswith("E") else "warning")
            rows.append((what, document["valid"], severity, d["code"], d["path"], d["message"], d["hint"]))
            table.append([what if k == 0 else "", d["code"], cut(d["path"], 40), cut(d["message"], 110),
                          cut(d["hint"], 150)])
    lib.write_csv(ctx, "diagnostics.csv", ["mistake", "valid", "severity", "code", "path", "message", "hint"], rows)
    lib.write_table(ctx, "diagnostics_table", ["Mistake", "Code", "Place", "Message", "Hint"], table,
                    "Diagnostics that openkeito.validate returns for deliberately wrong models.")

    catalog = openkeito.components()
    documented = sum(1 for c in catalog if c.get("example") and c.get("parameters"))
    with_unit = sum(1 for c in catalog for p in c["parameters"] if p.get("unit"))
    return {
        "diagnostics per case": list(counts.values()),
        "every diagnostic has code, path, message and hint": all(all(r[k] for k in (3, 4, 5, 6)) for r in rows),
        "codes in the table": sorted({r[3] for r in rows}),
        "the model with four mistakes: codes returned together": [r[3] for r in rows if r[0].startswith("Four")],
        "component catalog: circuit components, control blocks": [
            sum(1 for c in catalog if c.get("category") == "circuit"),
            sum(1 for c in catalog if c.get("category") == "control")],
        "component catalog: entries with parameters and an example": documented,
        "component catalog: parameters with a unit": with_unit,
    }
