"""Start from the steady state: what the phasor initialization saves and
how far the waveforms depart from it.

The network of examples/three_phase_fault.json without its fault: a 66 kV
source, a 50 km line as two pi sections, a 30 MVA YNd11 transformer with a
magnetizing branch and a 5 MW load. With `initialization: steady_state` the
solver solves the complex nodal equations once at 50 Hz and starts every
state at sqrt(2) Re(X); the summary of the run then gives the largest
departure of the waveforms from sqrt(2) Re(X e^{jwt}), relative to the
largest amplitude. That departure is the discretization error of the
method, of the order (w h)^2: it falls by 4 when the step is halved.

Without the initialization the run starts from rest and the transformer
flux and the line carry a transient for many periods.
"""

from __future__ import annotations

import math

import numpy as np

import lib
import style

TITLE = "Departure from the phasor steady state against the time step, and the start from rest"
FREQ = 50.0


def circuit(method: str, h: float, initialization: str, end: float) -> dict:
    m = lib.example("three_phase_fault.json")
    components = [c for c in m["components"] if c["type"] != "fault"]
    return lib.model({"time_step_s": h, "end_time_s": end, "method": method, "initialization": initialization},
                     components, [{"type": "bus_voltage", "bus": "b3"}])


def make(ctx: lib.Context) -> dict:
    steps = [100e-6, 50e-6] if ctx.quick else [100e-6, 50e-6, 20e-6, 10e-6, 5e-6]
    end = 0.04 if ctx.quick else 0.1
    rows, deviation = [], {}
    for method, _ in lib.METHODS:
        for h in steps:
            init = lib.run(circuit(method, h, "steady_state", end)).summary["initialization"]
            deviation[(method, h)] = (init["relative_voltage_deviation"], init["relative_current_deviation"])
            rows.append((method, h, (2 * math.pi * FREQ * h) ** 2, *deviation[(method, h)]))
    lib.write_csv(ctx, "steady_state.csv",
                  ["method", "time_step_s", "omega_h_squared", "relative_voltage_deviation",
                   "relative_current_deviation"], rows)

    h = 20e-6
    started = lib.run(circuit("2s-dirk", h, "steady_state", end))
    rest = lib.run(circuit("2s-dirk", h, "none", end))
    phasors = {p["name"]: p for p in started.summary["initialization"]["phasors"]}
    p = phasors["V(b3.a)"]
    amplitude = p["rms"] * math.sqrt(2)
    t = started.time
    ideal = amplitude * np.cos(2 * math.pi * FREQ * t + math.radians(p["angle_deg"]))
    every = 5
    lib.write_csv(ctx, "steady_state_waveform.csv",
                  ["time_s", "v_b3_a_phasor_v", "v_b3_a_initialized_v", "v_b3_a_from_rest_v"],
                  zip(t[::every], ideal[::every], started["V(b3.a)"][::every], rest["V(b3.a)"][::every]))
    from_rest = float(np.max(np.abs(rest["V(b3.a)"] - ideal)) / amplitude)

    fig, (ax1, ax2) = style.figure(2, 1, width="double", height_in=2.5)
    hs = np.array(steps) * 1e6
    for k, (method, label) in enumerate(lib.METHODS):
        ax1.loglog(hs, [deviation[(method, x)][0] for x in steps], label=label, **style.series(k))
    ax1.loglog(hs, (2 * math.pi * FREQ * np.array(steps)) ** 2 / 12, label=r"$(\omega h)^2/12$",
               **style.reference(dotted=True))
    style.step_axis(ax1, hs, ylabel="Departure from the steady state\n(relative to the amplitude)")
    ax1.legend()
    style.panel(ax1, "a")
    ms = t * 1e3
    ax2.plot(ms, (rest["V(b3.a)"] - ideal) / amplitude, label="Start from rest", **{**style.series(1, marker=False), "linestyle": "-", "linewidth": 0.6})
    ax2.plot(ms, (started["V(b3.a)"] - ideal) / amplitude, label="Start from the phasor solution",
             **style.series(0, marker=False))
    ax2.set_xlabel("Time (ms)")
    ax2.set_ylabel("Difference from the steady state\n(relative to the amplitude)")
    ax2.legend(loc="lower right")
    style.panel(ax2, "b")
    lib.save_figure(ctx, fig, "steady_state")

    out = {}
    for method, _ in lib.METHODS:
        v = [deviation[(method, x)][0] for x in steps]
        out[f"{method}: relative voltage departure at {[f'{x * 1e6:g}' for x in steps]} us"] = [
            lib.sig(x) for x in v]
        out[f"{method}: ratio when the step is halved (100 -> 50 us)"] = round(v[0] / v[1], 3)
    out["from rest: largest difference from the steady state in the run, relative to the amplitude"] = round(
        from_rest, 3)
    return out
