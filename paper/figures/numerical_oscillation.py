"""Numerical oscillation after a switch interrupts an inductive current.

A switch in series with R = 1 ohm and L = 1 mH carries 10 A from a 10 V
source and opens at 1 ms although the current is not zero; a resistor of
1 kohm across the switch takes the current over. The current then decays
with L / (R + Rs) = 1 us, a hundredth of the time step of 100 us, so the
exact voltage of the inductor is back at zero at the first grid point after
the opening:

    i(t) = E / (R + Rs) + (i0 - E / (R + Rs)) e^{-(R + Rs) (t - t0) / L},
    v_L(t) = L di/dt.

The trapezoidal rule keeps what is left of such a fast mode with the factor
(1 - x/2) / (1 + x/2) -> -1 per step (x = h (R + Rs) / L): the voltage
alternates in sign (Marti and Lin 1989). OpenKeito replaces the step after a
switching action by two backward-Euler half steps, which removes most of it
but not all; 2S-DIRK is L-stable and damps it within a few steps (Noda,
Takenaka and Inoue 2009).
"""

from __future__ import annotations

import numpy as np

import lib
import style

TITLE = "Inductor voltage after a switch interrupts an inductive current"
E, R, RS, L = 10.0, 1.0, 1e3, 1e-3
H, T_OPEN, END = 1e-4, 1e-3, 3e-3


def circuit(method: str) -> dict:
    return lib.model(
        {"time_step_s": H, "end_time_s": END, "method": method},
        [{"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": E}},
         {"type": "switch", "name": "S1", "nodes": ["n1", "n2"], "close_time_s": 0.0, "open_time_s": T_OPEN,
          "open_at_current_zero": False},
         {"type": "resistor", "name": "Rs", "nodes": ["n1", "n2"], "resistance_ohm": RS},
         {"type": "resistor", "name": "R1", "nodes": ["n2", "n3"], "resistance_ohm": R},
         {"type": "inductor", "name": "L1", "nodes": ["n3", "gnd"], "inductance_h": L,
          "initial_current_a": E / R}],
        [{"type": "node_voltage", "node": "n3"}, {"type": "branch_current", "component": "L1"}])


def exact(t: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """(v_L, i_L). At the grid point of the opening itself the row holds the
    values before the action."""
    i_end = E / (R + RS)
    x = np.maximum(t - T_OPEN, 0.0) * (R + RS) / L
    after = t > T_OPEN + 1e-12
    i = np.where(after, i_end + (E / R - i_end) * np.exp(-x), E / R)
    v = np.where(after, -(R + RS) * (E / R - i_end) * np.exp(-x), 0.0)
    return v, i


def make(ctx: lib.Context) -> dict:
    runs = {method: lib.run(circuit(method)) for method, _ in lib.METHODS}
    t = runs["2s-dirk"].time
    v_exact, i_exact = exact(t)
    lib.write_csv(ctx, "numerical_oscillation.csv",
                  ["time_s", "v_l_exact_v", "v_l_2s_dirk_v", "v_l_trapezoidal_v",
                   "i_l_exact_a", "i_l_2s_dirk_a", "i_l_trapezoidal_a"],
                  zip(t, v_exact, runs["2s-dirk"]["V(n3)"], runs["trapezoidal"]["V(n3)"],
                      i_exact, runs["2s-dirk"]["I(L1)"], runs["trapezoidal"]["I(L1)"]))

    fig, ax = style.figure()
    ms = t * 1e3
    ax.plot(ms, v_exact, label="Exact", **style.reference())
    for k, (method, label) in enumerate(lib.METHODS):
        ax.plot(ms, runs[method]["V(n3)"], label=label, **style.series(k))
    ax.set_xlim(0.8, 2.6)
    # Linear within 1 V, logarithmic beyond: the first value of 2S-DIRK is
    # 440 V, the oscillation of the trapezoidal rule a few volts.
    ax.set_yscale("symlog", linthresh=1.0)
    ax.set_ylim(-1e3, 1e3)
    ax.set_xlabel("Time (ms)")
    ax.set_ylabel("Inductor voltage (V)")
    style.legend_below(ax, ncols=3)
    lib.save_figure(ctx, fig, "numerical_oscillation")

    k0 = int(round(T_OPEN / H))
    out = {}
    for method, _ in lib.METHODS:
        v = runs[method]["V(n3)"]
        out[f"{method}: v_L 1, 2, 3, 5, 10 steps after the opening (V)"] = [
            lib.sig(v[k0 + n], 4) for n in (1, 2, 3, 5, 10)]
    x = H * (R + RS) / L
    out["trapezoidal: factor per step (1 - x/2)/(1 + x/2)"] = round((1 - x / 2) / (1 + x / 2), 4)
    v = runs["trapezoidal"]["V(n3)"]
    out["trapezoidal: measured factor per step"] = round(float(v[k0 + 6] / v[k0 + 5]), 4)
    v = runs["2s-dirk"]["V(n3)"]
    out["2s-dirk: measured factor per step"] = round(float(v[k0 + 3] / v[k0 + 2]), 4)
    out["exact v_L one step after the opening (V)"] = lib.sig(exact(np.array([T_OPEN + H]))[0][0])
    return out
