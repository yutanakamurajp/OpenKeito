"""From a schedule to EMT runs: time slices of an optimal power flow, and
the transition from one schedule time to the next.

Network: case9 of MATPOWER as shipped with pandapower (60 Hz, 345 kV).

(a), (b) Time slices. The loads follow a daily shape (0.6 to 1.0 of their
case values, 24 hours); pandapower's AC optimal power flow gives the
generator powers and voltages of every hour, which become a schedule file.
Every hour is converted to an EMT model that starts in the steady state of
its power flow, and a three-phase fault at bus 4 from 20 to 70 ms is added:
24 runs of 0.12 s, executed together. The batch is run twice, on one thread
and on four, and every value of every column is compared: the results are
the same bit for bit. The voltage of a bus is the magnitude
sqrt(v_a^2 + v_b^2 + v_c^2) / V_n of the three instantaneous phase voltages
(1 pu in a balanced steady state at the rated voltage).

(c), (d) Transition. Loads at 0.8, 0.9 and 1.0 of their case values at three
schedule times, with the set points of the AC optimal power flow. The model
of a transition starts from the power flow of the first time and moves the
set points of the machines (with an excitation system and a governor), the
loads and the source to those of the next time in a ramp of 2 s. The end of
the run is compared with the power flow of the second time.
"""

from __future__ import annotations

import math
import time
from datetime import datetime, timedelta, timezone

import numpy as np

import lib
import style

TITLE = "Time slices of an OPF schedule run in parallel, and a transition between two schedule times (case9)"
JST = timezone(timedelta(hours=9))
FAULT_BUS, WATCHED_BUS = 4, 7
T_ON, T_OFF, END = 0.02, 0.07, 0.12
#: A governor that is fast against the swings of the machines, as in the
#: tests of the end of a transition (python/tests/test_transition.py).
FAST = {"governor_time_constant_s": 0.02, "turbine_time_constant_s": 0.03}
TRANSITION_TIMES = ["2026-11-21T00:00:00+09:00", "2026-11-21T01:00:00+09:00", "2026-11-21T02:00:00+09:00"]


def day_plan(net, hours: int):
    from openkeito.pandapower import schedule_from_opf
    from openkeito.schedule import Schedule

    times = [datetime(2026, 11, 21, h, tzinfo=JST) for h in range(hours)]
    shape = [0.8 - 0.2 * math.cos(2 * math.pi * (h - 3) / 24) for h in range(hours)]
    values = {}
    for i in net.load.index:
        values[(f"load/{i}", "active_power_mw")] = [net.load.p_mw[i] * f for f in shape]
        values[(f"load/{i}", "reactive_power_mvar")] = [net.load.q_mvar[i] * f for f in shape]
    plan, notes = schedule_from_opf(net, Schedule.from_values(times, values, name="day"), opf="ac")
    return plan, shape, notes


def magnitude(result, bus: str, vn_kv: float) -> np.ndarray:
    return np.sqrt(sum(result[f"V({bus}.{p})"] ** 2 for p in "abc")) / (vn_kv * 1e3)


def identical(a, b) -> tuple[bool, int]:
    """Whether two batch results hold the same bits, and how many values
    were compared."""
    same, count = True, 0
    for name, x in a.cases.items():
        y = b[name]
        same &= x.columns == y.columns and np.array_equal(x.time, y.time)
        for c in x.columns:
            same &= bool(np.array_equal(x[c], y[c]))
            count += len(x[c])
    return bool(same), count


