"""Lines: the travelling-wave (Bergeron) line against the lattice diagram,
and pi sections converging to it.

(a) A lossless line with Zc = 400 ohm and a travel time tau = 100 us (25 km
at 4 us/km, the same in every mode, so that one phase is a line of its own)
between a dc source of 100 V behind Rs = 100 ohm and a load Rl = 1200 ohm.
With the reflection coefficients g_s = (Rs - Zc)/(Rs + Zc) = -0.6 and
g_l = (Rl - Zc)/(Rl + Zc) = +0.5 and the first wave v0 = E Zc/(Rs + Zc),
the lattice diagram gives (Dommel 1969; EMTP Theory Book, ch. 4)

    v_load(t)   = sum over arrivals k at tau, 3 tau, ...:  v0 (g_s g_l)^k (1 + g_l),
    v_source(t) = v0 + sum over arrivals k at 2 tau, 4 tau, ...:
                  v0 g_l (g_s g_l)^(k-1) (1 + g_s),

which tends to E Rl/(Rs + Rl). The travel time is a whole number of steps,
so the line model has no interpolation error.

(b) The open-end voltage of a lossless 300 km overhead line at 50 Hz, in the
phasor solution: N pi sections against the travelling-wave line, which for
a lossless line is the distributed line, V_r/V_s = 1/cos(w l sqrt(L C)). The
difference falls with 1/N^2.
"""

from __future__ import annotations

import math

import numpy as np

import lib
import style

TITLE = "Travelling-wave line against the lattice diagram; pi sections converging to the distributed line"
E, ZC, S_PER_KM, RS, RL = 100.0, 400.0, 4e-6, 100.0, 1200.0
LENGTH, H = 25.0, 10e-6
TAU = LENGTH * S_PER_KM
FREQ = 50.0
W = 2 * math.pi * FREQ
# Overhead line by its sequence values (those of examples/line_energization.json).
L1, C1, L0, C0 = 0.9e-3, 12.7e-9, 3e-3, 7e-9
LONG = 300.0


