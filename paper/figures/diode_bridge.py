"""Single-phase diode bridge against its ideal solution.

The bridge of examples/rectifier.json: 100 V rms at 50 Hz, C = 1 mF and
R = 100 ohm on the dc side (w R C = 31.4). With ideal diodes and in the
periodic state, per half period (angle x = w t modulo pi):

- the diodes stop conducting where their current C dv/dt + v/R is zero:
  x_off = pi - atan(w R C);
- the capacitor then discharges into R, v = Vp sin(x_off) e^{-(x - x_off)/(w R C)},
  until the source reaches it again at x_on + pi:
  sin(x_off) e^{-(x_on + pi - x_off)/(w R C)} = sin(x_on);
- while the diodes conduct, v = Vp |sin x|.

The instants of the diodes are read from the switching events of the last
period and compared with these angles, for switching at grid points and
within the step. A second circuit, the bridge on a resistor alone, is
compared with the solution of its resistive circuit.
"""

from __future__ import annotations

import math

import numpy as np

import lib
import style

TITLE = "Diode bridge with capacitor input: waveform and conduction angles against the ideal solution"
V_RMS, FREQ, R, C = 100.0, 50.0, 100.0, 1e-3
RON, ROFF = 1e-3, 1e6
W = 2 * math.pi * FREQ
VP = V_RMS * math.sqrt(2)
END = 0.1


def forced() -> tuple[float, float]:
    """(gain, phase) of the dc voltage while two diodes conduct: the source
    feeds R and C through the two on-resistances, v = Vp gain sin(x + phase)."""
    y = 1 / (2 * RON)
    h = y / (y + 1 / R + 1j * W * C)
    return abs(h), math.atan2(h.imag, h.real)


def angles(ideal_diodes: bool = False) -> tuple[float, float]:
    """(x_off, x_on) in radians, for ideal diodes or with the
    on-resistance of the two conducting diodes."""
    k = W * R * C
    if ideal_diodes:
        off = math.pi - math.atan(k)
        v_off = math.sin(off)
    else:
        # The diode current (v_s - v) / (2 R_on) is zero where the source
        # meets the forced response.
        gain, phase = forced()
        lo, hi = math.pi / 2, math.pi
        for _ in range(200):
            mid = 0.5 * (lo + hi)
            if math.sin(mid) > gain * math.sin(mid + phase):
                lo = mid
            else:
                hi = mid
        off = 0.5 * (lo + hi)
        v_off = math.sin(off)
    lo, hi = 0.0, math.pi / 2
    for _ in range(200):
        mid = 0.5 * (lo + hi)
        if v_off * math.exp(-(mid + math.pi - off) / k) > math.sin(mid):
            lo = mid
        else:
            hi = mid
    return off, 0.5 * (lo + hi)


def reference(t: np.ndarray) -> np.ndarray:
    """The dc voltage in the periodic state with the on-resistances: the
    forced response (and what is left of the start of the conduction,
    which decays with 2 R_on C = 2 us) while the diodes conduct, the
    discharge into R otherwise. The leakage of the blocking diodes is left
    out (R / R_off = 1e-4 of the droop)."""
    off, on = angles()
    gain, phase = forced()
    k = W * R * C
    x = np.mod(W * t, math.pi)
    since = np.where(x >= off, x - off, x + math.pi - off)
    decay = VP * math.sin(off) * np.exp(-since / k)
    tau = C / (1 / (2 * RON) + 1 / R)
    start = VP * (math.sin(on) - gain * math.sin(on + phase))
    conducting = VP * gain * np.sin(x + phase) + start * np.exp(-np.maximum(x - on, 0.0) / (W * tau))
    return np.where((x >= on) & (x <= off), conducting, decay)


def circuit(method: str, switching: str, h: float, capacitor: bool = True, end: float = END) -> dict:
    def diode(name, a, k):
        return {"type": "diode", "name": name, "nodes": [a, k], "on_resistance_ohm": RON,
                "off_resistance_ohm": ROFF}

    components = [
        {"type": "voltage_source", "name": "E1", "nodes": ["a", "gnd"],
         "waveform": {"type": "sine", "amplitude": VP, "frequency_hz": FREQ}},
        diode("D1", "a", "p"), diode("D2", "n", "a"), diode("D3", "gnd", "p"), diode("D4", "n", "gnd"),
        {"type": "resistor", "name": "R1", "nodes": ["p", "n"], "resistance_ohm": R}]
    if capacitor:
        components.append({"type": "capacitor", "name": "C1", "nodes": ["p", "n"], "capacitance_f": C})
    return lib.model({"time_step_s": h, "end_time_s": end, "method": method, "switching": switching}, components,
                     [{"type": "branch_voltage", "component": "R1"}, {"type": "node_voltage", "node": "a"}])


def event_angles(result) -> tuple[list[float], list[float], int]:
    """(turn-off angles, turn-on angles, events) of D1 and D2 in the last
    period, modulo pi: a diode turns on at its first closing and off at its
    last opening of the half period. More than two events per diode mean
    that it opened and closed again on the way."""
    off, on, count = [], [], 0
    for name in ("D1", "D2"):
        mine = [e for e in result.summary["all_events"]
                if e["time_s"] >= END - 1 / FREQ - 1e-9 and e["component"] == name]
        count += len(mine)
        on.append(math.fmod(W * [e for e in mine if e["action"] == "close"][0]["time_s"], math.pi))
        off.append(math.fmod(W * [e for e in mine if e["action"] == "open"][-1]["time_s"], math.pi))
    return off, on, count


