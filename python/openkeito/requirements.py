"""Requirement files: a set of grid-connection tests run as one batch and
judged against thresholds that name their source.

    from openkeito.requirements import load_requirements, run_suite
    req = load_requirements("examples/grid_test/requirements.json")
    suite = run_suite(req, jobs=8, out="suite")
    suite.passed, suite.counts            # False, {"cases": 18, "passed": 17, "failed": 1}
    print(suite.table_csv())              # one row per case and criterion

A requirement file (``"format": "openkeito-requirements"``, ``"version":
"0.1"``) names the device under test and its point of connection, the grid
equivalent (``openkeito.gridtest.GridEquivalent`` without the short-circuit
ratio), the short-circuit ratios and the disturbances (every pair is a
case), and the criteria: a metric of ``openkeito.metrics`` on a signal, a
column or the positive-sequence voltage of a bus, with its limits and the
source of the limits. OpenKeito does not ship thresholds of grid codes: a
criterion without a source is refused (E515), and a value that has no
clause to cite is declared as the user's own (``{"kind": "user", "note":
...}``). ``validate_requirements`` returns the diagnostics (E510 to E517,
each with a hint); docs/python.md, "Requirement files", describes the
fields.

A case fails when its run does not complete, when it reports a warning
listed in ``fail_on_warnings``, or when a criterion is outside its limits or
cannot be evaluated; every failure gives the criterion, the metric, the
value, the limits and their source. The table is the same, byte for byte,
for any number of jobs.
"""

from __future__ import annotations

import csv
import io
import json
import math
import os
import re
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import numpy as np

import openkeito
from openkeito import metrics
from openkeito.gridtest import (Fault, FrequencyRamp, FrequencyStep, GridEquivalent, PhaseJump, VoltageStep,
                                attach)

__all__ = [
    "FORMAT",
    "METRICS",
    "VERSION",
    "Requirements",
    "RequirementsError",
    "SuiteResult",
    "load_requirements",
    "run_suite",
    "validate_requirements",
]

FORMAT = "openkeito-requirements"
VERSION = "0.1"
RESULT_FORMAT = "openkeito-suite-result"

_NAME = re.compile(r"^[A-Za-z0-9_-]+$")
_CODE = re.compile(r"^[EW][0-9]{3}$")

#: The disturbances: their fields (required, optional).
DISTURBANCES: dict[str, tuple[tuple[str, ...], tuple[str, ...]]] = {
    "voltage_step": (("change_pu",), ("duration_s",)),
    "phase_jump": (("angle_deg",), ("duration_s",)),
    "frequency_step": (("change_hz",), ("duration_s",)),
    "frequency_ramp": (("rate_hz_per_s",), ("duration_s",)),
    "fault": (("duration_s",), ("phases", "ground", "resistance_ohm", "residual_voltage_pu")),
}

#: The metrics of a criterion: their fields (required, optional) and what the value is.
METRICS: dict[str, tuple[tuple[str, ...], tuple[str, ...], str]] = {
    "settling_time": (("tolerance",), ("target", "from"),
                      "s after the start (from: start) or the end (from: end, the default) of the disturbance "
                      "until the quantity stays within tolerance of target (pre: its mean over the period "
                      "before the disturbance, the default; or a number); inf when it never does"),
    "rise_time": ((), (), "s from 10 % to 90 % of the change from the value before the disturbance to the mean "
                          "of the last period of the run"),
    "overshoot": ((), (), "the largest excursion beyond the final value over the change (as rise_time)"),
    "maximum": ((), ("from", "delay_s"), "the largest value from the start (or end) of the disturbance "
                                         "plus delay_s to the end of the run"),
    "minimum": ((), ("from", "delay_s"), "the smallest value, as maximum"),
    "overvoltage_duration": (("threshold",), ("from", "delay_s"),
                             "s above threshold after the start (or end) of the disturbance plus delay_s"),
    "envelope": (("points",), ("side", "interpolation", "from"),
                 "the largest distance outside the voltage-time curve points [[s after the start, level], ...] "
                 "(side lower: stay at or above, upper: at or below; interpolation step or linear); 0 inside"),
    "thd": ((), ("cycles", "at"), "the THD over cycles (default 10) periods ending at the end of the run "
                                  "(at: end) or at the start of the disturbance (at: pre)"),
    "swing": ((), ("last_s",), "max - min of the quantity over the last last_s (default 0.1) s of the run"),
}

_QUANTITIES = ("signal", "column", "bus")
_CRITERION_FIELDS = {"name", "metric", "min", "max", "source", "disturbances", "description", *_QUANTITIES}
_SOURCE_DOCUMENT = ("document", "edition", "clause")


class RequirementsError(ValueError):
    """A requirement file that is not valid: ``diagnostics`` lists the
    errors (code, path, message, hint)."""

    def __init__(self, diagnostics: list[dict[str, Any]]):
        self.diagnostics = diagnostics
        super().__init__("\n".join(f"{d['code']} at {d['path']}: {d['message']} (hint: {d['hint']})"
                                   for d in diagnostics))


