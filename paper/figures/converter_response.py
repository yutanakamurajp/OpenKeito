"""Averaged grid-following converter: step of the current reference and the
current limit during a fault.

(a) examples/converter.json: a 2 MVA converter at a 6.6 kV bus behind a grid
impedance; its active power reference steps from 1.0 to 1.6 MW at 0.1 s. The
PI current control with decoupling and a gain Kp = L/tau, Ki = R/tau cancels
the pole of the reactor, so the closed loop is the first-order lag
1/(tau s + 1) with tau = 1 ms (Yazdani and Iravani 2010, ch. 8). The
reference of the comparison is that lag applied to the recorded current
reference i_ref.d (exactly, for a reference that is linear within a step).
The run differs from it by the dead time of one step between the controls
and the circuit: an error of first order in the step, about h/tau of the
step of the reference at its largest.

(b) examples/converter_voltage_support.json: a three-phase fault through
1.2 ohm at the bus from 0.1 to 0.25 s. The reactive current has priority
and the magnitude of the current reference is limited to 1.1 per unit
(the limit logic of the generic models REGC_A / REEC_A of WECC).
"""

from __future__ import annotations

import math

import numpy as np

import lib
import style

TITLE = "Grid-following converter: step of the current reference and current limit during a fault"
TAU = 1e-3
T_STEP = 0.1
LIMIT = 1.1
T_FAULT, T_CLEAR = 0.1, 0.25


def lag(t: np.ndarray, u: np.ndarray, tau: float) -> np.ndarray:
    """y of tau y' + y = u for u linear between the rows, from y = u at
    the first row."""
    y = np.empty_like(u)
    y[0] = u[0]
    for k in range(1, len(u)):
        h = t[k] - t[k - 1]
        e = math.exp(-h / tau)
        slope = (u[k] - u[k - 1]) / h
        # Particular solution of a ramp: u(t) - tau * slope.
        y[k] = u[k] - tau * slope + (y[k - 1] - u[k - 1] + tau * slope) * e
    return y


def step_run(h: float) -> dict:
    m = lib.example("converter.json")
    m["simulation"]["time_step_s"] = h
    m["simulation"]["end_time_s"] = 0.13
    return m


