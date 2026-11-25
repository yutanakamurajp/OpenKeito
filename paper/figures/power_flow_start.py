"""Start from a power flow: the steady state of the converted model against
the power flow it was made from.

The converters (openkeito.pandapower, openkeito.powsybl) choose the values
of the components so that the phasor solution of the EMT model is the power
flow: generators and external grids as sources E = V + Z I (or synchronous
machines at that operating point), loads and shunts as impedances at the
power-flow voltage, static generators as current injections. The run then
starts with the phasor initialization.

Three differences from the power flow are tabulated per network:

- phasor solution: bus voltage magnitude (pu) and angle (degrees) of the
  phasor solution against the power flow. What is left is the convergence
  tolerance of the power flow (pandapower: 1e-8 MVA) or rounding (PowSyBl,
  where the converter also places what the load flow left unbalanced);
- branch powers (pandapower): |S_emt - S_pf| / max(|S_pf|, 1 MVA) at both
  ends of every line and transformer;
- waveform: the phasors taken from the simulated waveforms by a
  least-squares fit of a sinusoid to the last period of a run of two
  periods (a fit, because a period of 60 Hz is not a whole number of steps
  of 50 us). This adds the discretization error of the time step, of the
  order (w h)^2.

The networks are the MATPOWER cases shipped with pandapower (case9, case39,
case118; Zimmerman et al. 2011) and the examples shipped with pypowsybl
(the CGMES MicroGrid configurations BE and NL of ENTSO-E, IEEE 14, 57 and
118, and the Eurostag tutorial example).
"""

from __future__ import annotations

import cmath
import math

import numpy as np

import lib
import style

TITLE = "Steady state of converted networks against the power flow (pandapower, PowSyBl)"

PANDAPOWER = [("case9", {}), ("case39", {}), ("case118", {}),
              ("case9", {"generators": "machine"}), ("case39", {"generators": "machine"})]
POWSYBL = [("create_micro_grid_be_network", "CGMES MicroGrid BE", {}),
           ("create_micro_grid_nl_network", "CGMES MicroGrid NL", {}),
           ("create_ieee14", "IEEE 14", {}), ("create_ieee57", "IEEE 57", {}), ("create_ieee118", "IEEE 118", {}),
           ("create_eurostag_tutorial_example1_network", "Eurostag example", {}),
           ("create_ieee14", "IEEE 14", {"generators": "machine"})]


def waveform_difference(conv, result) -> tuple[float, float]:
    """Largest difference of the bus voltages taken from the waveforms
    (phase a, last period) from the power flow: (pu, degrees)."""
    f = conv.mapping["frequency_hz"]
    dv = da = 0.0
    for key, name in conv.mapping["buses"].items():
        column = f"V({name}.a)"
        if column not in result:
            continue
        vm, va, vn = conv.power_flow[key]
        v = lib.fitted_phasor(result.time, result[column], f)
        dv = max(dv, abs(abs(v) * math.sqrt(3) / (vn * 1e3) - vm))
        da = max(da, abs((math.degrees(cmath.phase(v)) - va + 180.0) % 360.0 - 180.0))
    return dv, da


def branch_difference(net, conv, result) -> float:
    from openkeito.pandapower import branch_powers

    powers = branch_powers(conv, result)
    worst = 0.0
    for e in conv.mapping["elements"]:
        table, idx = e["element"].split("/")
        if table not in ("line", "trafo"):
            continue
        r = net[f"res_{table}"].loc[int(idx)]
        ends = (("p_from_mw", "q_from_mvar"), ("p_to_mw", "q_to_mvar")) if table == "line" else \
            (("p_hv_mw", "q_hv_mvar"), ("p_lv_mw", "q_lv_mvar"))
        for k, (p, q) in enumerate(ends):
            ref = complex(r[p], r[q])
            worst = max(worst, abs(powers[e["component"]][k] - ref) / max(abs(ref), 1.0))
    return worst


def row_of(tool: str, label: str, model: str, conv, result, branch: float | None) -> tuple[dict, list[str]]:
    c = conv.compare(result)
    dv, da = waveform_difference(conv, result)
    init = result.summary["initialization"]
    h = conv.model["simulation"]["time_step_s"]
    row_notes = [f"{label} ({model}): {w.get('code')} {w.get('message', '')[:160]}" for w in result.warnings]
    row = {"tool": tool, "network": label, "generators": model, "buses": len(c["buses"]),
           "unknowns": result.summary["solver"]["unknowns"],
           "frequency_hz": conv.mapping["frequency_hz"], "time_step_s": h,
           "phasor_max_dv_pu": c["max_abs_dv_pu"], "phasor_max_dangle_deg": c["max_abs_dva_degree"],
           "branch_power_max_relative": branch,
           "waveform_max_dv_pu": dv, "waveform_max_dangle_deg": da,
           "relative_voltage_deviation": init["relative_voltage_deviation"],
           "omega_h_squared": (2 * math.pi * conv.mapping["frequency_hz"] * h) ** 2,
           "warnings": len(result.warnings)}
    return row, row_notes