def _diag(code: str, path: str, message: str, hint: str) -> dict[str, Any]:
    return {"code": code, "severity": "error", "path": path, "message": message, "hint": hint}


def _number(x: Any) -> bool:
    return isinstance(x, (int, float)) and not isinstance(x, bool) and math.isfinite(float(x))


class _Checker:
    def __init__(self) -> None:
        self.out: list[dict[str, Any]] = []

    def add(self, code: str, path: str, message: str, hint: str) -> None:
        self.out.append(_diag(code, path, message, hint))

    def fields(self, code: str, path: str, obj: Mapping[str, Any], allowed: set[str], required: tuple[str, ...]
               ) -> None:
        for key in sorted(set(obj) - allowed):
            self.add(code, f"{path}.{key}", f"unknown field {key!r}",
                     f"Remove it or write one of: {', '.join(sorted(allowed))}.")
        for key in required:
            if key not in obj:
                self.add(code, f"{path}.{key}", f"missing field {key!r}", f"Add {key!r}.")

    def positive(self, code: str, path: str, value: Any, *, zero: bool = False) -> None:
        if not _number(value) or float(value) < 0 or (float(value) == 0 and not zero):
            self.add(code, path, f"{value!r} is not a {'non-negative' if zero else 'positive'} number",
                     "Write a finite number" + (" at least 0." if zero else " above 0."))


