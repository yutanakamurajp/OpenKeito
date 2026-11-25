"""Buck converter against its exact solution: switching at grid points and
switching instants within the time step.

The converter of examples/buck.json (100 V, L = 1 mH, C = 100 uF, R = 10 ohm,
triangular carrier at 10 kHz) with the duty 0.405, whose comparator crossings
fall at 20.25 and 79.75 us of every 100 us period: between the grid points of
every step used here. The switch and the diode are resistors of two values
(1 mohm, 1 Mohm), as in the solver.

The reference is the exact solution of that piecewise-linear circuit for the
ideal instants: between two instants the circuit is x' = A x + b with
x = (i_L, v_C), solved with the matrix exponential; the gate changes at the
crossings of duty and carrier, and the diode would turn off at the zero of
the inductor current (it does not in continuous conduction). The largest
difference of i_L over 2 ms is the error.

- "grid": an instant moves to a grid point, by up to one step: first order.
- "interpolated": the step is split at the instant: the error is that of
  the integration method, second order.
"""

from __future__ import annotations

import numpy as np

import lib
import style

TITLE = "Buck converter against the exact solution: switching at grid points and within the step"
E, L, C, R, F, DUTY = 100.0, 1e-3, 1e-4, 10.0, 1e4, 0.405
RON, ROFF = 1e-3, 1e6
X0 = (2.89, 40.5)
END = 2e-3
CASES = [("grid", "2s-dirk", "Grid points, 2S-DIRK"),
         ("interpolated", "2s-dirk", "Within the step, 2S-DIRK"),
         ("interpolated", "trapezoidal", "Within the step, trapezoidal")]


def circuit(method: str, switching: str, h: float, end: float = END) -> dict:
    return lib.model(
        {"time_step_s": h, "end_time_s": end, "method": method, "switching": switching},
        [{"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": E}},
         {"type": "controlled_switch", "name": "S1", "nodes": ["n1", "sw"], "gate": "pwm",
          "on_resistance_ohm": RON, "off_resistance_ohm": ROFF},
         {"type": "diode", "name": "D1", "nodes": ["gnd", "sw"], "on_resistance_ohm": RON,
          "off_resistance_ohm": ROFF},
         {"type": "inductor", "name": "L1", "nodes": ["sw", "out"], "inductance_h": L, "initial_current_a": X0[0]},
         {"type": "capacitor", "name": "C1", "nodes": ["out", "gnd"], "capacitance_f": C,
          "initial_voltage_v": X0[1]},
         {"type": "resistor", "name": "R1", "nodes": ["out", "gnd"], "resistance_ohm": R}],
        [{"type": "branch_current", "component": "L1"}, {"type": "node_voltage", "node": "out"}],
        controls=[{"type": "constant", "name": "duty", "value": DUTY},
                  {"type": "triangle_wave", "name": "carrier", "frequency_hz": F},
                  {"type": "comparator", "name": "pwm", "inputs": ["duty", "carrier"]}])


def system(s_on: bool, d_on: bool) -> tuple[np.ndarray, np.ndarray]:
    """A and b of x' = A x + b. KCL at the switch node gives
    v_sw = (g_S E - i_L) / (g_S + g_D)."""
    gs = 1 / RON if s_on else 1 / ROFF
    gd = 1 / RON if d_on else 1 / ROFF
    gt = gs + gd
    a = np.array([[-1 / (L * gt), -1 / L], [1 / C, -1 / (R * C)]])
    return a, np.array([gs * E / (gt * L), 0.0])


def advance(s: bool, x: np.ndarray, dt: float) -> np.ndarray:
    """The state after dt with S1 in the state s; the diode conducts while
    S1 is off and the current is positive, and turns off at its zero."""
    d = (not s) and x[0] > 0.0
    a, b = system(s, d)
    nxt = lib.propagate(a, b, x, dt)
    if not (d and nxt[0] < 0.0):
        return nxt
    lo, hi = 0.0, dt
    for _ in range(80):
        mid = 0.5 * (lo + hi)
        if lib.propagate(a, b, x, mid)[0] > 0.0:
            lo = mid
        else:
            hi = mid
    a2, b2 = system(False, False)
    return lib.propagate(a2, b2, lib.propagate(a, b, x, hi), dt - hi)


def gate_instants(end: float) -> list[tuple[float, bool]]:
    """(time, state after it): S1 is on while duty > carrier, and the
    carrier starts at its minimum, so S1 is on from (k - D/2)/f to
    (k + D/2)/f."""
    out, k = [], 0
    while True:
        off, on = (k + 0.5 * DUTY) / F, (k + 1 - 0.5 * DUTY) / F
        if off > end:
            return out
        out.append((off, False))
        if on <= end:
            out.append((on, True))
        k += 1


def ideal(h: float, steps: int) -> np.ndarray:
    """The exact solution at the grid points k h, k = 0 .. steps."""
    gates = gate_instants((steps + 1) * h)
    nxt, s, x = 0, True, np.array(X0)
    out = [x]
    for k in range(steps):
        t, t1 = k * h, (k + 1) * h
        while nxt < len(gates) and gates[nxt][0] <= t1:
            tg, state = gates[nxt]
            if tg > t:
                x = advance(s, x, tg - t)
                t = tg
            s = state
            nxt += 1
        if t1 > t:
            x = advance(s, x, t1 - t)
        out.append(x)
    return np.array(out)


