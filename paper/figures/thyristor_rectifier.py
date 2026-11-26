"""Six-pulse thyristor converter against the textbook (issue #49).

A 400 V, 50 Hz source with L_s = 1 mH per phase (phase a is
sqrt(2/3) V_LL sin(w t)), six thyristors fired by pulses of 120 deg that start
alpha after the natural commutation, and on the DC side a resistance, a
1 H smoothing inductor starting at I_d = 50 A and, for the inverter, a
source. Thyristors of 1 mohm and 1 Mohm, as in the solver.

The reference is the textbook (Mohan, Undeland, Robbins, "Power
Electronics", 3rd ed., 2003, ch. 6): the average DC voltage with
commutation overlap

    U_d = (3 sqrt 2/pi) V_LL cos(alpha) - (3/pi) w L_s I_d - 2 R_on I_d

(the last term: two conducting thyristors), and the overlap angle u from
cos(alpha) - cos(alpha + u) = 2 w L_s I_d/(sqrt 2 V_LL). U_d of a run is the
average over 40 to 60 ms from the current alone, R mean(i) + L (i(t1) -
i(t0))/(t1 - t0) + e, with the formula at the mean current.

- "grid": the firing waits for the first grid point at which the gate is
  >= 0.5, half a step on average: an error of first order.
- "interpolated": the firing lies at the instant of the gate, the end of
  the commutation at the zero of the current.
"""

from __future__ import annotations

import math

import numpy as np

import lib
import style

TITLE = "Six-pulse thyristor converter against the textbook average and overlap"
F, V_LL, LS, ID, LD, RON = 50.0, 400.0, 1e-3, 50.0, 1.0, 1e-3
W = 2 * math.pi * F
T0, T1 = 0.04, 0.06
ENDS = [("ac.a", "p"), ("n", "ac.c"), ("ac.b", "p"), ("n", "ac.a"), ("ac.c", "p"), ("n", "ac.b")]


def formula(alpha_deg: float, i: float) -> float:
    return (3 * math.sqrt(2) / math.pi * V_LL * math.cos(math.radians(alpha_deg)) - 3 / math.pi * W * LS * i
            - 2 * RON * i)


def overlap(alpha_deg: float, i: float) -> float:
    a = math.radians(alpha_deg)
    return (math.acos(math.cos(a) - 2 * W * LS * i / (math.sqrt(2) * V_LL)) - a) / W


def load(alpha_deg: float) -> tuple[float, float]:
    """Resistance and source of the DC side that hold I_d."""
    u = formula(alpha_deg, ID)
    return (u / ID, 0.0) if u > 0 else (1.0, u - ID)


def circuit(h: float, alpha_deg: float, switching: str) -> dict:
    rd, e = load(alpha_deg)
    components = [{"type": "three_phase_source", "name": "G1", "bus": "ac", "line_voltage_rms_v": V_LL,
                   "frequency_hz": F, "angle_deg": -90.0,
                   "impedance": {"type": "sequence", "r1_ohm": 0.0, "l1_h": LS, "r0_ohm": 0.0, "l0_h": LS}}]
    controls = [{"type": "constant", "name": "half", "value": 0.5}]
    for k, (a, c) in enumerate(ENDS):
        start = 30.0 + 60.0 * k + alpha_deg
        controls += [{"type": "sine_wave", "name": f"s{k + 1}", "amplitude": 1.0, "frequency_hz": F,
                      "phase_deg": 30.0 - start},
                     {"type": "comparator", "name": f"g{k + 1}", "inputs": [f"s{k + 1}", "half"]}]
        components.append({"type": "thyristor", "name": f"T{k + 1}", "nodes": [a, c], "gate": f"g{k + 1}",
                           "on_resistance_ohm": RON, "off_resistance_ohm": 1e6})
    components += [{"type": "resistor", "name": "RD", "nodes": ["p", "m"], "resistance_ohm": rd},
                   {"type": "inductor", "name": "LD", "nodes": ["m", "q"], "inductance_h": LD, "initial_current_a": ID},
                   {"type": "voltage_source", "name": "E", "nodes": ["q", "n"], "waveform": {"type": "dc", "value": e}}]
    return lib.model({"time_step_s": h, "end_time_s": T1, "switching": switching}, components,
                     [{"type": "branch_current", "component": "LD"}], controls=controls, version="0.9")