def make(ctx: lib.Context) -> dict:
    try:
        import pandapower.networks as pn

        from openkeito.pandapower import fault, run_time_slices, run_transitions, schedule_from_opf
        from openkeito.schedule import Schedule
    except ImportError:
        return {"skipped": "pandapower is not installed"}

    # Time slices ----------------------------------------------------------
    hours = 4 if ctx.quick else 24
    net = pn.case9()
    plan, shape, notes = day_plan(net, hours)
    disturbances = [fault(FAULT_BUS, T_ON, T_OFF)]
    one = run_time_slices(net, plan, disturbances, end_time_s=END, jobs=1, keep_rows=True)
    four = run_time_slices(net, plan, disturbances, end_time_s=END, jobs=4, keep_rows=True)
    same, compared = identical(one.batch, four.batch)
    vn = float(net.bus.vn_kv[WATCHED_BUS])
    series = {(s["target"], s["quantity"]): s["values"] for s in plan.series}
    generators = [t for t, q in series if q == "active_power_mw" and t.startswith("gen/")]
    rows = []
    for k, (row, s) in enumerate(zip(one.rows, one.slices)):
        r = one.batch[s.name]
        assert row["status"] == "completed", row
        v = magnitude(r, s.conversion.bus(WATCHED_BUS), vn)
        t = r.time
        during = (t > T_ON + 0.02) & (t <= T_OFF)
        rows.append([k, row["time"], shape[k], sum(float(net.load.p_mw[i]) * shape[k] for i in net.load.index)]
                    + [series[(g, "active_power_mw")][k] for g in generators]
                    + [row["max_abs_dv_pu"], row["max_abs_dva_degree"], row["relative_voltage_deviation"],
                       float(v[0]), float(np.min(v[during])), float(np.max(v[t > T_OFF])), row["warnings"]])
    lib.write_csv(ctx, "schedule_time_slices.csv",
                  ["slice", "time", "load_factor", "load_mw"] + [f"{g.replace('/', '')}_opf_mw" for g in generators]
                  + ["start_max_dv_pu", "start_max_dangle_deg", "relative_voltage_deviation_before_fault",
                     f"v_bus{WATCHED_BUS}_start_pu", f"v_bus{WATCHED_BUS}_min_during_fault_pu",
                     f"v_bus{WATCHED_BUS}_max_after_clearing_pu", "warnings"], rows)

    # Transition -----------------------------------------------------------
    net2 = pn.case9()
    factors = (0.8, 0.9, 1.0)
    values = {}
    for i, row in net2.load.iterrows():
        values[(f"load/{i}", "active_power_mw")] = [row.p_mw * f for f in factors]
        values[(f"load/{i}", "reactive_power_mvar")] = [row.q_mvar * f for f in factors]
    plan2, _ = schedule_from_opf(net2, Schedule.from_values(TRANSITION_TIMES, values, name="loads"))
    settle = 3.0 if ctx.quick else 10.0
    options = {"start_time_s": 0.5, "ramp_time_s": 2.0, "settle_time_s": settle, "governor": FAST}
    tr = run_transitions(net2, plan2, jobs=1, **options)
    tr_parallel = run_transitions(net2, plan2, jobs=2, **options)
    same_transition, _ = identical(tr.batch, tr_parallel.batch)
    keys = ["case", "from_time", "to_time", "status", "max_abs_dv_pu", "max_abs_dva_degree",
            "drift_before_change_pu", "end_max_abs_dv_pu", "end_max_abs_dva_degree", "end_max_abs_dp_mw",
            "end_frequency_hz", "frequency_min_hz", "frequency_max_hz", "voltage_min_pu", "voltage_max_pu",
            "max_machine_current_pu", "events", "warnings"]
    lib.write_csv(ctx, "schedule_transitions.csv", keys, [[r[k] for k in keys] for r in tr.rows])
    first = tr.transitions[0]
    r = tr.batch[tr.rows[0]["case"]]
    assert r.status == "completed"
    machines = {c["name"]: c["rated_power_mva"] for c in first.model["components"]
                if c["type"] == "synchronous_machine"}
    p_series = {(s["target"], s["quantity"]): s["values"] for s in plan2.series}
    f_hz = float(net2.f_hz)
    wave_header, wave_columns = ["time_s"], [r.time]
    for name, rating in machines.items():
        wave_header += [f"{name}_p_mw", f"{name}_frequency_hz"]
        wave_columns += [r[f"S({name}_p)"] * rating, r[f"S({name}_speed)"] * f_hz]
    lib.write_csv(ctx, "schedule_transition_waveform.csv", wave_header, zip(*wave_columns))

    # Figure ---------------------------------------------------------------
    fig, axes = style.figure(2, 2, width="double", height_in=4.6)
    (ax1, ax2), (ax3, ax4) = axes
    hour = np.arange(hours)
    ax1.plot(hour, [x[3] for x in rows], label="Load", **style.reference())
    for k, g in enumerate(generators):
        ax1.plot(hour, series[(g, "active_power_mw")], label=f"Generator {k + 1} (OPF)", **style.series(k))
    ax1.set_xlabel("Hour of the schedule")
    ax1.set_ylabel("Active power (MW)")
    ax1.legend()
    style.panel(ax1, "a")
    ax2.plot(hour, [x[-3] for x in rows], label="Lowest during the fault", **style.series(0))
    ax2.plot(hour, [x[-2] for x in rows], label="Highest after the clearing", **style.series(1))
    ax2.plot(hour, [x[-4] for x in rows], label="Before the fault (power flow)", **style.reference(dotted=True))
    ax2.set_xlabel("Hour of the schedule (one EMT run each)")
    ax2.set_ylabel(f"Voltage of bus {WATCHED_BUS} (pu)")
    ax2.legend()
    style.panel(ax2, "b")
    for k, (name, rating) in enumerate(machines.items()):
        element = f"gen/{name[3:]}"
        ax3.plot(r.time, r[f"S({name}_p)"] * rating, label=f"Generator {k + 1}", **style.series(k, marker=False))
        target = p_series[(element, "active_power_mw")][1]
        ax3.plot([r.time[0], r.time[-1]], [target, target], **style.reference(dotted=True),
                 label="Power flow of the next time" if k == 0 else None)
        ax4.plot(r.time, r[f"S({name}_speed)"] * f_hz, label=f"Generator {k + 1}", **style.series(k, marker=False))
    ax3.set_xlabel("Time (s)")
    ax3.set_ylabel("Active power (MW)")
    ax3.legend(loc="center right")
    style.panel(ax3, "c")
    ax4.set_xlabel("Time (s)")
    ax4.set_ylabel("Rotor speed (Hz)")
    ax4.ticklabel_format(axis="y", useOffset=False)
    ax4.legend()
    style.panel(ax4, "d")
    lib.save_figure(ctx, fig, "schedule_runs")

    out = {
        "time slices: cases, rows per case": [len(one.rows), len(one.batch[one.slices[0].name].time)],
        "time slices: 1 thread and 4 threads identical bit for bit": same,
        "time slices: values compared": compared,
        "time slices: largest start difference from the power flow (pu, deg)": [
            lib.sig(max(x[-7] for x in rows), 2), lib.sig(max(x[-6] for x in rows), 2)],
        "time slices: largest departure before the fault (relative)": lib.sig(max(x[-5] for x in rows), 2),
        f"time slices: lowest voltage of bus {WATCHED_BUS} during the fault, range over the day (pu)": [
            round(min(x[-3] for x in rows), 4), round(max(x[-3] for x in rows), 4)],
        "time slices: notes of the OPF schedule": notes,
        "transitions: 1 thread and 2 threads identical bit for bit": same_transition,
    }
    for row in tr.rows:
        out[f"transition {row['case']}: start |dV| pu; drift before the change; end |dV| pu, angle deg, dP MW"] = [
            lib.sig(row[k], 2) for k in ("max_abs_dv_pu", "drift_before_change_pu", "end_max_abs_dv_pu",
                                         "end_max_abs_dva_degree", "end_max_abs_dp_mw")]
        out[f"transition {row['case']}: frequency min, max, end (Hz)"] = [
            round(row[k], 5) for k in ("frequency_min_hz", "frequency_max_hz", "end_frequency_hz")]

    if ctx.measure:
        # Wall-clock time of the whole batch (conversion included in neither).
        from openkeito.pandapower import time_slices
        from openkeito.convert import run_slices

        slices = time_slices(net, plan, disturbances, end_time_s=END)
        measured = []
        for jobs in (1, 2, 4, 8):
            best = math.inf
            for _ in range(5):
                start = time.perf_counter()
                run_slices(slices, jobs=jobs)
                best = min(best, time.perf_counter() - start)
            measured.append((jobs, len(slices), best, len(slices) / best))
        lib.write_csv(ctx, "schedule_runs_measured.csv", ["jobs", "cases", "best_wall_s", "cases_per_s"], measured)
        out["measured here: cases per second with 1, 2, 4, 8 jobs"] = [round(m[3], 1) for m in measured]
    return out