def validate_requirements(doc: Any, base: str | os.PathLike | None = None) -> list[dict[str, Any]]:
    """The errors of a requirement file (a dict), each with code, path,
    message and hint; an empty list when it is valid. With ``base`` (the
    directory of the file), the device model is read from it and its bus
    and the buses and columns that the criteria name are checked (E517)."""
    c = _Checker()
    if not isinstance(doc, Mapping):
        c.add("E510", "$", "the requirement file is not a JSON object",
              f'Write {{"format": "{FORMAT}", "version": "{VERSION}", ...}}.')
        return c.out
    if doc.get("format") != FORMAT or doc.get("version") != VERSION:
        c.add("E510", "format", f"format {doc.get('format')!r} version {doc.get('version')!r} is not a requirement "
              "file this version reads", f'Set "format": "{FORMAT}" and "version": "{VERSION}".')
    top = {"format", "version", "name", "description", "device", "grid", "simulation", "scr", "disturbances",
           "criteria", "fail_on_warnings"}
    c.fields("E511", "$", doc, top, ("device", "grid", "scr", "disturbances", "criteria"))

    device = doc.get("device")
    if isinstance(device, Mapping):
        c.fields("E511", "device", device, {"model", "bus"}, ("model", "bus"))
        if "model" in device and not isinstance(device["model"], (str, Mapping)):
            c.add("E511", "device.model", "the device model is neither a path nor a model",
                  "Write the path of the model, relative to the requirement file, or the model itself.")
        if "bus" in device and not (isinstance(device["bus"], str) and device["bus"]):
            c.add("E511", "device.bus", "the bus of the point of connection is not a name", 'Write "bus": "<name>".')
    elif "device" in doc:
        c.add("E511", "device", "device is not an object", 'Write {"model": "device.json", "bus": "poi"}.')

    grid = doc.get("grid")
    frequency = 50.0
    if isinstance(grid, Mapping):
        allowed = {"rated_power_mva", "rated_voltage_kv", "frequency_hz", "x_over_r", "z0_over_z1"}
        c.fields("E511", "grid", grid, allowed, ("rated_power_mva", "rated_voltage_kv", "x_over_r"))
        for key in allowed & set(grid):
            c.positive("E511", f"grid.{key}", grid[key])
        if _number(grid.get("frequency_hz")) and grid["frequency_hz"] > 0:
            frequency = float(grid["frequency_hz"])
    elif "grid" in doc:
        c.add("E511", "grid", "grid is not an object",
              'Write {"rated_power_mva": .., "rated_voltage_kv": .., "x_over_r": ..}.')

    sim = doc.get("simulation", {})
    if isinstance(sim, Mapping):
        c.fields("E511", "simulation", sim, {"time_step_s", "end_time_s"}, ())
        for key in {"time_step_s", "end_time_s"} & set(sim):
            c.positive("E511", f"simulation.{key}", sim[key])
    else:
        c.add("E511", "simulation", "simulation is not an object", 'Write {"end_time_s": ..} or leave it out.')

    scr = doc.get("scr")
    if isinstance(scr, list) and scr:
        for k, v in enumerate(scr):
            c.positive("E511", f"scr[{k}]", v)
        labels = [format(float(v), "g") for v in scr if _number(v)]
        if len(set(labels)) != len(labels):
            c.add("E511", "scr", "a short-circuit ratio is listed twice", "List each value once.")
    elif "scr" in doc:
        c.add("E511", "scr", "scr is not a non-empty list", "Write the short-circuit ratios, [10, 3, 1.2].")

    names: set[str] = set()
    dists = doc.get("disturbances")
    if isinstance(dists, list) and dists:
        for k, d in enumerate(dists):
            path = f"disturbances[{k}]"
            if not isinstance(d, Mapping):
                c.add("E512", path, "a disturbance is not an object", 'Write {"name": .., "type": .., "time_s": ..}.')
                continue
            kind = d.get("type")
            if kind not in DISTURBANCES:
                c.add("E512", f"{path}.type", f"unknown disturbance type {kind!r}",
                      f"Write one of: {', '.join(DISTURBANCES)}.")
                continue
            required, optional = DISTURBANCES[kind]
            c.fields("E512", path, d, {"name", "type", "time_s", "description", *required, *optional},
                     ("name", "time_s", *required))
            name = d.get("name")
            if isinstance(name, str) and _NAME.match(name):
                if name in names:
                    c.add("E512", f"{path}.name", f"the disturbance name {name!r} is used twice",
                          "Give every disturbance a name of its own.")
                names.add(name)
            elif "name" in d:
                c.add("E512", f"{path}.name", f"{name!r} is not a name of letters, digits, '_' and '-'",
                      "Write a name such as fault_150ms: it is part of the name of each case.")
            if "time_s" in d:
                if not _number(d["time_s"]) or float(d["time_s"]) < 1.0 / frequency:
                    c.add("E512", f"{path}.time_s", f"{d['time_s']!r} is before the end of the first period",
                          f"Start the disturbance at {1.0 / frequency:g} s or later: the values before it are "
                          "the mean over the period before.")
            for key in ("duration_s", "resistance_ohm", "rate_hz_per_s", "change_pu", "change_hz", "angle_deg",
                        "residual_voltage_pu"):
                if key not in d:
                    continue
                if key == "duration_s":
                    c.positive("E512", f"{path}.{key}", d[key])
                elif key == "resistance_ohm":
                    c.positive("E512", f"{path}.{key}", d[key], zero=True)
                elif not _number(d[key]):
                    c.add("E512", f"{path}.{key}", f"{d[key]!r} is not a number", "Write a finite number.")
            if kind == "voltage_step" and _number(d.get("change_pu")) and d["change_pu"] <= -1:
                c.add("E512", f"{path}.change_pu", "the voltage would be zero or negative",
                      "Write a change above -1 (-0.1: to 90 %).")
            if kind == "fault":
                if "resistance_ohm" in d and "residual_voltage_pu" in d:
                    c.add("E512", path, "both resistance_ohm and residual_voltage_pu are given", "Give one of them.")
                rv = d.get("residual_voltage_pu")
                if rv is not None and not (_number(rv) and 0 <= rv < 1):
                    c.add("E512", f"{path}.residual_voltage_pu", f"{rv!r} is not at least 0 and below 1",
                          "Write the voltage left at the point of connection, in per unit (0.2).")
                ph = d.get("phases", ["a", "b", "c"])
                if not (isinstance(ph, list) and ph and set(ph) <= {"a", "b", "c"} and len(set(ph)) == len(ph)):
                    c.add("E512", f"{path}.phases", f"{ph!r} is not a list of phases",
                          'Write ["a", "b", "c"], ["a"] (with ground) or ["b", "c"].')
                if "ground" in d and not isinstance(d["ground"], bool):
                    c.add("E512", f"{path}.ground", "ground is not true or false", "Write true or false.")
    elif "disturbances" in doc:
        c.add("E512", "disturbances", "disturbances is not a non-empty list",
              'Write [{"name": "jump", "type": "phase_jump", "time_s": 0.2, "angle_deg": 30}].')

    crits = doc.get("criteria")
    seen: set[str] = set()
    if isinstance(crits, list) and crits:
        for k, cr in enumerate(crits):
            _check_criterion(c, f"criteria[{k}]", cr, names, seen)
        if isinstance(dists, list):
            _check_bus_start(c, dists, crits, frequency)
    elif "criteria" in doc:
        c.add("E514", "criteria", "criteria is not a non-empty list",
              'Write [{"name": .., "metric": .., "signal": .., "max": .., "source": {..}}].')

    warn = doc.get("fail_on_warnings", [])
    if not (isinstance(warn, list) and all(isinstance(w, str) and _CODE.match(w) for w in warn)):
        c.add("E516", "fail_on_warnings", f"{warn!r} is not a list of diagnostic codes",
              'Write the codes of the warnings that fail a case, ["W290"].')

    if not c.out and base is not None:
        try:
            model = _device_model(doc, Path(base))
        except (OSError, ValueError) as e:
            c.add("E517", "device.model", f"the device model cannot be read: {e}",
                  "Write the path of the model relative to the requirement file.")
        else:
            bus = doc["device"]["bus"]
            buses = _buses(model)
            if bus not in buses:
                c.add("E517", "device.bus", f"the device model has no bus {bus!r}",
                      f"Write one of its buses: {', '.join(sorted(buses)) or '(none)'}.")
            _check_names(c, doc["criteria"], model, buses)
    return c.out


def _uses_pre(cr: Mapping[str, Any]) -> bool:
    """The criterion needs the mean over the period before the disturbance."""
    metric = cr.get("metric")
    return metric in ("rise_time", "overshoot") or (metric == "settling_time" and cr.get("target", "pre") == "pre")


