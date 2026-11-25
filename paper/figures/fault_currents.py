"""Fault currents against symmetrical components.

A 66 kV, 50 Hz source with Z1 = 0.5 + j5 ohm and Z0 = 1 + j15 ohm feeds a
coupled line with Z1 = 1 + j4 ohm and Z0 = 3 + j12 ohm; a fault at its end
starts at t = 0. Up to the fault the sequence impedances are
Z1 = Z2 = 1.5 + j9 ohm and Z0 = 4 + j27 ohm. With the pre-fault voltage E of
phase a, a resistance Zf in each faulted phase and Zg from the fault point to
ground, the sequence networks give (Fortescue 1918; Anderson 1973, ch. 3;
Grainger and Stevenson 1994, ch. 12):

    three-phase:          I1 = E / (Z1 + Zf)
    single line-ground:   I0 = I1 = I2 = E / (Z1 + Z2 + Z0 + 3 (Zf + Zg))
    line-line (b, c):     I1 = -I2 = E / (Z1 + Z2 + 2 Zf)
    double line-ground:   I1 = E / (Z1 + Zf + (Z2 + Zf) || (Z0 + Zf + 3 Zg)),
                          I2, I0 by current division

and I_a = I0 + I1 + I2, I_b = I0 + a^2 I1 + a I2, I_c = I0 + a I1 + a^2 I2.

The phasors of the run are taken by a discrete Fourier transform over its
last period (0.4 s: the dc offsets, which decay with L/R = 21.5 ms at the
slowest, are down to 8e-9). The largest relative error over the faulted
phases is compared with (w h)^2 / 12, the error that the frequency warping
of the trapezoidal rule predicts.
"""

from __future__ import annotations

import cmath
import math

import numpy as np

import lib
import style

TITLE = "Fault currents of the four fault types against symmetrical components"
V_LL, ANGLE, FREQ = 66e3, 20.0, 50.0
W = 2 * math.pi * FREQ
Z1, Z0 = complex(1.5, 9.0), complex(4.0, 27.0)
END = 0.4
KINDS = [("three_phase", "Three-phase", ["a", "b", "c"], True),
         ("single_line_ground", "Single line-to-ground", ["a"], True),
         ("line_line", "Line-to-line", ["b", "c"], False),
         ("double_line_ground", "Double line-to-ground", ["b", "c"], True)]
#: (fault resistance, ground resistance) in ohm.
RESISTANCES = [(0.0, 0.0), (2.0, 5.0)]


def sequence_currents(kind: str, zf: float, zg: float) -> list[complex]:
    """[I_a, I_b, I_c] of the fault."""
    e = lib.polar(V_LL / math.sqrt(3), ANGLE)
    z1, z2, z0 = Z1, Z1, Z0
    if kind == "three_phase":
        i0, i1, i2 = 0j, e / (z1 + zf), 0j
    elif kind == "single_line_ground":
        i0 = i1 = i2 = e / (z1 + z2 + z0 + 3 * (zf + zg))
    elif kind == "line_line":
        i1 = e / (z1 + z2 + 2 * zf)
        i0, i2 = 0j, -i1
    else:
        zz0, zz2 = z0 + zf + 3 * zg, z2 + zf
        i1 = e / (z1 + zf + zz2 * zz0 / (zz2 + zz0))
        i0, i2 = -i1 * zz2 / (zz2 + zz0), -i1 * zz0 / (zz2 + zz0)
    a = cmath.rect(1.0, 2 * math.pi / 3)
    return [i0 + i1 + i2, i0 + a * a * i1 + a * i2, i0 + a * i1 + a * a * i2]