def make(ctx: lib.Context) -> dict:
    # 0.25 us is left out: the crossings would fall on grid points.
    steps = [10e-6, 5e-6, 2e-6] if ctx.quick else [10e-6, 5e-6, 2.5e-6, 2e-6, 1e-6, 0.5e-6]
    rows, errors, stats, runs = [], {}, {}, {}
    for switching, method, _ in CASES:
        e = []
        for h in steps:
            r = lib.run(circuit(method, switching, h))
            runs[(switching, method, h)] = r
            x = ideal(h, len(r.time) - 1)
            ei = float(np.max(np.abs(r["I(L1)"] - x[:, 0])))
            ev = float(np.max(np.abs(r["V(out)"] - x[:, 1])))
            solver = r.summary["solver"]
            rows.append((switching, method, h, ei, ev, solver["instants_within_steps"], solver["split_steps"],
                         solver["factorizations"], solver["solves"]))
            e.append(ei)
            stats[(switching, method, h)] = solver
        errors[(switching, method)] = e
    lib.write_csv(ctx, "buck_switching.csv",
                  ["switching", "method", "time_step_s", "max_error_i_l_a", "max_error_v_c_v",
                   "instants_within_steps", "split_steps", "factorizations", "solves"], rows)

    # Waveforms of one carrier period: 1 us at grid points, 10 us within the
    # step, and the exact solution on a fine grid.
    h_fine = 0.25e-6
    t0, t1 = 1.0e-3, 1.1e-3
    fine = ideal(h_fine, int(round(END / h_fine)))
    tf = np.arange(len(fine)) * h_fine
    grid = runs.get(("grid", "2s-dirk", 1e-6)) or lib.run(circuit("2s-dirk", "grid", 1e-6))
    split = runs[("interpolated", "2s-dirk", 10e-6)]

    def window(t, y):
        keep = (t >= t0 - 1e-12) & (t <= t1 + 1e-12)
        return t[keep], y[keep]

    wave = []
    for name, t, i in (("exact", tf, fine[:, 0]), ("grid_1us", grid.time, grid["I(L1)"]),
                       ("interpolated_10us", split.time, split["I(L1)"])):
        tw, iw = window(t, i)
        wave += [(name, float(a), float(b)) for a, b in zip(tw, iw)]
    lib.write_csv(ctx, "buck_switching_waveform.csv", ["series", "time_s", "i_l_a"], wave)

    fig, (ax1, ax2) = style.figure(2, 1, width="double", height_in=3.1)
    hs = np.array(steps) * 1e6
    for k, (switching, method, label) in enumerate(CASES):
        ax1.loglog(hs, errors[(switching, method)], label=label, **style.series(k))
    e_split = errors[("interpolated", "2s-dirk")]
    ax1.loglog(hs, 0.25 * e_split[0] * (hs / hs[0]) ** 2, label="Slope 2", **style.reference(dotted=True))
    e_grid = errors[("grid", "2s-dirk")]
    ax1.loglog(hs, 3 * e_grid[0] * (hs / hs[0]), label="Slope 1", **style.reference())
    style.step_axis(ax1, hs, ylabel="Maximum error of $i_L$ (A)", ticks=[0.5, 1, 2, 5, 10])
    style.legend_below(ax1)
    style.panel(ax1, "a")

    def window_us(t, y):
        tw, yw = window(t, y)
        return (tw - t0) * 1e6, yw

    ax2.plot(*window_us(tf, fine[:, 0]), label="Exact", **style.reference())
    ax2.plot(*window_us(grid.time, grid["I(L1)"]), label="Grid points, 1 µs", **style.series(0, marker=False))
    ax2.plot(*window_us(split.time, split["I(L1)"]), label="Within the step, 10 µs", **style.series(1, line=False))
    ax2.set_xlabel("Time within a carrier period (µs)")
    ax2.set_ylabel("Inductor current (A)")
    style.legend_below(ax2)
    style.panel(ax2, "b")
    lib.save_figure(ctx, fig, "buck_switching")

    out = {}
    for switching, method, _ in CASES:
        e = errors[(switching, method)]
        out[f"{switching}, {method}: max error of i_L (A) at {[f'{h * 1e6:g}' for h in steps]} us"] = [
            lib.sig(x, 4) for x in e]
    if not ctx.quick:
        k1, k05, k10 = steps.index(1e-6), steps.index(0.5e-6), steps.index(10e-6)
        out["interpolated, 2s-dirk: error ratio 1 us / 0.5 us"] = round(e_split[k1] / e_split[k05], 3)
        out["grid, 2s-dirk: error ratio 1 us / 0.5 us"] = round(e_grid[k1] / e_grid[k05], 3)
        out["grid at 1 us / interpolated at 10 us"] = round(e_grid[k1] / e_split[k10])
        s = stats[("interpolated", "2s-dirk", 10e-6)]
        out["interpolated at 10 us: instants within steps, split steps of 200"] = [
            s["instants_within_steps"], s["split_steps"]]
    return out