def _check_bus_start(c: _Checker, dists: list[Any], crits: list[Any], frequency: float) -> None:
    """The positive-sequence voltage of a bus is the DFT over one period, so
    it has values from the end of the first period on, and its mean over
    the period before a disturbance (for the criteria that use it) needs
    the disturbance two periods or more after the start."""
    period = 1.0 / frequency
    flagged: set[int] = set()
    for cr in crits:
        if not (isinstance(cr, Mapping) and "bus" in cr and _uses_pre(cr)):
            continue
        only = cr.get("disturbances")
        for k, d in enumerate(dists):
            if k in flagged or not isinstance(d, Mapping) or not _number(d.get("time_s")):
                continue
            if isinstance(only, list) and d.get("name") not in only:
                continue
            ts = float(d["time_s"])
            if period <= ts < 2.0 * period * (1.0 - 1e-12):
                flagged.add(k)
                c.add("E512", f"disturbances[{k}].time_s",
                      f"{d['time_s']!r} is before the end of the second period, and criterion {cr.get('name')!r} "
                      f"judges the voltage of bus {cr['bus']!r}",
                      f"Start the disturbance at {2.0 * period:g} s or later: the voltage of a bus is the DFT over "
                      "one period, which has values from the end of the first period, and its value before the "
                      "disturbance is the mean over the period before.")


_GRID_COMPONENT = re.compile(r"^grid(_z|_fault[0-9]*)?$")
# V(<bus>.<phase>) of bus_voltage, I(<component>...) of branch_current (docs/model.md). The
# names of buses and components may hold '.', so a column is split by the names the model has.
_COLUMN = re.compile(r"^([VI])\(([^()]+)\)$")


def _check_names(c: _Checker, crits: list[Mapping[str, Any]], model: Mapping[str, Any], buses: set[str]) -> None:
    """The buses and the columns of voltages and currents that the criteria
    name are in the device model (or are those that attach adds)."""
    buses = buses | {"grid_src"}
    components = {comp.get("name") for comp in model.get("components", []) if isinstance(comp.get("name"), str)}
    nodes = _nodes(model)
    for k, cr in enumerate(crits):
        if "bus" in cr and cr["bus"] not in buses:
            c.add("E517", f"criteria[{k}].bus", f"the device model has no bus {cr['bus']!r}",
                  f"Write one of its buses: {', '.join(sorted(buses))}.")
        m = _COLUMN.match(cr.get("column", ""))
        if m is None:
            continue
        kind, inner = m.groups()
        if kind == "V":
            # V(<node>) of node_voltage: a node name may end in .a, .b or .c too (V(n1.x.b)).
            if inner in nodes:
                continue
            bus, _, phase = inner.rpartition(".")
            if bus and phase in ("a", "b", "c") and bus not in buses:
                c.add("E517", f"criteria[{k}].column", f"the column {cr['column']!r} names {bus!r}, which is not a "
                      "bus of the device model",
                      f"Write the voltage of one of its buses: {', '.join(sorted(buses))}.")
        elif not (any(inner == n or inner.startswith(n + ".") for n in components)
                  or _GRID_COMPONENT.match(inner.split(".", 1)[0])):
            c.add("E517", f"criteria[{k}].column", f"the column {cr['column']!r} names no component of the device "
                  "model",
                  f"Write the current of one of its components: {', '.join(sorted(components)) or '(none)'}.")


def _check_criterion(c: _Checker, path: str, cr: Any, names: set[str], seen: set[str]) -> None:
    if not isinstance(cr, Mapping):
        c.add("E514", path, "a criterion is not an object", 'Write {"name": .., "metric": .., ..}.')
        return
    name = cr.get("name")
    if not (isinstance(name, str) and _NAME.match(name)):
        c.add("E514", f"{path}.name", f"{name!r} is not a name of letters, digits, '_' and '-'",
              "Give the criterion a name such as p_recovery.")
    elif name in seen:
        c.add("E514", f"{path}.name", f"the criterion name {name!r} is used twice", "Give every criterion its own name.")
    else:
        seen.add(name)
    metric = cr.get("metric")
    if metric not in METRICS:
        c.add("E513", f"{path}.metric", f"unknown metric {metric!r}", f"Write one of: {', '.join(METRICS)}.")
        return
    required, optional, _ = METRICS[metric]
    c.fields("E514", path, cr, _CRITERION_FIELDS | set(required) | set(optional), ("name", "metric", *required))
    given = [q for q in _QUANTITIES if q in cr]
    if len(given) != 1:
        c.add("E514", path, "the criterion names " + ("no quantity" if not given else "more than one quantity"),
              'Give one of "signal" (a control signal), "column" (a column of result.csv) or "bus" (the '
              "positive-sequence voltage of a bus, per unit).")
    elif not (isinstance(cr[given[0]], str) and cr[given[0]]):
        c.add("E514", f"{path}.{given[0]}", "the quantity is not a name", "Write the name of the signal, column or bus.")
    elif metric == "thd" and given[0] == "bus":
        c.add("E514", f"{path}.bus", "thd is not defined for the positive-sequence voltage of a bus (a magnitude "
              "without its waveform)", 'Judge the THD of a waveform: "column": "V(<bus>.a)" or a current.')
    limits = [key for key in ("min", "max") if key in cr]
    if not limits:
        c.add("E514", path, "the criterion has no limit", 'Give "max", "min" or both.')
    for key in limits:
        if not _number(cr[key]):
            c.add("E514", f"{path}.{key}", f"{cr[key]!r} is not a number", "Write a finite number.")
    if len(limits) == 2 and _number(cr["min"]) and _number(cr["max"]) and cr["min"] > cr["max"]:
        c.add("E514", path, "min is above max", "Write min at most max.")
    dl = cr.get("disturbances")
    if dl is not None:
        if not (isinstance(dl, list) and dl and all(isinstance(x, str) for x in dl)):
            c.add("E514", f"{path}.disturbances", "disturbances is not a list of names",
                  "List the names of the disturbances the criterion applies to, or leave it out (all).")
        else:
            for x in dl:
                if x not in names:
                    c.add("E514", f"{path}.disturbances", f"no disturbance is named {x!r}",
                          f"Write one of: {', '.join(sorted(names))}.")
    _check_metric(c, path, metric, cr)
    _check_source(c, f"{path}.source", cr.get("source"))