def make(ctx: lib.Context) -> dict:
    off, on = angles()
    steps = [50e-6, 20e-6] if ctx.quick else [50e-6, 20e-6, 10e-6, 5e-6, 2e-6, 1e-6]
    rows, worst, events = [], {}, {}
    for switching in ("grid", "interpolated"):
        for h in steps:
            r = lib.run(circuit("2s-dirk", switching, h))
            offs, ons, count = event_angles(r)
            d_off = max(abs(a - off) for a in offs)
            d_on = max(abs(a - on) for a in ons)
            t, v = r.time, r["U(R1)"]
            last = t >= END - 1 / FREQ - 1e-12
            dv = float(np.max(np.abs(v[last] - reference(t[last]))))
            rows.append((switching, h, W * h, d_off, d_on, dv, count, r.summary["solver"]["max_resolves_per_step"]))
            events[(switching, h)] = count
            worst[(switching, h)] = (d_off, d_on, dv)
    lib.write_csv(ctx, "diode_bridge_angles.csv",
                  ["switching", "time_step_s", "omega_h_rad", "turn_off_angle_error_rad", "turn_on_angle_error_rad",
                   "max_error_v_dc_last_period_v", "diode_events_last_period", "max_resolves_per_step"], rows)

    h_wave = 20e-6
    r = lib.run(circuit("2s-dirk", "interpolated", h_wave))
    keep = r.time >= END - 1 / FREQ - 1e-12
    t, v, vs = r.time[keep], r["U(R1)"][keep], r["V(a)"][keep]
    lib.write_csv(ctx, "diode_bridge_waveform.csv", ["time_s", "v_source_v", "v_dc_v", "v_dc_reference_v"],
                  zip(t, vs, v, reference(t)))

    fig, (ax1, ax2) = style.figure(2, 1, width="double", height_in=3.1)
    ms = (t - t[0]) * 1e3
    ax1.plot(ms, np.abs(vs), label="|Source voltage|", color=style.GRID, linewidth=1.0)
    ax1.plot(ms, reference(t), label="Closed form", **style.reference())
    every = max(1, len(t) // 40)
    ax1.plot(ms[::every], v[::every], label="OpenKeito, 20 µs", **style.series(0, line=False))
    ax1.set_ylim(100, 150)
    ax1.set_xlabel("Time within the last period (ms)")
    ax1.set_ylabel("DC voltage (V)")
    style.legend_below(ax1)
    style.panel(ax1, "a")
    hs = np.array(steps) * 1e6
    series = [("grid", 0, "turn-off", "Turn-off, grid points"), ("grid", 1, "turn-on", "Turn-on, grid points"),
              ("interpolated", 0, "turn-off", "Turn-off, within the step"),
              ("interpolated", 1, "turn-on", "Turn-on, within the step")]
    for k, (switching, which, _, label) in enumerate(series):
        ax2.loglog(hs, [worst[(switching, h)][which] for h in steps], label=label, **style.series(k))
    ax2.loglog(hs, W * np.array(steps), label=r"$\omega h$", **style.reference(dotted=True))
    style.step_axis(ax2, hs, ylabel="Error of the angle (rad)")
    style.legend_below(ax2)
    style.panel(ax2, "b")
    lib.save_figure(ctx, fig, "diode_bridge")

    # Resistive load: v_R = |v_s| R / (R + 2 R_on) while two diodes conduct.
    h = 1e-5
    r = lib.run(circuit("2s-dirk", "interpolated", h, capacitor=False, end=0.04))
    expected = np.abs(r["V(a)"]) * R / (R + 2 * RON)
    # The rows at the zero crossings hold the state before the commutation.
    diff = np.abs(r["U(R1)"] - expected)
    resistive = float(np.max(diff))
    away = np.abs(r["V(a)"]) > 1e-6
    resistive_away = float(np.max(diff[away]))
    n = int(round(1 / (FREQ * h)))
    mean = float(np.mean(r["U(R1)"][-n:]))
    formula = 2 * math.sqrt(2) * V_RMS / math.pi * R / (R + 2 * RON)

    h0 = steps[-1]
    return {
        "turn-off and turn-on angles with ideal diodes (deg)": [
            round(math.degrees(a), 3) for a in angles(ideal_diodes=True)],
        "turn-off and turn-on angles with the on-resistance of 1 mohm (deg)": [
            round(math.degrees(off), 3), round(math.degrees(on), 3)],
        "dc voltage with ideal diodes, minimum and maximum (V)": [
            round(VP * math.sin(angles(ideal_diodes=True)[1]), 3), round(VP, 3)],
        "steps at which a diode opens and closes again at the turn-off (us)": [
            f"{sw} {h * 1e6:g}" for (sw, h), n in events.items() if n > 4],
        f"grid, {h0 * 1e6:g} us: turn-off, turn-on angle error (rad), w h": [
            lib.sig(worst[("grid", h0)][0]), lib.sig(worst[("grid", h0)][1]), lib.sig(W * h0)],
        f"interpolated, {h0 * 1e6:g} us: turn-off, turn-on angle error (rad)": [
            lib.sig(worst[("interpolated", h0)][0]), lib.sig(worst[("interpolated", h0)][1])],
        f"interpolated: max error of v_dc in the last period (V) at {[f'{h * 1e6:g}' for h in steps]} us": [
            lib.sig(worst[("interpolated", h)][2]) for h in steps],
        "resistive load: max difference from the resistive circuit (V)": lib.sig(resistive),
        "resistive load: max difference away from zero crossings (V)": lib.sig(resistive_away),
        "resistive load: mean of the last period, formula 2 sqrt(2) V / pi (V)": [round(mean, 5), round(formula, 5)],
    }