def make(ctx: lib.Context) -> dict:
    rows, notes, skipped = [], [], []
    try:
        import pandapower.networks as ppn

        from openkeito.pandapower import from_pandapower
    except ImportError:
        skipped.append("pandapower is not installed")
    else:
        for name, options in (PANDAPOWER[:1] if ctx.quick else PANDAPOWER):
            net = getattr(ppn, name)()
            # Two periods, rounded up to whole steps of the converter's 50 us.
            end = math.ceil(2.0 / float(net.f_hz) / 50e-6 - 1e-9) * 50e-6
            conv = from_pandapower(net, end_time_s=end, outputs="all", **options)
            result = lib.run(conv.model)
            row, row_notes = row_of("pandapower", name, options.get("generators", "source"), conv, result,
                                    branch_difference(net, conv, result))
            rows.append(row)
            notes += row_notes
    try:
        import pypowsybl.network as pn

        from openkeito.powsybl import from_powsybl
    except ImportError:
        skipped.append("pypowsybl is not installed")
    else:
        for make_name, label, options in (POWSYBL[2:3] if ctx.quick else POWSYBL):
            net = getattr(pn, make_name)()
            conv = from_powsybl(net, outputs="all", end_time_s=0.04, **options)
            conv.model["simulation"]["end_time_s"] = math.ceil(
                2.0 / conv.mapping["frequency_hz"] / 50e-6 - 1e-9) * 50e-6
            result = lib.run(conv.model)
            row, row_notes = row_of("PowSyBl", label, options.get("generators", "source"), conv, result, None)
            rows.append(row)
            notes += row_notes

    header = list(rows[0]) if rows else ["tool"]
    lib.write_csv(ctx, "power_flow_start.csv", header, [[r[k] for k in header] for r in rows])
    lib.write_table(
        ctx, "power_flow_start_table",
        ["Tool", "Network", "Generators", "Buses", "Unknowns", "Phasor solution: |dV| (pu)", "angle (deg)",
         "Branch S (rel.)", "Waveform: |dV| (pu)", "angle (deg)"],
        [[r["tool"], r["network"], r["generators"], r["buses"], r["unknowns"], lib.sci(r["phasor_max_dv_pu"]),
          lib.sci(r["phasor_max_dangle_deg"]),
          "--" if r["branch_power_max_relative"] is None else lib.sci(r["branch_power_max_relative"]),
          lib.sci(r["waveform_max_dv_pu"]), lib.sci(r["waveform_max_dangle_deg"])] for r in rows],
        "Largest differences of the bus voltages of the converted EMT model from the power flow.")

    fig, ax = style.figure(width="double", height_in=0.9 + 0.2 * len(rows))
    y = np.arange(len(rows))[::-1]
    floor = 1e-16
    ax.semilogx([max(r["phasor_max_dv_pu"], floor) for r in rows], y, label="Phasor solution",
                **style.series(0, line=False))
    ax.semilogx([max(r["waveform_max_dv_pu"], floor) for r in rows], y, label="Waveform, last period",
                **style.series(1, line=False))
    ax.axvline(1e-3, label="0.1 %", **style.reference(dotted=True))
    ax.set_yticks(y)
    ax.set_yticklabels([f"{r['network']} ({r['tool']}{', machines' if r['generators'] == 'machine' else ''})"
                        for r in rows])
    ax.set_ylim(-0.6, len(rows) - 0.4)
    ax.set_xlabel("Largest difference of a bus voltage magnitude from the power flow (pu)")
    ax.legend(loc="lower center", bbox_to_anchor=(0.5, 1.0), ncols=3)
    lib.save_figure(ctx, fig, "power_flow_start")

    out = {}
    for r in rows:
        key = f"{r['tool']} {r['network']}" + (" (machines)" if r["generators"] == "machine" else "")
        out[f"{key}: phasor |dV| pu, angle deg, branch S; waveform |dV| pu, angle deg"] = [
            lib.sig(r[k], 2) if r[k] is not None else None
            for k in ("phasor_max_dv_pu", "phasor_max_dangle_deg", "branch_power_max_relative", "waveform_max_dv_pu",
                      "waveform_max_dangle_deg")]
    out["(w h)^2 at 50 us: 50 Hz, 60 Hz"] = [lib.sig((2 * math.pi * f * 50e-6) ** 2) for f in (50, 60)]
    out["warnings of the runs"] = notes
    if skipped:
        out["skipped"] = skipped
    return out