def _check_metric(c: _Checker, path: str, metric: str, cr: Mapping[str, Any]) -> None:
    def one_of(key: str, choices: tuple[str, ...]) -> None:
        if key in cr and cr[key] not in choices:
            c.add("E514", f"{path}.{key}", f"{cr[key]!r} is not one of {', '.join(choices)}",
                  f"Write one of: {', '.join(choices)}.")

    one_of("from", ("start", "end"))
    one_of("side", ("lower", "upper"))
    one_of("interpolation", ("step", "linear"))
    one_of("at", ("end", "pre"))
    for key in ("tolerance", "last_s"):
        if key in cr:
            c.positive("E514", f"{path}.{key}", cr[key])
    if "delay_s" in cr:
        c.positive("E514", f"{path}.delay_s", cr["delay_s"], zero=True)
    if "threshold" in cr and not _number(cr["threshold"]):
        c.add("E514", f"{path}.threshold", f"{cr['threshold']!r} is not a number", "Write a finite number.")
    if "target" in cr and cr["target"] != "pre" and not _number(cr["target"]):
        c.add("E514", f"{path}.target", f"{cr['target']!r} is neither \"pre\" nor a number",
              'Write "pre" (the value before the disturbance) or a number.')
    if "cycles" in cr and not (isinstance(cr["cycles"], int) and not isinstance(cr["cycles"], bool)
                               and cr["cycles"] >= 1):
        c.add("E514", f"{path}.cycles", f"{cr['cycles']!r} is not a whole number of periods", "Write 1 or more.")
    if metric == "envelope" and "points" in cr:
        pts = cr["points"]
        ok = (isinstance(pts, list) and pts and all(isinstance(p, list) and len(p) == 2 and all(map(_number, p))
                                                    for p in pts))
        if ok:
            times = [float(p[0]) for p in pts]
            ok = times[0] >= 0 and all(b > a for a, b in zip(times, times[1:]))
        if not ok:
            c.add("E514", f"{path}.points", "points is not a list of [time, level] with ascending times from 0",
                  "Write the curve as [[0, 0.2], [0.15, 0.2], [0.15001, 0.85]] (s after the start, level).")


def _check_source(c: _Checker, path: str, source: Any) -> None:
    hint = ('Write where the limits come from: {"kind": "document", "document": "<title>", "edition": "<edition '
            'or year>", "clause": "<clause or table>"}, or, for a value of your own without a clause to cite, '
            '{"kind": "user", "note": "<why this value>"}. OpenKeito ships no thresholds of grid codes.')
    if not isinstance(source, Mapping):
        c.add("E515", path, "the limits have no source", hint)
        return
    kind = source.get("kind")
    if kind == "document":
        c.fields("E515", path, source, {"kind", *_SOURCE_DOCUMENT, "note"}, ())
        for key in _SOURCE_DOCUMENT:
            if not (isinstance(source.get(key), str) and source[key].strip()):
                c.add("E515", f"{path}.{key}", f"the source has no {key}", hint)
    elif kind == "user":
        c.fields("E515", path, source, {"kind", "note"}, ())
        if not (isinstance(source.get("note"), str) and source["note"].strip()):
            c.add("E515", f"{path}.note", "the value of the user has no note", hint)
    else:
        c.add("E515", f"{path}.kind", f"the source kind {kind!r} is neither document nor user", hint)


def _buses(model: Mapping[str, Any]) -> set[str]:
    out: set[str] = set()
    for comp in model.get("components", []):
        for key in ("bus", "buses"):
            value = comp.get(key)
            for b in ([value] if isinstance(value, str) else value or []):
                if isinstance(b, str):
                    out.add(b)
    return out