def make(ctx: lib.Context) -> dict:
    steps = [50e-6, 25e-6]
    rows, errors, runs = [], {}, {}
    for h in steps:
        r = lib.run(step_run(h))
        t, i_d, i_ref = r.time, r["S(PV1.i_d)"], r["S(PV1.i_ref.d)"]
        expected = lag(t, i_ref, TAU)
        k0 = int(round(T_STEP / h))
        delta = float(i_ref[-1] - i_ref[k0 - 1])
        errors[h] = (float(np.max(np.abs(i_d - expected))), delta, float(np.max(np.abs(i_d[:k0] - i_ref[:k0]))))
        runs[h] = (t, i_d, i_ref, expected)
        rows.append((h, errors[h][0], errors[h][0] / delta, errors[h][2]))
    lib.write_csv(ctx, "converter_step.csv",
                  ["time_step_s", "max_error_i_d_pu", "max_error_relative_to_the_step", "max_error_before_the_step_pu"],
                  rows)
    t, i_d, i_ref, expected = runs[steps[0]]
    keep = (t >= T_STEP - 2e-3) & (t <= T_STEP + 8e-3)
    lib.write_csv(ctx, "converter_step_waveform.csv", ["time_s", "i_ref_d_pu", "i_d_first_order_lag_pu", "i_d_pu"],
                  zip(t[keep], i_ref[keep], expected[keep], i_d[keep]))

    m = lib.example("converter_voltage_support.json")
    if ctx.quick:
        m["simulation"]["end_time_s"] = 0.3
    m["simulation"]["output_interval_steps"] = 4
    f = lib.run(m)
    tf = f.time
    magnitude = np.hypot(f["S(PV1.i_d)"], f["S(PV1.i_q)"])
    reference = np.hypot(f["S(PV1.i_ref.d)"], f["S(PV1.i_ref.q)"])
    lib.write_csv(ctx, "converter_fault.csv",
                  ["time_s", "v_d_pu", "i_d_pu", "i_q_pu", "current_magnitude_pu", "reference_magnitude_pu", "p_pu",
                   "q_pu"],
                  zip(tf, f["S(PV1.v_d)"], f["S(PV1.i_d)"], f["S(PV1.i_q)"], magnitude, reference, f["S(PV1.p)"],
                      f["S(PV1.q)"]))
    settled = (tf >= T_FAULT + 0.05) & (tf <= T_CLEAR - 1e-9)
    last = (tf >= T_CLEAR - 0.01) & (tf <= T_CLEAR - 1e-9)
    at_limit = float(np.max(np.abs(magnitude[last] - LIMIT)))
    reference_at_limit = float(np.max(np.abs(reference[settled] - LIMIT)))
    peak = float(np.max(magnitude))
    after = tf > T_CLEAR
    p0 = float(f["S(PV1.p)"][0])
    away = np.abs(f["S(PV1.p)"] - p0) > 0.01
    back = float(tf[after & away][-1] - T_CLEAR) if np.any(after & away) else 0.0

    fig, (ax1, ax2) = style.figure(2, 1, width="double", height_in=3.3)
    ms = (t[keep] - T_STEP) * 1e3
    ax1.plot(ms, i_ref[keep], label="Reference $i_{d}^{*}$", color=style.GRID, linewidth=1.2)
    ax1.plot(ms, expected[keep], label=r"First-order lag, $\tau$ = 1 ms", **style.reference())
    every = 4
    ax1.plot(ms[::every], i_d[keep][::every], label="OpenKeito, 50 µs", **style.series(0, line=False))
    ax1.set_xlabel("Time after the step (ms)")
    ax1.set_ylabel("Active current $i_d$ (pu)")
    style.legend_below(ax1, ncols=2)
    style.panel(ax1, "a")
    ax2.axhline(LIMIT, label="Limit, 1.1 pu", **style.reference(dotted=True))
    ax2.plot(tf * 1e3, magnitude, label="Current magnitude", **style.series(0, marker=False))
    ax2.plot(tf * 1e3, -f["S(PV1.i_q)"], label="Reactive current $-i_q$", **style.series(1, marker=False))
    ax2.plot(tf * 1e3, f["S(PV1.i_d)"], label="Active current $i_d$", **style.series(2, marker=False))
    ax2.plot(tf * 1e3, f["S(PV1.v_d)"], label="Voltage $v_d$", **style.series(3, marker=False))
    ax2.set_xlabel("Time (ms)")
    ax2.set_ylabel("Per unit of the ratings")
    style.legend_below(ax2, ncols=2)
    style.panel(ax2, "b")
    lib.save_figure(ctx, fig, "converter_response")

    return {
        "step of the current reference (pu)": round(errors[steps[0]][1], 4),
        "max difference from the first-order lag at 50, 25 us (pu)": [
            lib.sig(errors[h][0]) for h in steps],
        "the same relative to the step": [lib.sig(errors[h][0] / errors[h][1]) for h in steps],
        "ratio 50 us / 25 us (first order: 2)": round(errors[steps[0]][0] / errors[steps[1]][0], 2),
        "a dead time of one step moves the lag by h / tau of the step: at 50 us": round(50e-6 / TAU, 4),
        "fault: voltage v_d during the fault (pu)": round(float(np.mean(f["S(PV1.v_d)"][settled])), 3),
        "fault: largest difference of the reference magnitude from 1.1 pu, from 50 ms after the start": lib.sig(
            reference_at_limit, 2),
        "fault: largest difference of the current magnitude from 1.1 pu in the last 10 ms of the fault": lib.sig(
            at_limit, 2),
        "fault: peak of the current magnitude in the run (pu)": round(peak, 3),
        "fault: time after the clearing until p stays within 0.01 pu of its start (s)": round(back, 3),
    }
