"""Three-phase two-level PWM inverter at large time steps against the same
model solved within the step at 0.25 us (issue #48).

The circuit of crates/openkeito-core/tests/pwm_inverter.rs: per phase a
controlled switch to each rail with an antiparallel diode, between DC
sources of +-375 V with the midpoint at gnd; an L filter of 0.2 ohm and
2 mH; a 400 V, 50 Hz grid behind 0.02 ohm and 0.2 mH. Sine-triangle PWM at
2450 Hz (a carrier period of 408.16 us, a whole number of none of the time
steps), modulation 0.89 fixed, with and without a dead time of 2 us (shorter
than the steps). About 100 A peak; the rating, 50 kVA, is 102 A peak.

Compared on the phase currents over one period of the fundamental (40 to
60 ms), with the reference sampled at the grid points of the coarser run:
the largest difference of the averages over carrier periods (both curves
integrated the same way between their samples), and the amplitude and
phase of the fundamental (a DFT over the period).

- "grid": a switching instant moves to a grid point: first order.
- "interpolated": the step is split at the instant: second order, also at
  25 us, where the narrowest pulse ((1 - M) Tc/2 = 22.4 us here) begins and
  ends within a step and is found at the vertex of the carrier (to 0.14.0
  only the end of the part was previewed and such pulses were lost).
"""

from __future__ import annotations

import math

import numpy as np

import lib
import style

TITLE = "Three-phase PWM inverter: switching at grid points and within the step against 0.25 us"
F, FC, V_LL, VDC = 50.0, 2450.0, 400.0, 750.0
RF, LF, RG, LG = 0.2, 2e-3, 0.02, 0.2e-3
M, DELTA_DEG = 0.89, 11.9
T0, T1 = 0.04, 0.06
H_ROW = 1e-6
RATED_PEAK = 50e3 / (math.sqrt(3) * V_LL) * math.sqrt(2)


def circuit(h: float, switching: str, dead_s: float, end: float = T1) -> dict:
    seq = {"type": "sequence", "r1_ohm": RF, "l1_h": LF, "r0_ohm": RF, "l0_h": LF}
    grid = {"type": "sequence", "r1_ohm": RG, "l1_h": LG, "r0_ohm": RG, "l0_h": LG}
    components = [
        {"type": "voltage_source", "name": "EP", "nodes": ["dcp", "gnd"], "waveform": {"type": "dc", "value": VDC / 2}},
        {"type": "voltage_source", "name": "EN", "nodes": ["dcn", "gnd"], "waveform": {"type": "dc", "value": -VDC / 2}},
        {"type": "coupled_rl", "name": "LF", "buses": ["sw", "pcc"], "impedance": seq},
        {"type": "coupled_rl", "name": "ZG", "buses": ["pcc", "g"], "impedance": grid},
        {"type": "three_phase_source", "name": "G1", "bus": "g", "line_voltage_rms_v": V_LL, "frequency_hz": F},
    ]
    # The carrier shifted by +-2 fc Td for the two comparisons: both switches
    # are off for Td at every edge.
    controls = [
        {"type": "triangle_wave", "name": "carrier", "frequency_hz": FC, "minimum": -1.0, "maximum": 1.0},
        {"type": "constant", "name": "shift", "value": 2 * FC * dead_s},
        {"type": "sum", "name": "cp", "inputs": ["carrier", "shift"], "signs": [1.0, 1.0]},
        {"type": "sum", "name": "cn", "inputs": ["carrier", "shift"], "signs": [1.0, -1.0]},
    ]
    for ph, angle in (("a", 0.0), ("b", -120.0), ("c", 120.0)):
        sw = f"sw.{ph}"
        controls += [
            {"type": "sine_wave", "name": f"m_{ph}", "amplitude": M, "frequency_hz": F, "phase_deg": 90 + DELTA_DEG + angle},
            {"type": "comparator", "name": f"gp_{ph}", "inputs": [f"m_{ph}", "cp"]},
            {"type": "comparator", "name": f"gn_{ph}", "inputs": ["cn", f"m_{ph}"]},
        ]
        components += [
            {"type": "controlled_switch", "name": f"SP{ph}", "nodes": ["dcp", sw], "gate": f"gp_{ph}"},
            {"type": "controlled_switch", "name": f"SN{ph}", "nodes": [sw, "dcn"], "gate": f"gn_{ph}"},
            {"type": "diode", "name": f"DP{ph}", "nodes": [sw, "dcp"]},
            {"type": "diode", "name": f"DN{ph}", "nodes": ["dcn", sw]},
        ]
    interval = max(1, round(H_ROW / h))
    return lib.model({"time_step_s": h, "end_time_s": end, "switching": switching, "output_interval_steps": interval},
                     components, [{"type": "branch_current", "component": "LF"}], controls=controls)