def lattice(t: float) -> tuple[float, float]:
    """(sending-end voltage, receiving-end voltage). A front that arrives
    at a grid point is in its row."""
    v0 = E * ZC / (RS + ZC)
    gs, gl = (RS - ZC) / (RS + ZC), (RL - ZC) / (RL + ZC)
    n = math.floor(t / TAU + 1e-9)
    at_load = sum(v0 * (gs * gl) ** k * (1 + gl) for k in range((n + 1) // 2))
    at_source = sum(v0 * gl * (gs * gl) ** (k - 1) * (1 + gs) for k in range(1, n // 2 + 1))
    return v0 + at_source, at_load


def lattice_circuit(method: str, travel_times: float) -> dict:
    per_km = {"type": "sequence", "r1_ohm_per_km": 0.0, "l1_h_per_km": 1.6e-3, "c1_f_per_km": 1e-8,
              "r0_ohm_per_km": 0.0, "l0_h_per_km": 1.6e-3, "c0_f_per_km": 1e-8}
    return lib.model(
        {"time_step_s": H, "end_time_s": travel_times * TAU, "method": method, "initialization": "none"},
        [{"type": "voltage_source", "name": "E1", "nodes": ["e", "gnd"], "waveform": {"type": "dc", "value": E}},
         {"type": "resistor", "name": "Rs", "nodes": ["e", "s.a"], "resistance_ohm": RS},
         {"type": "bergeron_line", "name": "L1", "buses": ["s", "r"], "length_km": LENGTH, "per_km": per_km},
         {"type": "resistor", "name": "Rl", "nodes": ["r.a", "gnd"], "resistance_ohm": RL}],
        [{"type": "node_voltage", "node": "s.a"}, {"type": "node_voltage", "node": "r.a"}])


def open_end(line: dict) -> complex:
    """Phasor of the open-end voltage of phase a."""
    r = lib.run(lib.model(
        {"time_step_s": 50e-6, "end_time_s": 1e-3, "method": "2s-dirk", "initialization": "steady_state"},
        [{"type": "three_phase_source", "name": "G", "bus": "s", "line_voltage_rms_v": 66e3, "frequency_hz": FREQ},
         line],
        [{"type": "bus_voltage", "bus": "r"}]))
    p = next(p for p in r.summary["initialization"]["phasors"] if p["name"] == "V(r.a)")
    return lib.polar(p["rms"], p["angle_deg"])


def make(ctx: lib.Context) -> dict:
    travel_times = 14.0
    runs = {m: lib.run(lattice_circuit(m, travel_times)) for m in ("2s-dirk", "trapezoidal")}
    t = runs["2s-dirk"].time
    expected = np.array([lattice(x) for x in t])
    lib.write_csv(ctx, "line_lattice.csv",
                  ["time_s", "v_source_end_lattice_v", "v_load_end_lattice_v", "v_source_end_2s_dirk_v",
                   "v_load_end_2s_dirk_v", "v_source_end_trapezoidal_v", "v_load_end_trapezoidal_v"],
                  zip(t, expected[:, 0], expected[:, 1], runs["2s-dirk"]["V(s.a)"], runs["2s-dirk"]["V(r.a)"],
                      runs["trapezoidal"]["V(s.a)"], runs["trapezoidal"]["V(r.a)"]))
    lattice_error = {m: float(max(np.max(np.abs(r["V(s.a)"] - expected[:, 0])),
                                  np.max(np.abs(r["V(r.a)"] - expected[:, 1]))) / E) for m, r in runs.items()}

    per_km = {"type": "sequence", "r1_ohm_per_km": 0.0, "l1_h_per_km": L1, "c1_f_per_km": C1,
              "r0_ohm_per_km": 0.0, "l0_h_per_km": L0, "c0_f_per_km": C0}
    wave = open_end({"type": "bergeron_line", "name": "L1", "buses": ["s", "r"], "length_km": LONG,
                     "per_km": per_km})
    theta = W * LONG * math.sqrt(L1 * C1)
    closed = 66e3 / math.sqrt(3) / math.cos(theta)
    sections = [1, 2, 4, 8] if ctx.quick else [1, 2, 4, 8, 16, 32, 64]
    difference = []
    for n in sections:
        v = open_end({"type": "pi_line", "name": "L1", "buses": ["s", "r"], "length_km": LONG, "sections": n,
                      "per_km": per_km})
        difference.append(abs(v - wave) / abs(wave))
    ratios = [float("nan")] + [a / b for a, b in zip(difference, difference[1:])]
    lib.write_csv(ctx, "line_pi_sections.csv", ["sections", "relative_difference_open_end_voltage", "ratio"],
                  zip(sections, difference, ratios))

    fig, (ax1, ax2) = style.figure(2, 1, width="double", height_in=2.5)
    us = t / TAU
    r = runs["2s-dirk"]
    ax1.step(us, expected[:, 1], where="post", label="Lattice diagram, load end", **style.reference())
    ax1.step(us, expected[:, 0], where="post", label="Lattice diagram, source end", **style.reference(dotted=True))
    every = 5
    ax1.plot(us[::every], r["V(r.a)"][::every], label="OpenKeito, load end", **style.series(0, line=False))
    ax1.plot(us[::every], r["V(s.a)"][::every], label="OpenKeito, source end", **style.series(1, line=False))
    ax1.set_xlabel(r"Time (travel times $\tau$ = 100 µs)")
    ax1.set_ylabel("Voltage (V)")
    ax1.set_ylim(0, 135)
    ax1.legend(loc="lower right", ncols=1)
    style.panel(ax1, "a")
    ax2.loglog(sections, difference, label="Pi sections against the travelling-wave line", **style.series(0))
    ax2.loglog(sections, difference[0] * 2.5 / np.array(sections, dtype=float) ** 2, label="Slope $-2$",
               **style.reference(dotted=True))
    ax2.set_xlabel("Number of pi sections")
    ax2.set_ylabel("Relative difference of the\nopen-end voltage")
    style.plain_log_ticks(ax2, sections)
    ax2.legend(loc="lower left")
    style.panel(ax2, "b")
    lib.save_figure(ctx, fig, "line_models")

    return {
        "lattice: largest error of both ends over 14 travel times, relative to E (2S-DIRK, trapezoidal)": [
            lib.sig(lattice_error["2s-dirk"], 2), lib.sig(lattice_error["trapezoidal"], 2)],
        "lattice: dc value E Rl/(Rs + Rl) (V)": round(E * RL / (RS + RL), 4),
        "open end, travelling-wave line against 1/cos(theta), relative": lib.sig(abs(abs(wave) - closed) / closed, 2),
        "open end: rise of the voltage 1/cos(theta) - 1": round(1 / math.cos(theta) - 1, 5),
        f"pi sections {sections}: relative difference": [lib.sig(d) for d in difference],
        "pi sections: ratio per doubling": [round(x, 3) for x in ratios[1:]],
    }
