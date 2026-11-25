"""Order of the two integration methods on a linear circuit.

The series RLC circuit of examples/rlc.json (1 V dc applied at t = 0 to
R = 10 ohm, L = 10 mH, C = 1 uF: underdamped, 1.59 kHz, decay 500 1/s) has
the closed form

    v_C(t) = E (1 - e^{-a t} (cos w_d t + (a / w_d) sin w_d t)),
    i(t)   = E / (w_d L) e^{-a t} sin w_d t,

with a = R / 2L and w_d = sqrt(1/LC - a^2). The largest error over the run
is taken for steps that are halved; the slope of the error is the order.
"""

from __future__ import annotations

import math

import numpy as np

import lib
import style

TITLE = "Error and order of 2S-DIRK and the trapezoidal rule on a series RLC circuit"
E, R, L, C = 1.0, 10.0, 0.01, 1e-6
END = 5e-3


def exact(t: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    a = R / (2 * L)
    wd = math.sqrt(1 / (L * C) - a * a)
    decay = np.exp(-a * t)
    v = E * (1 - decay * (np.cos(wd * t) + a / wd * np.sin(wd * t)))
    i = E / (wd * L) * decay * np.sin(wd * t)
    return v, i


def circuit(method: str, h: float) -> dict:
    return lib.model(
        {"time_step_s": h, "end_time_s": END, "method": method},
        [{"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": E}},
         {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": R},
         {"type": "inductor", "name": "L1", "nodes": ["n2", "n3"], "inductance_h": L},
         {"type": "capacitor", "name": "C1", "nodes": ["n3", "gnd"], "capacitance_f": C}],
        [{"type": "node_voltage", "node": "n3"}, {"type": "branch_current", "component": "L1"}])


def make(ctx: lib.Context) -> dict:
    steps = [40e-6, 20e-6, 10e-6] if ctx.quick else [40e-6, 20e-6, 10e-6, 5e-6, 2.5e-6, 1.25e-6]
    rows, errors = [], {}
    for method, _ in lib.METHODS:
        ev, ei = [], []
        for h in steps:
            r = lib.run(circuit(method, h))
            v, i = exact(r.time)
            ev.append(float(np.max(np.abs(r["V(n3)"] - v))))
            ei.append(float(np.max(np.abs(r["I(L1)"] - i))))
        ov, oi = [float("nan")] + lib.order(ev), [float("nan")] + lib.order(ei)
        errors[method] = (ev, ei, ov, oi)
        rows += [(method, h, ev[k], ov[k], ei[k], oi[k]) for k, h in enumerate(steps)]
    lib.write_csv(ctx, "integration_order.csv",
                  ["method", "time_step_s", "max_error_v_c_v", "order_v_c", "max_error_i_l_a", "order_i_l"], rows)

    fig, ax = style.figure()
    hs = np.array(steps) * 1e6
    for k, (method, label) in enumerate(lib.METHODS):
        ax.loglog(hs, errors[method][0], label=label, **style.series(k))
    # A slope of 2 above the errors of the trapezoidal rule.
    ref = errors["trapezoidal"][0][0] * 3
    ax.loglog(hs, ref * (hs / hs[0]) ** 2, label="Slope 2", **style.reference(dotted=True))
    style.step_axis(ax, hs, ylabel="Maximum error of $v_C$ (V)")
    ax.legend()
    lib.save_figure(ctx, fig, "integration_order")

    out = {}
    for method, _ in lib.METHODS:
        ev, ei, ov, oi = errors[method]
        out[f"{method}: max error of v_C at {steps[2] * 1e6:g} us (V)"] = lib.sig(ev[2], 4)
        out[f"{method}: order of v_C at the smallest steps"] = round(ov[-1], 3)
        out[f"{method}: order of i_L at the smallest steps"] = round(oi[-1], 3)
    out["ratio of the errors of v_C, trapezoidal / 2S-DIRK, at the smallest step"] = round(
        errors["trapezoidal"][0][-1] / errors["2s-dirk"][0][-1], 2)
    return out