def sampled(r, h: float) -> tuple[np.ndarray, np.ndarray]:
    """The times k h from T0 to T1 and the phase currents there (3 columns)."""
    t = r.time
    step = round(h / (t[1] - t[0]))
    k0, k1 = round(T0 / (t[1] - t[0])), round(T1 / (t[1] - t[0]))
    keep = np.arange(k0, k1 + 1, step)
    return t[keep], np.column_stack([r[f"I(LF.{ph})"][keep] for ph in "abc"])


def carrier_averages(t: np.ndarray, x: np.ndarray) -> np.ndarray:
    """Averages of the line through the samples over the carrier periods
    from t[0]."""
    cumulative = np.concatenate([[0.0], np.cumsum(0.5 * (x[1:] + x[:-1]) * np.diff(t))])

    def integral_to(a: float) -> float:
        k = min(max(int(np.searchsorted(t, a, side="right")) - 1, 0), len(t) - 2)
        xa = x[k] + (x[k + 1] - x[k]) * (a - t[k]) / (t[k + 1] - t[k])
        return cumulative[k] + 0.5 * (x[k] + xa) * (a - t[k])

    tc = 1.0 / FC
    count = int(math.floor((t[-1] - t[0]) / tc))
    edges = [integral_to(t[0] + j * tc) for j in range(count + 1)]
    return np.diff(edges) / tc


def fundamental(t: np.ndarray, x: np.ndarray) -> complex:
    """Phasor (peak) of the fundamental over the period from t[0] (the last
    sample is the first one again)."""
    w = 2 * math.pi * F
    samples, times = x[:-1], t[:-1] - t[0]
    return complex(2.0 / len(samples) * np.sum(samples * np.exp(-1j * w * times)))


def errors(coarse, h: float, reference) -> dict[str, float]:
    tc, xc = sampled(coarse, h)
    tr, xr = sampled(reference, h)
    assert np.allclose(tc, tr)
    out = {"row": 0.0, "average": 0.0, "amplitude": 0.0, "phase_deg": 0.0}
    for k in range(3):
        a, b = xc[:, k], xr[:, k]
        out["row"] = max(out["row"], float(np.max(np.abs(a - b))))
        out["average"] = max(out["average"], float(np.max(np.abs(carrier_averages(tc, a) - carrier_averages(tc, b)))))
        pa, pb = fundamental(tc, a), fundamental(tc, b)
        out["amplitude"] = max(out["amplitude"], abs(abs(pa) - abs(pb)) / abs(pb))
        d = math.degrees(np.angle(pa / pb))
        out["phase_deg"] = max(out["phase_deg"], abs(d))
    return out


