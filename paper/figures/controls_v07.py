"""Format 0.7 control and initialization features against closed-form and
self-consistent references:

(a) `piecewise_linear` interpolation across breakpoints and clamping outside
    the domain, compared against exact piecewise-linear interpolation, plus
    dynamic `limiter` tracking under time-varying `minimum` and `maximum`
    signals.
(b) Second-order step-size convergence of `lag`, `pi`, and `integrator` under
    dynamic signal limits (`minimum`/`maximum` and `output_min`/`output_max`),
    including IEEE Std 421.5 non-windup clamping while at a limit.
(c) `pi` `anti_windup`: `"conditional"` (default) vs `"state"` (IEEE Std 421.5
    non-windup PI), confirming `"state"` matches the explicit block
    decomposition (`gain` + limited `integrator` + `sum` + `limiter`) and
    comparing recovery when the limit clears.
(d) `"initialization": "exclude"` on a grounded shunt inductor vs included
    shunt: the excluded branch starts from zero state at t = 0 while the rest
    of the circuit begins in sinusoidal steady state.
"""

from __future__ import annotations

import math

import numpy as np

import lib
import style

TITLE = "Format 0.7 control blocks, dynamic signal limits, anti-windup, and excluded shunt initialization"
FREQ = 50.0
MINIMAL_CIRCUIT = [
    {"type": "voltage_source", "name": "E0", "nodes": ["n0", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
    {"type": "resistor", "name": "R0", "nodes": ["n0", "gnd"], "resistance_ohm": 1.0},
]


def _pwl_and_limiter(ctx: lib.Context) -> tuple[dict, np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    """Verify piecewise_linear and dynamic limiter against exact formulas."""
    h = 2e-4 if ctx.quick else 1e-4
    x_bp = [0.0, 0.4, 0.85, 1.0]
    y_bp = [0.2, 0.4, 1.0, 1.1]
    m = lib.model(
        {"time_step_s": h, "end_time_s": 0.04, "method": "trapezoidal"},
        MINIMAL_CIRCUIT,
        [{"type": "signal", "signal": s} for s in ("u", "pwl", "lo", "hi", "lim")],
        controls=[
            {"type": "ramp", "name": "u", "initial": -0.2, "slope_per_s": 40.0},
            {"type": "piecewise_linear", "name": "pwl", "input": "u", "x": x_bp, "y": y_bp},
            {"type": "ramp", "name": "lo", "initial": 0.3, "slope_per_s": 5.0},
            {"type": "step", "name": "hi", "time_s": 0.02, "before": 0.75, "after": 1.02},
            {"type": "limiter", "name": "lim", "input": "pwl", "minimum": "lo", "maximum": "hi"},
        ],
        version="0.7",
    )
    r = lib.run(m)
    u = r["S(u)"]
    pwl = r["S(pwl)"]
    pwl_exact = np.interp(u, x_bp, y_bp)
    pwl_err = float(np.max(np.abs(pwl - pwl_exact)))

    lim = r["S(lim)"]
    lim_exact = np.clip(pwl_exact, r["S(lo)"], r["S(hi)"])
    lim_err = float(np.max(np.abs(lim - lim_exact)))

    every = 2 if ctx.quick else 4
    lib.write_csv(
        ctx,
        "controls_v07_pwl.csv",
        ["time_s", "u", "pwl", "pwl_exact", "lo", "hi", "limiter", "limiter_exact"],
        zip(
            r.time[::every],
            u[::every],
            pwl[::every],
            pwl_exact[::every],
            r["S(lo)"][::every],
            r["S(hi)"][::every],
            lim[::every],
            lim_exact[::every],
        ),
    )
    return {"pwl_max_error": pwl_err, "limiter_max_error": lim_err}, u, pwl, pwl_exact, lim


def _dynamic_limit_convergence(ctx: lib.Context) -> tuple[dict, list[float], dict[str, list[float]]]:
    """Second-order convergence of lag, pi, and integrator under dynamic signal limits."""
    steps = [400e-6, 200e-6, 100e-6] if ctx.quick else [400e-6, 200e-6, 100e-6, 50e-6]

    def lag_err(h: float) -> float:
        m = lib.model(
            {"time_step_s": h, "end_time_s": 0.05, "method": "trapezoidal"},
            MINIMAL_CIRCUIT,
            [{"type": "signal", "signal": "y"}],
            controls=[
                {"type": "constant", "name": "u", "value": 2.0},
                {"type": "ramp", "name": "hi", "initial": 0.2, "slope_per_s": 50.0},
                {
                    "type": "lag",
                    "name": "y",
                    "input": "u",
                    "gain": 1.0,
                    "time_constant_s": 0.01,
                    "maximum": "hi",
                    "initial_input": 0.0,
                },
            ],
            version="0.7",
        )
        r = lib.run(m)
        t = r.time
        keep = t >= 0.026 - 1e-12
        exact = 2.0 - 0.5 * np.exp(-(t[keep] - 0.026) / 0.01)
        return float(np.max(np.abs(r["S(y)"][keep] - exact)))

    def pi_or_int_err(block_type: str, h: float) -> float:
        if block_type == "pi":
            block = {
                "type": "pi",
                "name": "y",
                "input": "e",
                "proportional_gain": 0.0,
                "integral_gain_per_s": 100.0,
                "initial_integral": 0.5,
                "output_max": "hi",
            }
        else:
            block = {
                "type": "integrator",
                "name": "y",
                "input": "e",
                "gain_per_s": 100.0,
                "initial_value": 0.5,
                "maximum": "hi",
            }
        m = lib.model(
            {"time_step_s": h, "end_time_s": 0.02, "method": "trapezoidal"},
            MINIMAL_CIRCUIT,
            [{"type": "signal", "signal": "y"}],
            controls=[
                {"type": "sine_wave", "name": "e", "amplitude": 1.0, "frequency_hz": 25.0, "phase_deg": 90.0},
                {"type": "step", "name": "hi", "time_s": 0.01, "before": 0.5, "after": 2.0},
                block,
            ],
            version="0.7",
        )
        r = lib.run(m)
        t = r.time
        # Before t = 0.01 s, non-windup clamping holds the state at 0.5.
        hold_err = float(np.max(np.abs(r["S(y)"][t < 0.01 - 1e-12] - 0.5)))
        assert hold_err < 1e-12, f"{block_type} failed non-windup hold: {hold_err}"
        keep = t >= 0.01 - 1e-12
        exact = 0.5 + (100.0 / (50.0 * math.pi)) * (np.sin(50.0 * math.pi * t[keep]) - 1.0)
        return float(np.max(np.abs(r["S(y)"][keep] - exact)))

    errs = {
        "lag": [lag_err(h) for h in steps],
        "pi": [pi_or_int_err("pi", h) for h in steps],
        "integrator": [pi_or_int_err("integrator", h) for h in steps],
    }
    orders = {k: lib.order(v) for k, v in errs.items()}

    rows = []
    for k, h in enumerate(steps):
        rows.append((
            h,
            errs["lag"][k],
            float("nan") if k == 0 else orders["lag"][k - 1],
            errs["pi"][k],
            float("nan") if k == 0 else orders["pi"][k - 1],
            errs["integrator"][k],
            float("nan") if k == 0 else orders["integrator"][k - 1],
        ))
    lib.write_csv(
        ctx,
        "controls_v07_convergence.csv",
        ["time_step_s", "lag_error", "lag_order", "pi_error", "pi_order", "integrator_error", "integrator_order"],
        rows,
    )
    metrics = {
        "lag_orders": [round(x, 3) for x in orders["lag"]],
        "pi_orders": [round(x, 3) for x in orders["pi"]],
        "integrator_orders": [round(x, 3) for x in orders["integrator"]],
        "finest_errors": {k: lib.sig(v[-1]) for k, v in errs.items()},
    }
    return metrics, steps, errs


def _anti_windup_comparison(ctx: lib.Context) -> tuple[dict, np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    """Verify PI anti_windup='state' vs block decomposition and compare with 'conditional'."""
    h = 2e-4 if ctx.quick else 1e-4
    # 1) Exact match between anti_windup='state' and explicit block decomposition.
    m_decomp = lib.model(
        {"time_step_s": h, "end_time_s": 0.03, "method": "trapezoidal"},
        MINIMAL_CIRCUIT,
        [{"type": "signal", "signal": s} for s in ("pi_state", "ref_out")],
        controls=[
            {"type": "sine_wave", "name": "e", "amplitude": 1.5, "frequency_hz": 50.0},
            {"type": "constant", "name": "lo", "value": -0.7},
            {"type": "step", "name": "hi", "time_s": 0.015, "before": 0.6, "after": 1.1},
            {
                "type": "pi",
                "name": "pi_state",
                "input": "e",
                "proportional_gain": 0.6,
                "integral_gain_per_s": 120.0,
                "initial_integral": 0.1,
                "output_min": "lo",
                "output_max": "hi",
                "anti_windup": "state",
            },
            {"type": "gain", "name": "p_part", "input": "e", "gain": 0.6},
            {
                "type": "integrator",
                "name": "i_part",
                "input": "e",
                "gain_per_s": 120.0,
                "initial_value": 0.1,
                "minimum": "lo",
                "maximum": "hi",
            },
            {"type": "sum", "name": "raw_sum", "inputs": ["p_part", "i_part"], "signs": [1.0, 1.0]},
            {"type": "limiter", "name": "ref_out", "input": "raw_sum", "minimum": "lo", "maximum": "hi"},
        ],
        version="0.7",
    )
    r_decomp = lib.run(m_decomp)
    decomp_diff = float(np.max(np.abs(r_decomp["S(pi_state)"] - r_decomp["S(ref_out)"])))

    # 2) Recovery difference between 'conditional' and 'state' when error reverses.
    m_cmp = lib.model(
        {"time_step_s": h, "end_time_s": 0.025, "method": "trapezoidal"},
        MINIMAL_CIRCUIT,
        [{"type": "signal", "signal": s} for s in ("e", "cond", "state", "ref_out")],
        controls=[
            {"type": "step", "name": "e", "time_s": 0.015, "before": 1.2, "after": -0.5},
            {
                "type": "pi",
                "name": "cond",
                "input": "e",
                "proportional_gain": 1.0,
                "integral_gain_per_s": 200.0,
                "initial_integral": 0.0,
                "output_min": -1.0,
                "output_max": 1.0,
                "anti_windup": "conditional",
            },
            {
                "type": "pi",
                "name": "state",
                "input": "e",
                "proportional_gain": 1.0,
                "integral_gain_per_s": 200.0,
                "initial_integral": 0.0,
                "output_min": -1.0,
                "output_max": 1.0,
                "anti_windup": "state",
            },
            {"type": "gain", "name": "p_part", "input": "e", "gain": 1.0},
            {
                "type": "integrator",
                "name": "i_part",
                "input": "e",
                "gain_per_s": 200.0,
                "initial_value": 0.0,
                "minimum": -1.0,
                "maximum": 1.0,
            },
            {"type": "sum", "name": "raw_sum", "inputs": ["p_part", "i_part"], "signs": [1.0, 1.0]},
            {"type": "limiter", "name": "ref_out", "input": "raw_sum", "minimum": -1.0, "maximum": 1.0},
        ],
        version="0.7",
    )
    r_cmp = lib.run(m_cmp)
    t = r_cmp.time
    cond = r_cmp["S(cond)"]
    state = r_cmp["S(state)"]
    ref_out = r_cmp["S(ref_out)"]

    after = t >= 0.015
    k_step = int(np.argmax(after))
    # Just after the step at t = 0.015 s (one step later), conditional jumps to -0.5 while state starts near +0.5.
    cond_rec = float(cond[k_step + 1])
    state_rec = float(state[k_step + 1])

    every = 1 if ctx.quick else 2
    lib.write_csv(
        ctx,
        "controls_v07_anti_windup.csv",
        ["time_s", "error_input", "pi_conditional", "pi_state", "block_decomposition"],
        zip(t[::every], r_cmp["S(e)"][::every], cond[::every], state[::every], ref_out[::every]),
    )
    return (
        {
            "state_vs_decomposition_max_diff": lib.sig(decomp_diff),
            "recovery_at_step_conditional": round(cond_rec, 4),
            "recovery_at_step_state": round(state_rec, 4),
        },
        t,
        cond,
        state,
        ref_out,
    )


def _excluded_shunt_initialization(ctx: lib.Context) -> tuple[dict, np.ndarray, np.ndarray, np.ndarray]:
    """Compare initialization='exclude' vs 'include' on a grounded shunt inductor."""
    h = 1e-4 if ctx.quick else 5e-5
    end = 0.04 if ctx.quick else 0.06

    def shunt_model(init_mode: str) -> dict:
        return lib.model(
            {"time_step_s": h, "end_time_s": end, "method": "trapezoidal", "initialization": "steady_state"},
            [
                {
                    "type": "voltage_source",
                    "name": "E1",
                    "nodes": ["n1", "gnd"],
                    "waveform": {"type": "sine", "amplitude": 100.0, "frequency_hz": FREQ},
                },
                {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 10.0},
                {"type": "resistor", "name": "R2", "nodes": ["n2", "gnd"], "resistance_ohm": 10.0},
                {
                    "type": "inductor",
                    "name": "L_shunt",
                    "nodes": ["n2", "gnd"],
                    "inductance_h": 0.05,
                    "initialization": init_mode,
                },
            ],
            [
                {"type": "node_voltage", "node": "n2"},
                {"type": "branch_current", "component": "L_shunt"},
            ],
            version="0.7",
        )

    r_ex = lib.run(shunt_model("exclude"))
    r_in = lib.run(shunt_model("include"))

    phasors_ex = {p["name"]: p for p in r_ex.summary["initialization"]["phasors"]}
    v_n2_ex = phasors_ex["V(n2)"]
    # Without L_shunt, R1=10 and R2=10 form an ideal 0.5x divider on E1 (100 V peak -> 50/sqrt(2) V rms, 0 deg).
    v_n2_expected_rms = 50.0 / math.sqrt(2.0)
    v_n2_phasor_err = abs(v_n2_ex["rms"] - v_n2_expected_rms)

    i_ex = r_ex["I(L_shunt)"]
    i_in = r_in["I(L_shunt)"]
    i_ex_t0 = float(abs(i_ex[0]))
    i_in_t0 = float(abs(i_in[0]))

    every = 2 if ctx.quick else 4
    lib.write_csv(
        ctx,
        "controls_v07_exclude.csv",
        ["time_s", "i_shunt_excluded_a", "i_shunt_included_a", "v_n2_excluded_v", "v_n2_included_v"],
        zip(r_ex.time[::every], i_ex[::every], i_in[::every], r_ex["V(n2)"][::every], r_in["V(n2)"][::every]),
    )
    return (
        {
            "excluded_i_t0_a": lib.sig(i_ex_t0),
            "included_i_t0_a": round(i_in_t0, 4),
            "excluded_v_n2_phasor_rms_error_v": lib.sig(v_n2_phasor_err),
        },
        r_ex.time,
        i_ex,
        i_in,
    )


def make(ctx: lib.Context) -> dict:
    pwl_metrics, u, pwl, pwl_exact, lim = _pwl_and_limiter(ctx)
    conv_metrics, steps, errs = _dynamic_limit_convergence(ctx)
    aw_metrics, t_aw, cond, state, ref_out = _anti_windup_comparison(ctx)
    ex_metrics, t_ex, i_ex, i_in = _excluded_shunt_initialization(ctx)

    lib.write_table(
        ctx,
        "controls_v07_table",
        ["Feature (format 0.7)", "Reference", "Measured metric"],
        [
            ["piecewise_linear", "Exact np.interp", f"max error = {lib.sci(pwl_metrics['pwl_max_error'])}"],
            ["Dynamic limiter", "Exact np.clip(pwl, lo, hi)", f"max error = {lib.sci(pwl_metrics['limiter_max_error'])}"],
            [
                "lag (dynamic max)",
                "Analytic detachment",
                f"order = {conv_metrics['lag_orders'][-1]:.3f} (err {lib.sci(conv_metrics['finest_errors']['lag'])})",
            ],
            [
                "pi (dynamic max)",
                "Analytic non-windup release",
                f"order = {conv_metrics['pi_orders'][-1]:.3f} (err {lib.sci(conv_metrics['finest_errors']['pi'])})",
            ],
            [
                "integrator (dynamic max)",
                "Analytic non-windup release",
                f"order = {conv_metrics['integrator_orders'][-1]:.3f} (err {lib.sci(conv_metrics['finest_errors']['integrator'])})",
            ],
            [
                "pi anti_windup='state'",
                "Explicit block decomposition",
                f"max diff = {lib.sci(aw_metrics['state_vs_decomposition_max_diff'])}",
            ],
            [
                "initialization='exclude'",
                "Zero shunt state at t = 0",
                f"|I(0)| = {lib.sci(ex_metrics['excluded_i_t0_a'])} A (included: {ex_metrics['included_i_t0_a']:.3f} A)",
            ],
        ],
        "Verification of format 0.7 control blocks and initialization exclusion.",
    )

    fig, ((ax1, ax2), (ax3, ax4)) = style.figure(2, 2, width="double", height_in=4.6)

    # (a) piecewise_linear & dynamic limiter
    ax1.plot(u, pwl_exact, label="Exact piecewise linear", **style.reference())
    stride = max(1, len(u) // 25)
    ax1.plot(u[::stride], pwl[::stride], label="piecewise_linear", **style.series(0, line=False))
    ax1.plot(u, lim, label="Dynamic limiter output", **style.series(1, marker=False))
    ax1.set_xlabel("Input $u$")
    ax1.set_ylabel("Output")
    ax1.legend(loc="upper left")
    style.panel(ax1, "a")

    # (b) 2nd-order convergence under dynamic signal limits
    hs = np.array(steps) * 1e6
    for k, (key, label) in enumerate([("lag", "lag"), ("pi", "pi"), ("integrator", "integrator")]):
        ax2.loglog(hs, errs[key], label=label, **style.series(k))
    ref_line = errs["lag"][0] * (np.array(steps) / steps[0]) ** 2
    ax2.loglog(hs, ref_line, label=r"$\mathcal{O}(h^2)$ slope", **style.reference(dotted=True))
    style.step_axis(ax2, hs, ylabel="Max error vs analytic")
    ax2.legend(loc="lower right")
    style.panel(ax2, "b")

    # (c) PI anti-windup: conditional vs state vs block decomposition
    ms_aw = t_aw * 1e3
    ax3.plot(ms_aw, cond, label='anti_windup = "conditional"', **style.series(1, marker=False))
    ax3.plot(ms_aw, state, label='anti_windup = "state"', **style.series(0, marker=False))
    stride_aw = max(1, len(ms_aw) // 25)
    ax3.plot(ms_aw[::stride_aw], ref_out[::stride_aw], label="Explicit block decomposition",
             **style.series(2, line=False))
    ax3.set_xlabel("Time (ms)")
    ax3.set_ylabel("PI output")
    ax3.legend(loc="lower left")
    style.panel(ax3, "c")

    # (d) initialization: exclude vs include on grounded shunt inductor
    ms_ex = t_ex * 1e3
    ax4.plot(ms_ex, i_in, label='initialization = "include"', **style.series(1, marker=False))
    ax4.plot(ms_ex, i_ex, label='initialization = "exclude"', **style.series(0, marker=False))
    ax4.set_xlabel("Time (ms)")
    ax4.set_ylabel("Shunt inductor current (A)")
    ax4.legend(loc="upper right")
    style.panel(ax4, "d")

    lib.save_figure(ctx, fig, "controls_v07")

    return {
        "piecewise_linear max error vs np.interp": lib.sig(pwl_metrics["pwl_max_error"]),
        "dynamic limiter max error vs np.clip": lib.sig(pwl_metrics["limiter_max_error"]),
        "dynamic limit 2nd-order convergence (lag, pi, integrator)": {
            "steps_us": [int(round(h * 1e6)) for h in steps],
            "lag_orders": conv_metrics["lag_orders"],
            "pi_orders": conv_metrics["pi_orders"],
            "integrator_orders": conv_metrics["integrator_orders"],
            "finest_errors": conv_metrics["finest_errors"],
        },
        "pi anti_windup": aw_metrics,
        "initialization exclude": ex_metrics,
    }
