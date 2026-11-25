"""WECC generic inverter-based resource (REPCA1 / REECA1 / REGCA1) response
to a balanced three-phase fault in examples/wecc_ibr.json.

Shows:
(a) Pre-fault sinusoidal steady-state initialization, terminal voltage sag
    during the 0.2-0.3 s fault, active/reactive power response, and post-fault
    recovery.
(b) Reactive current injection (iq_gen) and active current reduction (ip_gen)
    governed by the dynamic piecewise-linear limits (vdl1_iq_max, lvpl_max)
    and Q-priority current limit (ip_lim).
"""

from __future__ import annotations

import numpy as np

import lib
import style

TITLE = "WECC generic IBR (REPCA1 / REECA1 / REGCA1): steady start, fault ride-through, and recovery"
T_FAULT, T_CLEAR = 0.2, 0.3


def make(ctx: lib.Context) -> dict:
    m = lib.example("wecc_ibr.json")
    if ctx.quick:
        m["simulation"]["end_time_s"] = 0.36
    m["simulation"]["output_interval_steps"] = 4
    r = lib.run(m)

    t = r.time
    vt = r["S(vt_pu)"]
    p = r["S(p_pu)"]
    q = r["S(q_pu)"]
    ip = r["S(ip_gen)"]
    iq = r["S(iq_gen)"]
    ip_lim = r["S(ip_lim)"]
    vdl1 = r["S(vdl1_iq_max)"]
    lvpl = r["S(lvpl_max)"]
    freq = r["S(pll.frequency_hz)"]

    lib.write_csv(
        ctx,
        "wecc_ibr.csv",
        ["time_s", "vt_pu", "p_pu", "q_pu", "ip_gen", "iq_gen", "ip_lim", "vdl1_iq_max", "lvpl_max",
         "pll_frequency_hz"],
        zip(t, vt, p, q, ip, iq, ip_lim, vdl1, lvpl, freq),
    )

    pre = t < T_FAULT - 1e-9
    fault = (t >= T_FAULT) & (t <= T_CLEAR + 1e-9)
    fault_settled = (t >= T_FAULT + 0.05) & (t <= T_CLEAR - 1e-9)
    post_tail = t >= (0.33 if ctx.quick else 0.45)

    pre_vt_dep = float(np.max(np.abs(vt[pre] - vt[0])))
    pre_p_dep = float(np.max(np.abs(p[pre] - p[0])))
    fault_vt_min = float(np.min(vt[fault]))
    fault_vt_mean = float(np.mean(vt[fault_settled]))
    fault_iq_peak = float(np.max(iq[fault]))
    fault_ip_min = float(np.min(ip[fault]))
    post_vt_mean = float(np.mean(vt[post_tail]))
    post_p_mean = float(np.mean(p[post_tail]))
    post_q_mean = float(np.mean(q[post_tail]))

    fig, (ax1, ax2) = style.figure(2, 1, width="double", height_in=3.3)
    ms = t * 1e3

    ax1.axvspan(T_FAULT * 1e3, T_CLEAR * 1e3, color=style.GRID, alpha=0.35, label="Fault (0.2–0.3 s)")
    ax1.plot(ms, vt, label="Terminal voltage $v_t$", **style.series(0, marker=False))
    ax1.plot(ms, p, label="Active power $p$", **style.series(1, marker=False))
    ax1.plot(ms, q, label="Reactive power $q$", **style.series(2, marker=False))
    ax1.set_xlabel("Time (ms)")
    ax1.set_ylabel("Per unit of 10 MVA, 6.6 kV")
    style.legend_below(ax1, ncols=2)
    style.panel(ax1, "a")

    ax2.axvspan(T_FAULT * 1e3, T_CLEAR * 1e3, color=style.GRID, alpha=0.35, label="Fault (0.2–0.3 s)")
    ax2.plot(ms, ip, label="Active current $i_{p,\\mathrm{gen}}$", **style.series(0, marker=False))
    ax2.plot(ms, iq, label="Reactive current $i_{q,\\mathrm{gen}}$", **style.series(1, marker=False))
    ax2.plot(ms, ip_lim, label="Q-priority limit $i_{p,\\mathrm{lim}}$", **style.series(2, marker=False))
    ax2.plot(ms, lvpl, label="LVPL limit", **style.series(3, marker=False))
    ax2.plot(ms, vdl1, label="VDL1 $i_{q,\\max}$", **style.reference(dotted=True))
    ax2.set_xlabel("Time (ms)")
    ax2.set_ylabel("Current / limit (pu)")
    style.legend_below(ax2, ncols=2)
    style.panel(ax2, "b")

    lib.save_figure(ctx, fig, "wecc_ibr")

    return {
        "pre-fault max departure of vt_pu from t=0 (pu)": lib.sig(pre_vt_dep),
        "pre-fault max departure of p_pu from t=0 (pu)": lib.sig(pre_p_dep),
        "fault minimum vt_pu (pu)": round(fault_vt_min, 4),
        "fault settled mean vt_pu in [0.25, 0.30] s (pu)": round(fault_vt_mean, 4),
        "fault peak iq_gen (pu)": round(fault_iq_peak, 4),
        "fault minimum ip_gen (pu)": round(fault_ip_min, 4),
        "post-fault tail mean vt_pu (pu)": round(post_vt_mean, 4),
        "post-fault tail mean p_pu (pu)": round(post_p_mean, 4),
        "post-fault tail mean q_pu (pu)": round(post_q_mean, 4),
    }