def make(ctx: lib.Context) -> dict:
    if ctx.quick:
        h_ref, dead_times = 1e-6, [2e-6]
        steps = {"interpolated": [2e-5, 1e-5], "grid": [2e-5, 1e-5]}
    else:
        h_ref, dead_times = 2.5e-7, [0.0, 2e-6]
        steps = {"interpolated": [1e-6, 5e-6, 1e-5, 2e-5, 2.5e-5], "grid": [1e-6, 5e-6, 1e-5, 2e-5, 2.5e-5]}
    rows, table, runs = [], {}, {}
    for dead in dead_times:
        reference = lib.run(circuit(h_ref, "interpolated", dead))
        runs[("reference", dead)] = reference
        events_ref = len(reference.summary.get("all_events", reference.summary.get("events", [])))
        for switching, hs in steps.items():
            for h in hs:
                r = lib.run(circuit(h, switching, dead))
                runs[(switching, dead, h)] = r
                e = errors(r, h, reference)
                solver = r.summary["solver"]
                events = len(r.summary.get("all_events", r.summary.get("events", [])))
                rows.append((switching, dead, h, e["row"], e["average"], e["average"] / RATED_PEAK, e["amplitude"],
                             e["phase_deg"], events, events_ref, solver.get("instants_within_steps", 0)))
                table[(switching, dead, h)] = e
    lib.write_csv(ctx, "pwm_accuracy.csv",
                  ["switching", "dead_time_s", "time_step_s", "max_row_error_a", "max_carrier_average_error_a",
                   "carrier_average_error_of_rated_peak", "fundamental_amplitude_error", "fundamental_phase_error_deg",
                   "events", "events_of_reference", "instants_within_steps"], rows)

    fig, (ax1, ax2) = style.figure(2, 1, width="double", height_in=3.1)
    k = 0
    for switching, label in (("interpolated", "Within the step"), ("grid", "Grid points")):
        for dead in dead_times:
            hs = steps[switching]
            ax1.loglog(np.array(hs) * 1e6, [table[(switching, dead, h)]["average"] for h in hs],
                       label=f"{label}, dead time {dead * 1e6:g} µs", **style.series(k % 4))
            k += 1
    hs = np.array(steps["interpolated"]) * 1e6
    base = table[("interpolated", dead_times[0], steps["interpolated"][0])]["average"]
    ax1.loglog(hs, base * (hs / hs[0]) ** 2, label="Slope 2", **style.reference(dotted=True))
    style.step_axis(ax1, hs, ylabel="Error of carrier averages (A)", ticks=[1, 5, 10, 20, 25] if not ctx.quick else [10, 20])
    style.legend_below(ax1)
    style.panel(ax1, "a")

    # Phase a over two carrier periods from 50 ms: the reference, 20 us
    # within the step and 20 us at grid points.
    dead = dead_times[-1]
    a, b = 0.05, 0.05 + 2 / FC
    for label, r, kw in (("Reference, 0.25 µs" if not ctx.quick else "Reference", runs[("reference", dead)], style.reference()),
                         ("Within the step, 20 µs", runs[("interpolated", dead, 2e-5)], style.series(0, line=False)),
                         ("Grid points, 20 µs", runs[("grid", dead, 2e-5)], style.series(1, marker=False))):
        t = r.time
        keep = (t >= a - 1e-12) & (t <= b + 1e-12)
        if label.startswith("Within") or label.startswith("Grid"):
            stride = max(1, round(2e-5 / (t[1] - t[0])))
            idx = np.nonzero(keep)[0][::stride]
        else:
            idx = np.nonzero(keep)[0]
        ax2.plot((t[idx] - a) * 1e6, r["I(LF.a)"][idx], label=label, **kw)
    ax2.set_xlabel("Time from 50 ms (µs)")
    ax2.set_ylabel("Phase current $i_a$ (A)")
    style.legend_below(ax2)
    style.panel(ax2, "b")
    lib.save_figure(ctx, fig, "pwm_accuracy")

    out: dict = {"rated peak current (A)": lib.sig(RATED_PEAK, 4)}
    for (switching, dead, h), e in table.items():
        key = f"{switching}, dead time {dead * 1e6:g} us, h {h * 1e6:g} us"
        out[f"{key}: carrier average (A), amplitude, phase (deg)"] = [
            lib.sig(e["average"], 3), lib.sig(e["amplitude"], 3), lib.sig(e["phase_deg"], 3)]
    if not ctx.quick:
        for dead in dead_times:
            out[f"interpolated, dead time {dead * 1e6:g} us: ratio of carrier averages 20 us / 10 us"] = round(
                table[("interpolated", dead, 2e-5)]["average"] / table[("interpolated", dead, 1e-5)]["average"], 2)
    return out
