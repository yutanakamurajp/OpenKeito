"""Grid-connection tests from a requirement file (issues #53 to #55).

examples/grid_test/requirements.json: the converter of
examples/grid_test/converter_device.json (2 MVA, 0.8 per unit of active
power) behind grid equivalents of short-circuit ratio 10, 5, 3, 2, 1.5 and
1.2 (X/R 10), each with a three-phase fault for 150 ms that leaves 0.2 per
unit, a phase jump of +30 deg and a voltage step of -10 % at 0.2 s: 18 cases
run as one batch (openkeito.requirements.run_suite) and judged by four
criteria (recovery of the active power, its swing at the end, an envelope of
the positive-sequence voltage, the THD of the current). The limits are
example values of the user, not thresholds of a grid code.

The table of the judgement is the same, byte for byte, whatever the number
of jobs (python/tests/test_requirements.py). A second run of the same file
with the phase-locked loop at 80 Hz (default 20 Hz) shows a converter that
is unstable behind the weakest grids.
"""

from __future__ import annotations

import copy
import json

import numpy as np

import lib
import style
from openkeito.requirements import load_requirements, run_suite

TITLE = "Grid-connection tests: 18 cases of a requirement file and their judgement"
EXAMPLE = lib.EXAMPLES_DIR / "grid_test" / "requirements.json"
SHOWN = [10.0, 3.0, 1.5, 1.2]
DISTURBANCES = [("fault", "Fault 150 ms, 0.2 pu left"), ("jump", "Phase jump +30°"), ("dip", "Voltage step −10 %")]


def make(ctx: lib.Context) -> dict:
    req = load_requirements(EXAMPLE)
    suite = run_suite(req, jobs=1)
    lib.write_csv(ctx, "grid_tests.csv", ["case", "scr", "disturbance", "criterion", "value", "min", "max", "result"],
                  [(r["case"], r["scr"], r["disturbance"], r["criterion"], r.get("value"), r.get("min"),
                    r.get("max"), r["result"]) for r in suite.rows])

    names = {(c["scr"], c["disturbance"]): c["name"] for c in suite.cases}
    wave_rows = []
    fig, axes = style.figure(3, 1, width="double", height_in=2.3, sharey=True)
    for k, (dist, label) in enumerate(DISTURBANCES):
        ax = axes[k]
        for j, scr in enumerate(SHOWN):
            r = suite.batch[names[(scr, dist)]]
            t, p = r.time, r["S(PV1.p)"]
            keep = (t >= 0.15) & (t <= 0.8)
            ax.plot(t[keep], p[keep], label=f"SCR {scr:g}", **style.series(j, marker=False))
            sel = keep & (np.arange(len(t)) % 20 == 0)
            wave_rows += [(dist, scr, float(a), float(b)) for a, b in zip(t[sel], p[sel])]
        ax.set_xlabel("Time (s)")
        ax.set_title(label, fontsize=None)
        style.panel(ax, "abc"[k])
    axes[0].set_ylabel("Active power (pu)")
    style.legend_below(axes[1], ncols=4)
    lib.save_figure(ctx, fig, "grid_tests")
    lib.write_csv(ctx, "grid_tests_waveforms.csv", ["disturbance", "scr", "time_s", "active_power_pu"], wave_rows)

    table = []
    for scr in req.doc["scr"]:
        cells = [f"{float(scr):g}"]
        for dist, _ in DISTURBANCES:
            case = next(c for c in suite.cases if c["name"] == names[(float(scr), dist)])
            settle = case["values"].get("p_recovery", float("inf"))
            text = "∞" if not np.isfinite(settle) else f"{settle * 1e3:.0f} ms"
            cells.append(("pass" if case["passed"] else "fail: " + ", ".join(f["criterion"] for f in case["failures"]))
                         + f" ({text})")
        table.append(cells)
    lib.write_table(ctx, "grid_tests_table", ["SCR", *[label for _, label in DISTURBANCES]], table,
                    "Judgement of the 18 cases of examples/grid_test/requirements.json (in brackets: time for "
                    "the active power to stay within 0.05 pu of its value before the disturbance, after its end). "
                    "The limits are example values, not thresholds of a grid code.")

    unstable = copy.deepcopy(req.doc)
    device = json.loads(json.dumps(req.device))
    device["components"][0]["pll_natural_frequency_hz"] = 80.0
    unstable["device"]["model"] = device
    unstable["scr"] = [10, 1.2]
    loose = run_suite(unstable, jobs=1)

    worst = {dist: max(c["values"]["p_recovery"] for c in suite.cases
                       if c["disturbance"] == dist and np.isfinite(c["values"].get("p_recovery", np.inf)))
             for dist, _ in DISTURBANCES}
    return {
        "cases, passed, failed": [suite.counts["cases"], suite.counts["passed"], suite.counts["failed"]],
        "failed cases": [c["name"] for c in suite.cases if not c["passed"]],
        "largest finite recovery time (s) by disturbance": {k: lib.sig(v, 3) for k, v in worst.items()},
        "PLL 80 Hz: failed cases": [c["name"] for c in loose.cases if not c["passed"]],
        "PLL 80 Hz, SCR 1.2: largest swing of P in the last 0.1 s (pu)": lib.sig(
            max(c["values"]["stable"] for c in loose.cases if c["scr"] == 1.2), 3),
    }