def average(r, alpha_deg: float) -> tuple[float, float]:
    rd, e = load(alpha_deg)
    t, i = r.time, r["I(LD)"]
    keep = (t >= T0 - 1e-12) & (t < T1 - 1e-12)
    mean = float(np.mean(i[keep]))
    k0, k1 = int(round(T0 / (t[1] - t[0]))), int(round(T1 / (t[1] - t[0])))
    return rd * mean + LD * (i[k1] - i[k0]) / (T1 - T0) + e, mean


def overlaps(r) -> list[float]:
    events = [e for e in r.summary.get("all_events", r.summary.get("events", []))
              if e["time_s"] > T0 and e["component"] in ("T1", "T3", "T5")]
    return [b["time_s"] - a["time_s"] for a, b in zip(events, events[1:])
            if a["action"] == "close" and b["action"] == "open"]


def make(ctx: lib.Context) -> dict:
    alphas = [15.0, 60.0, 150.0] if ctx.quick else [15.0, 30.0, 45.0, 60.0, 120.0, 150.0]
    rows, deviations = [], {}
    for alpha in alphas:
        for switching in ("interpolated", "grid"):
            r = lib.run(circuit(2e-5, alpha, switching))
            u, i = average(r, alpha)
            f = formula(alpha, i)
            o = overlaps(r)
            ou = overlap(alpha, i)
            worst = max((abs(x - ou) for x in o), default=float("nan"))
            rows.append((switching, alpha, 2e-5, u, i, f, (u - f) / abs(f), ou, worst, len(o)))
            deviations[(switching, alpha)] = (u - f, worst)
    lib.write_csv(ctx, "thyristor_rectifier.csv",
                  ["switching", "alpha_deg", "time_step_s", "average_dc_voltage_v", "mean_current_a",
                   "formula_v", "relative_error", "overlap_formula_s", "largest_overlap_deviation_s",
                   "commutations"], rows)

    steps = [4e-5, 2e-5] if ctx.quick else [4e-5, 2e-5, 1e-5, 5e-6]
    order_rows, by_h = [], {}
    for h in steps:
        for switching in ("interpolated", "grid"):
            r = lib.run(circuit(h, 30.0, switching))
            u, i = average(r, 30.0)
            by_h[(switching, h)] = u - formula(30.0, i)
            order_rows.append((switching, h, u - formula(30.0, i)))
    lib.write_csv(ctx, "thyristor_rectifier_steps.csv", ["switching", "time_step_s", "error_v"], order_rows)

    fig, (ax1, ax2) = style.figure(2, 1, width="double", height_in=2.9)
    for k, (switching, label) in enumerate((("interpolated", "Within the step, 20 µs"),
                                            ("grid", "Grid points, 20 µs"))):
        ax1.semilogy(alphas, [abs(r[6]) for r in rows if r[0] == switching], label=label, **style.series(k))
    ax1.set_xlabel("Firing angle α (deg)")
    ax1.set_ylabel("|Error of $U_d$| / $|U_d|$")
    style.legend_below(ax1)
    style.panel(ax1, "a")
    hs = np.array(steps) * 1e6
    for k, (switching, label) in enumerate((("interpolated", "Within the step"), ("grid", "Grid points"))):
        ax2.loglog(hs, [abs(by_h[(switching, h)]) for h in steps], label=label, **style.series(k))
    ax2.loglog(hs, abs(by_h[("grid", steps[0])]) * hs / hs[0], label="Slope 1", **style.reference(dotted=True))
    style.step_axis(ax2, hs, ylabel="|Error of $U_d$| at α = 30° (V)", ticks=[5, 10, 20, 40] if not ctx.quick else [20, 40])
    style.legend_below(ax2)
    style.panel(ax2, "b")
    lib.save_figure(ctx, fig, "thyristor_rectifier")

    out: dict = {}
    for switching in ("interpolated", "grid"):
        out[f"{switching}, 20 us: relative error of U_d at alpha {[f'{a:g}' for a in alphas]}"] = [
            lib.sig(r[6], 3) for r in rows if r[0] == switching]
        out[f"{switching}, 20 us: largest overlap deviation (s)"] = lib.sig(
            max(r[8] for r in rows if r[0] == switching), 3)
        out[f"{switching}, alpha 30: error of U_d (V) at {[f'{h * 1e6:g}' for h in steps]} us"] = [
            lib.sig(by_h[(switching, h)], 4) for h in steps]
    return out