def _nodes(model: Mapping[str, Any]) -> set[str]:
    """The nodes of the components with nodes and those of the node_voltage outputs."""
    out: set[str] = set()
    for comp in model.get("components", []):
        value = comp.get("nodes")
        if isinstance(value, list):
            out.update(n for n in value if isinstance(n, str))
    for o in model.get("outputs", []):
        if isinstance(o, Mapping) and o.get("type") == "node_voltage" and isinstance(o.get("node"), str):
            out.add(o["node"])
    return out


def _device_model(doc: Mapping[str, Any], base: Path) -> dict[str, Any]:
    model = doc["device"]["model"]
    if isinstance(model, Mapping):
        return json.loads(json.dumps(model))
    path = Path(model)
    if not path.is_absolute():
        path = base / path
    return json.loads(path.read_text(encoding="utf-8"))


@dataclass
class Requirements:
    """A requirement file that is valid, with its device model."""

    doc: dict[str, Any]
    device: dict[str, Any]

    @property
    def name(self) -> str:
        return str(self.doc.get("name", ""))

    @property
    def bus(self) -> str:
        return self.doc["device"]["bus"]

    @property
    def frequency_hz(self) -> float:
        return float(self.doc["grid"].get("frequency_hz", 50.0))

    def grid(self, scr: float) -> GridEquivalent:
        g = self.doc["grid"]
        return GridEquivalent(rated_power_mva=g["rated_power_mva"], rated_voltage_kv=g["rated_voltage_kv"],
                              scr=float(scr), x_over_r=g["x_over_r"], frequency_hz=self.frequency_hz,
                              z0_over_z1=g.get("z0_over_z1", 1.0))

    def disturbance(self, name: str) -> Any:
        d = next(x for x in self.doc["disturbances"] if x["name"] == name)
        kind = d["type"]
        if kind == "voltage_step":
            return VoltageStep(d["time_s"], d["change_pu"], d.get("duration_s"))
        if kind == "phase_jump":
            return PhaseJump(d["time_s"], d["angle_deg"], d.get("duration_s"))
        if kind == "frequency_step":
            return FrequencyStep(d["time_s"], d["change_hz"], d.get("duration_s"))
        if kind == "frequency_ramp":
            return FrequencyRamp(d["time_s"], d["rate_hz_per_s"], d.get("duration_s"))
        return Fault(d["time_s"], d["duration_s"], tuple(d.get("phases", ("a", "b", "c"))), d.get("ground", True),
                     d.get("resistance_ohm"), d.get("residual_voltage_pu"))

    def cases(self) -> list[tuple[str, float, str]]:
        """(name, SCR, disturbance) of every case, in the order of the file."""
        return [(f"scr{format(float(s), 'g')}.{d['name']}", float(s), d["name"])
                for s in self.doc["scr"] for d in self.doc["disturbances"]]


def load_requirements(source: Any) -> Requirements:
    """Reads a requirement file (a path, JSON text or a dict; the device
    model path is relative to the file, or to the working directory for a
    dict or text). Raises RequirementsError with the diagnostics when it is
    not valid."""
    base = Path.cwd()
    if isinstance(source, Mapping):
        doc = json.loads(json.dumps(source))
    else:
        text = os.fspath(source) if isinstance(source, os.PathLike) else str(source)
        if text.lstrip().startswith("{"):
            raw = text
        else:
            path = Path(text)
            raw = path.read_text(encoding="utf-8")
            base = path.resolve().parent
        try:
            doc = json.loads(raw)
        except json.JSONDecodeError as e:
            raise RequirementsError([_diag("E510", f"line {e.lineno} column {e.colno}", f"not JSON: {e.msg}",
                                           "Write the requirement file as JSON.")]) from None
    errors = validate_requirements(doc, base)
    if errors:
        raise RequirementsError(errors)
    return Requirements(doc=doc, device=_device_model(doc, base))


def _fmt(value: Any) -> str:
    if value is None:
        return ""
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, float):
        return repr(value)
    return str(value)


def _json_value(value: Any) -> Any:
    return None if isinstance(value, float) and not math.isfinite(value) else value


def _source_text(source: Mapping[str, Any]) -> str:
    if source["kind"] == "document":
        return f"{source['document']}, {source['edition']}, {source['clause']}"
    return f"user value: {source['note']}"