def circuit(method: str, h: float, phases: list[str], ground: bool, zf: float, zg: float) -> dict:
    return lib.model(
        {"time_step_s": h, "end_time_s": END, "method": method},
        [{"type": "three_phase_source", "name": "G1", "bus": "s", "line_voltage_rms_v": V_LL,
          "frequency_hz": FREQ, "angle_deg": ANGLE,
          "impedance": {"type": "sequence", "r1_ohm": 0.5, "l1_h": 5.0 / W, "r0_ohm": 1.0, "l0_h": 15.0 / W}},
         {"type": "coupled_rl", "name": "LINE", "buses": ["s", "f"],
          "impedance": {"type": "sequence", "r1_ohm": 1.0, "l1_h": 4.0 / W, "r0_ohm": 3.0, "l0_h": 12.0 / W}},
         {"type": "fault", "name": "F1", "bus": "f", "phases": phases, "ground": ground,
          "fault_resistance_ohm": zf, "ground_resistance_ohm": zg, "start_time_s": 0.0}],
        [{"type": "branch_current", "component": "F1"}])


def make(ctx: lib.Context) -> dict:
    steps = [100e-6, 50e-6] if ctx.quick else [100e-6, 50e-6, 20e-6, 10e-6]
    h0 = 20e-6 if 20e-6 in steps else steps[-1]
    kinds = KINDS[:2] if ctx.quick else KINDS
    rows, worst = [], {}
    table = []
    for kind, label, phases, ground in kinds:
        for zf, zg in RESISTANCES:
            expected = sequence_currents(kind, zf, zg if ground else 0.0)
            for method, _ in lib.METHODS:
                for h in steps:
                    r = lib.run(circuit(method, h, phases, ground, zf, zg))
                    errors = []
                    for p in phases:
                        ref = expected["abc".index(p)]
                        got = lib.phasor(r.time, r[f"I(F1.{p})"], FREQ)
                        errors.append(abs(got - ref) / abs(ref))
                        rows.append((kind, zf, zg, method, h, p, abs(ref), math.degrees(cmath.phase(ref)),
                                     abs(got), math.degrees(cmath.phase(got)), errors[-1]))
                    key = (method, h)
                    worst[key] = max(worst.get(key, 0.0), max(errors))
                    worst[(kind, zf, method, h)] = max(errors)
            table.append([label, f"{zf:g}, {zg:g}",
                          ", ".join(f"{abs(expected['abc'.index(p)]) / 1e3:.4f}" for p in phases),
                          lib.sci(worst[(kind, zf, "2s-dirk", h0)]), lib.sci(worst[(kind, zf, "trapezoidal", h0)])])
    lib.write_csv(ctx, "fault_currents.csv",
                  ["fault", "fault_resistance_ohm", "ground_resistance_ohm", "method", "time_step_s", "phase",
                   "hand_rms_a", "hand_angle_deg", "emt_rms_a", "emt_angle_deg", "relative_error"], rows)
    lib.write_table(ctx, "fault_currents_table",
                    ["Fault", "Zf, Zg (ohm)", "Hand calculation, faulted phases (kA rms)",
                     f"Rel. error, 2S-DIRK, {h0 * 1e6:g} us", f"Rel. error, trapezoidal, {h0 * 1e6:g} us"],
                    table, "Fault currents: symmetrical components against the EMT run (phasor of the last period).")

    fig, ax = style.figure()
    hs = np.array(steps) * 1e6
    for k, (method, label) in enumerate(lib.METHODS):
        ax.loglog(hs, [worst[(method, h)] for h in steps], label=f"{label}, largest of all faults",
                  **style.series(k))
    ax.loglog(hs, (W * np.array(steps)) ** 2 / 12, label=r"$(\omega h)^2/12$", **style.reference(dotted=True))
    style.step_axis(ax, hs, ylabel="Relative error of the fault current")
    ax.legend()
    lib.save_figure(ctx, fig, "fault_currents")

    out = {f"(w h)^2 / 12 at {h0 * 1e6:g} us": lib.sig((W * h0) ** 2 / 12)}
    for method, _ in lib.METHODS:
        out[f"{method}: largest relative error over all faults at {[f'{h * 1e6:g}' for h in steps]} us"] = [
            lib.sig(worst[(method, h)]) for h in steps]
        per_kind = [worst[(kind, zf, method, h0)] for kind, *_ in kinds for zf, _ in RESISTANCES]
        out[f"{method}: range of the relative error at {h0 * 1e6:g} us"] = [
            lib.sig(min(per_kind)), lib.sig(max(per_kind))]
    return out