@dataclass
class SuiteResult:
    """The judged cases of a requirement file: ``cases`` (one dict per case:
    name, scr, disturbance, status, passed, warnings, failures and the
    values of the criteria) and ``rows`` (one per case and criterion, and
    one per run failure or failing warning), the rows of ``table_csv``."""

    requirements: Requirements
    cases: list[dict[str, Any]]
    rows: list[dict[str, Any]] = field(default_factory=list)
    batch: Any = None

    COLUMNS = ("case", "scr", "disturbance", "criterion", "metric", "quantity", "value", "min", "max", "result",
               "reason", "source")

    @property
    def passed(self) -> bool:
        return all(c["passed"] for c in self.cases)

    @property
    def counts(self) -> dict[str, int]:
        n = sum(1 for c in self.cases if c["passed"])
        return {"cases": len(self.cases), "passed": n, "failed": len(self.cases) - n}

    def table_csv(self) -> str:
        buf = io.StringIO()
        w = csv.writer(buf, lineterminator="\n")
        w.writerow(self.COLUMNS)
        for r in self.rows:
            w.writerow([_fmt(r.get(k)) for k in self.COLUMNS])
        return buf.getvalue()

    def to_json(self) -> str:
        doc = {"format": RESULT_FORMAT, "version": VERSION, "requirements": self.requirements.name,
               "passed": self.passed, "counts": self.counts,
               "cases": [{**c, "values": {k: _json_value(v) for k, v in c["values"].items()},
                          "failures": [{**f, "value": _json_value(f.get("value"))} for f in c["failures"]]}
                         for c in self.cases]}
        return json.dumps(doc, indent=2, ensure_ascii=False) + "\n"

    def to_pandas(self):
        import pandas as pd

        return pd.DataFrame(self.rows, columns=list(self.COLUMNS))

    def __repr__(self) -> str:
        return f"<openkeito.requirements.SuiteResult {'passed' if self.passed else 'failed'}, {self.counts}>"


def _quantity_name(cr: Mapping[str, Any]) -> str:
    if "signal" in cr:
        return f"S({cr['signal']})"
    if "column" in cr:
        return cr["column"]
    return f"V1({cr['bus']}) pu"


def _add_outputs(model: dict[str, Any], criteria: list[Mapping[str, Any]]) -> None:
    outputs = model.setdefault("outputs", [])
    for cr in criteria:
        if "signal" in cr and not any(o.get("type") == "signal" and o.get("signal") == cr["signal"] for o in outputs):
            outputs.append({"type": "signal", "signal": cr["signal"]})
        if "bus" in cr and not any(o.get("type") == "bus_voltage" and o.get("bus") == cr["bus"] for o in outputs):
            outputs.append({"type": "bus_voltage", "bus": cr["bus"]})


class _Case:
    """The columns of one run and the times of its disturbance."""

    def __init__(self, req: Requirements, result: Any, disturbance: Mapping[str, Any]):
        self.req, self.result = req, result
        self.t = np.asarray(result.time, dtype=float)
        self.start = float(disturbance["time_s"])
        self.end = self.start + float(disturbance.get("duration_s", 0.0))
        self.period = 1.0 / req.frequency_hz
        self._bus: dict[str, np.ndarray] = {}

    def quantity(self, cr: Mapping[str, Any]) -> np.ndarray:
        if "bus" in cr:
            bus = cr["bus"]
            if bus not in self._bus:
                seq = metrics.sequence_series(self.t, *metrics.phases(self.result, f"V({bus}.{{}})"),
                                              self.req.frequency_hz)
                base = 1e3 * self.req.doc["grid"]["rated_voltage_kv"] / math.sqrt(3.0)
                self._bus[bus] = np.abs(seq["positive"]) / base
            return self._bus[bus]
        return np.asarray(self.result[_quantity_name(cr)], dtype=float)

    def mean(self, y: np.ndarray, end: float) -> float:
        h = float(self.t[1] - self.t[0])
        sel = (self.t > end - self.period + 0.5 * h) & (self.t <= end + 0.5 * h)
        w = y[sel]
        if len(w) == 0 or not np.all(np.isfinite(w)):
            raise ValueError(f"the quantity has no finite values over the period ending at {end:g} s (the voltage "
                             "of a bus has values from the end of the first period)")
        return float(np.mean(w))

    def value(self, cr: Mapping[str, Any]) -> float:
        metric = cr["metric"]
        y = self.quantity(cr)
        t = self.t
        begin = self.end if cr.get("from", "end" if metric == "settling_time" else "start") == "end" else self.start
        begin += float(cr.get("delay_s", 0.0))
        if metric == "settling_time":
            target = self.mean(y, self.start) if cr.get("target", "pre") == "pre" else float(cr["target"])
            return metrics.settling_time(t, y, target, float(cr["tolerance"]), start_time_s=begin)
        if metric in ("rise_time", "overshoot"):
            pre, final = self.mean(y, self.start), self.mean(y, float(t[-1]))
            r = metrics.step_response(t, y, self.start, initial=pre, final=final)
            return r.rise_time_s if metric == "rise_time" else r.overshoot
        if metric in ("maximum", "minimum"):
            w = y[t >= begin - 1e-9 * self.period]
            return float(np.max(w) if metric == "maximum" else np.min(w))
        if metric == "overvoltage_duration":
            return metrics.overvoltage(t, y, float(cr["threshold"]), start_time_s=begin)["duration_s"]
        if metric == "envelope":
            return metrics.envelope(t, y, cr["points"], start_time_s=begin, side=cr.get("side", "lower"),
                                    interpolation=cr.get("interpolation", "step"))["excess"]
        if metric == "thd":
            at = float(t[-1]) if cr.get("at", "end") == "end" else self.start
            return metrics.harmonics(t, y, self.req.frequency_hz, cycles=int(cr.get("cycles", 10)),
                                     end_time_s=at)["thd"]
        if metric == "swing":
            w = y[t >= t[-1] - float(cr.get("last_s", 0.1)) - 1e-9 * self.period]
            return float(np.max(w) - np.min(w))
        raise ValueError(f"unknown metric {metric!r}")


def _judge(req: Requirements, name: str, scr: float, dist: str, result: Any) -> tuple[dict[str, Any],
                                                                                       list[dict[str, Any]]]:
    doc = req.doc
    case = {"name": name, "scr": scr, "disturbance": dist, "status": result.status, "passed": True,
            "warnings": sorted({w["code"] for w in result.warnings}), "failures": [], "values": {}}
    rows: list[dict[str, Any]] = []
    base = {"case": name, "scr": scr, "disturbance": dist}

    def fail(row: dict[str, Any]) -> None:
        case["passed"] = False
        case["failures"].append({k: row.get(k) for k in ("criterion", "metric", "quantity", "value", "min", "max",
                                                         "reason", "source")})

    if result.status != "completed":
        codes = ", ".join(f"{e.get('code')}: {e.get('message')}" for e in result.summary.get("errors", []))
        row = {**base, "criterion": "run", "value": result.status, "result": "fail",
               "reason": f"the run did not complete ({codes or result.status})"}
        rows.append(row)
        fail(row)
        return case, rows
    for w in result.warnings:
        if w["code"] in doc.get("fail_on_warnings", []):
            row = {**base, "criterion": f"warning {w['code']}", "value": w["code"], "result": "fail",
                   "reason": f"{w['code']} at {w.get('path')}: {w.get('message')}", "source": "fail_on_warnings"}
            rows.append(row)
            fail(row)
    data = _Case(req, result, next(d for d in doc["disturbances"] if d["name"] == dist))
    for cr in doc["criteria"]:
        if "disturbances" in cr and dist not in cr["disturbances"]:
            continue
        lo, hi = cr.get("min"), cr.get("max")
        row = {**base, "criterion": cr["name"], "metric": cr["metric"], "quantity": _quantity_name(cr),
               "min": None if lo is None else float(lo), "max": None if hi is None else float(hi),
               "source": _source_text(cr["source"])}
        try:
            v = float(data.value(cr))
        except (KeyError, ValueError) as e:
            row.update(value=None, result="fail", reason=f"cannot be evaluated: {e}")
            rows.append(row)
            fail(row)
            continue
        case["values"][cr["name"]] = v
        inside = not math.isnan(v) and (lo is None or v >= lo) and (hi is None or v <= hi)
        row.update(value=v, result="pass" if inside else "fail")
        if not inside:
            bounds = " and ".join(x for x in (f"at least {lo!r}" if lo is not None else "",
                                              f"at most {hi!r}" if hi is not None else "") if x)
            row["reason"] = f"{cr['metric']} of {_quantity_name(cr)} is {v!r}, required {bounds}"
            if cr["metric"] == "settling_time" and math.isinf(v):
                y = data.quantity(cr)
                target = data.mean(y, data.start) if cr.get("target", "pre") == "pre" else float(cr["target"])
                row["reason"] += (f" (it is not within {float(cr['tolerance'])!r} of {target!r} at the end of the "
                                  f"run: {float(y[-1])!r})")
            fail(row)
        rows.append(row)
    return case, rows


def run_suite(requirements: Any, *, jobs: int | None = None, out: str | os.PathLike | None = None,
              force: bool = False) -> SuiteResult:
    """Makes every case of the requirement file (each short-circuit ratio
    with each disturbance), runs them as one batch (``openkeito.run_batch``;
    with ``out``, into ``out/cases``) and judges them. With ``out`` the
    table is also written to ``out/suite.csv`` and the cases to
    ``out/suite.json``."""
    req = requirements if isinstance(requirements, Requirements) else load_requirements(requirements)
    doc = req.doc
    sim = doc.get("simulation", {})
    cases: dict[str, dict[str, Any]] = {}
    injection = None
    for name, scr, dist in req.cases():
        test = attach(req.device, req.bus, req.grid(scr), [req.disturbance(dist)], injection_mva=injection,
                      end_time_s=sim.get("end_time_s"), time_step_s=sim.get("time_step_s"))
        injection = test.injection_mva
        _add_outputs(test.model, doc["criteria"])
        cases[name] = test.model
    batch_out = None if out is None else Path(out) / "cases"
    if out is not None:
        Path(out).mkdir(parents=True, exist_ok=True)
        for f in ("suite.csv", "suite.json"):
            if (Path(out) / f).exists() and not force:
                raise FileExistsError(f"{Path(out) / f} exists; pass force=True or another out")
    batch = openkeito.run_batch(cases, batch_out, jobs=jobs, force=force, keep_rows=True)
    judged, rows = [], []
    for name, scr, dist in req.cases():
        case, case_rows = _judge(req, name, scr, dist, batch[name])
        judged.append(case)
        rows += case_rows
    suite = SuiteResult(requirements=req, cases=judged, rows=rows, batch=batch)
    if out is not None:
        (Path(out) / "suite.csv").write_text(suite.table_csv(), encoding="utf-8", newline="\n")
        (Path(out) / "suite.json").write_text(suite.to_json(), encoding="utf-8", newline="\n")
    return suite
