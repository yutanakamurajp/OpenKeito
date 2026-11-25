"""Format 0.8 `expression` control block verification against closed-form formulas,
explicit block decompositions, closed-loop ODE convergence, and interpolated switching preview:

(a) Single-input (`2*x^2 + sin(x)`) and multi-input (`hypot(x, y)`, `atan2(y, x)`)
    expressions compared against exact NumPy/analytic evaluation and explicit
    block decompositions (`product`/`gain`/`sum`/`function`).
(b) `variables` mapping with dotted signals (`dq.d`, `dq.q` from `abc_to_dq0`),
    built-in constants (`pi`, `e`), and right-associative exponentiation (`^`)
    compared against closed-form formulas and explicit blocks within 1e-14.
(c) Second-order step-size convergence of a nonlinear closed loop (`dx/dt = -x^2`,
    `x(0) = 1`, exact `1 / (1 + t)`) using `expression` + `integrator`.
(d) Sub-step `preview` under `"switching": "interpolated"`: PWM comparator driven
    by `expression` vs explicit blocks, plus nonlinear `u^2` threshold crossing at
    exact sub-step time `t* = 0.537 ms` (interpolated vs grid switching).
"""

from __future__ import annotations

import math

import numpy as np

import lib
import style

TITLE = "Format 0.8 expression control block: exactness, dotted variables, closed-loop order, and switching preview"
MINIMAL_CIRCUIT = [
    {"type": "voltage_source", "name": "E0", "nodes": ["n0", "gnd"], "waveform": {"type": "dc", "value": 1.0}},
    {"type": "resistor", "name": "R0", "nodes": ["n0", "gnd"], "resistance_ohm": 1.0},
]


def _algebraic_verification(ctx: lib.Context) -> tuple[dict, np.ndarray, dict[str, np.ndarray]]:
    """Verifies single/multi-input expressions, dotted variables, constants, and ^ precedence."""
    h = 5e-4 if ctx.quick else 2e-4
    end = 0.02
    m = lib.model(
        {"time_step_s": h, "end_time_s": end, "method": "trapezoidal"},
        MINIMAL_CIRCUIT,
        [{"type": "signal", "signal": s} for s in (
            "x", "y", "c", "dq.d", "dq.q",
            "poly_sin", "poly_sin_blocks",
            "norm_hypot", "norm_hypot_blocks",
            "ang_atan2", "ang_atan2_blocks",
            "dq_expr", "dq_blocks",
            "const_pow",
        )],
        controls=[
            {"type": "ramp", "name": "x", "initial": -1.0, "slope_per_s": 150.0},
            {"type": "sine_wave", "name": "y", "amplitude": 1.2, "frequency_hz": 60.0, "phase_deg": 30.0},
            {"type": "constant", "name": "c", "value": 2.0},
            {"type": "sine_wave", "name": "va", "amplitude": 1.0, "frequency_hz": 50.0, "phase_deg": 0.0},
            {"type": "sine_wave", "name": "vb", "amplitude": 1.0, "frequency_hz": 50.0, "phase_deg": -120.0},
            {"type": "sine_wave", "name": "vc", "amplitude": 1.0, "frequency_hz": 50.0, "phase_deg": 120.0},
            {"type": "ramp", "name": "theta", "initial": 0.0, "slope_per_s": 2.0 * math.pi * 45.0},
            {"type": "abc_to_dq0", "name": "dq", "inputs": ["va", "vb", "vc"], "angle": "theta"},
            # 1) Single-input: 2*x^2 + sin(x) vs explicit blocks
            {"type": "expression", "name": "poly_sin", "inputs": ["x"], "expression": "2*x^2 + sin(x)"},
            {"type": "product", "name": "x_sq", "inputs": ["x", "x"]},
            {"type": "gain", "name": "two_x_sq", "input": "x_sq", "gain": 2.0},
            {"type": "function", "name": "sin_x", "function": "sin", "inputs": ["x"]},
            {"type": "sum", "name": "poly_sin_blocks", "inputs": ["two_x_sq", "sin_x"], "signs": [1.0, 1.0]},
            # 2) Multi-input: hypot(x, y) & atan2(y, x) vs explicit function blocks
            {"type": "expression", "name": "norm_hypot", "inputs": ["x", "y"], "expression": "hypot(x, y)"},
            {"type": "function", "name": "norm_hypot_blocks", "function": "hypot", "inputs": ["x", "y"]},
            {"type": "expression", "name": "ang_atan2", "inputs": ["x", "y"], "expression": "atan2(y, x)"},
            {"type": "function", "name": "ang_atan2_blocks", "function": "atan2", "inputs": ["y", "x"]},
            # 3) Dotted variables (dq.d, dq.q) via variables map vs explicit blocks
            {
                "type": "expression",
                "name": "dq_expr",
                "variables": {"vd": "dq.d", "vq": "dq.q"},
                "expression": "hypot(vd, vq) + 0.1*vd - 0.2*vq",
            },
            {"type": "function", "name": "dq_norm_blk", "function": "hypot", "inputs": ["dq.d", "dq.q"]},
            {"type": "gain", "name": "dq_d_term", "input": "dq.d", "gain": 0.1},
            {"type": "gain", "name": "dq_q_term", "input": "dq.q", "gain": -0.2},
            {
                "type": "sum",
                "name": "dq_blocks",
                "inputs": ["dq_norm_blk", "dq_d_term", "dq_q_term"],
                "signs": [1.0, 1.0, 1.0],
            },
            # 4) Built-in constants (pi, e) and right-associative ^ precedence (-c^2 + c^-2 + 2^3^2)
            {
                "type": "expression",
                "name": "const_pow",
                "inputs": ["c"],
                "expression": "-c^2 + c^-2 + 2^3^2 + pi + e",
            },
        ],
        version="0.8",
    )
    r = lib.run(m)
    t = r.time
    x = r["S(x)"]
    y = r["S(y)"]
    vd = r["S(dq.d)"]
    vq = r["S(dq.q)"]

    poly_exact = 2.0 * x ** 2 + np.sin(x)
    hypot_exact = np.hypot(x, y)
    atan2_exact = np.arctan2(y, x)
    dq_exact = np.hypot(vd, vq) + 0.1 * vd - 0.2 * vq
    const_pow_exact = np.full_like(t, -(2.0 ** 2) + 2.0 ** (-2) + 2.0 ** (3.0 ** 2) + math.pi + math.e)

    errors = {
        "poly_sin_vs_exact": float(np.max(np.abs(r["S(poly_sin)"] - poly_exact))),
        "poly_sin_vs_blocks": float(np.max(np.abs(r["S(poly_sin)"] - r["S(poly_sin_blocks)"]))),
        "hypot_vs_exact": float(np.max(np.abs(r["S(norm_hypot)"] - hypot_exact))),
        "hypot_vs_blocks": float(np.max(np.abs(r["S(norm_hypot)"] - r["S(norm_hypot_blocks)"]))),
        "atan2_vs_exact": float(np.max(np.abs(r["S(ang_atan2)"] - atan2_exact))),
        "atan2_vs_blocks": float(np.max(np.abs(r["S(ang_atan2)"] - r["S(ang_atan2_blocks)"]))),
        "dq_variables_vs_exact": float(np.max(np.abs(r["S(dq_expr)"] - dq_exact))),
        "dq_variables_vs_blocks": float(np.max(np.abs(r["S(dq_expr)"] - r["S(dq_blocks)"]))),
        "constants_pow_vs_exact": float(np.max(np.abs(r["S(const_pow)"] - const_pow_exact))),
    }

    every = 1 if ctx.quick else 2
    lib.write_csv(
        ctx,
        "expression_control.csv",
        [
            "time_s",
            "x",
            "y",
            "poly_sin",
            "poly_sin_exact",
            "poly_sin_blocks",
            "norm_hypot",
            "norm_hypot_exact",
            "norm_hypot_blocks",
            "ang_atan2",
            "ang_atan2_exact",
            "ang_atan2_blocks",
            "dq_d",
            "dq_q",
            "dq_expr",
            "dq_exact",
            "dq_blocks",
            "const_pow",
            "const_pow_exact",
        ],
        zip(
            t[::every],
            x[::every],
            y[::every],
            r["S(poly_sin)"][::every],
            poly_exact[::every],
            r["S(poly_sin_blocks)"][::every],
            r["S(norm_hypot)"][::every],
            hypot_exact[::every],
            r["S(norm_hypot_blocks)"][::every],
            r["S(ang_atan2)"][::every],
            atan2_exact[::every],
            r["S(ang_atan2_blocks)"][::every],
            vd[::every],
            vq[::every],
            r["S(dq_expr)"][::every],
            dq_exact[::every],
            r["S(dq_blocks)"][::every],
            r["S(const_pow)"][::every],
            const_pow_exact[::every],
        ),
    )

    waves = {
        "x": x,
        "y": y,
        "poly_sin": r["S(poly_sin)"],
        "poly_exact": poly_exact,
        "norm_hypot": r["S(norm_hypot)"],
        "hypot_exact": hypot_exact,
        "ang_atan2": r["S(ang_atan2)"],
        "atan2_exact": atan2_exact,
    }
    return errors, t, waves


def _closed_loop_convergence(ctx: lib.Context) -> tuple[dict, list[float], dict[str, list[float]]]:
    """Second-order convergence of closed-loop ODE dx/dt = -x^2, x(0) = 1 -> 1/(1+t)."""
    steps = [400e-6, 200e-6, 100e-6] if ctx.quick else [400e-6, 200e-6, 100e-6, 50e-6]
    errs: dict[str, list[float]] = {}
    orders: dict[str, list[float]] = {}
    rows = []
    for method, _ in lib.METHODS:
        e_list = []
        for h in steps:
            m = lib.model(
                {"time_step_s": h, "end_time_s": 1.0, "method": method},
                MINIMAL_CIRCUIT,
                [{"type": "signal", "signal": "x"}],
                controls=[
                    {"type": "expression", "name": "rhs", "inputs": ["x"], "expression": "-x^2"},
                    {"type": "integrator", "name": "x", "input": "rhs", "gain_per_s": 1.0, "initial_value": 1.0},
                ],
                version="0.8",
            )
            r = lib.run(m)
            exact = 1.0 / (1.0 + r.time)
            e_list.append(float(np.max(np.abs(r["S(x)"] - exact))))
        o_list = [float("nan")] + lib.order(e_list)
        errs[method] = e_list
        orders[method] = o_list[1:]
        rows += [(method, h, e_list[k], o_list[k]) for k, h in enumerate(steps)]

    lib.write_csv(
        ctx,
        "expression_control_convergence.csv",
        ["method", "time_step_s", "max_error_x", "order_x"],
        rows,
    )
    metrics = {
        "steps_us": [int(round(h * 1e6)) for h in steps],
        "orders_2s_dirk": [round(x, 3) for x in orders["2s-dirk"]],
        "orders_trapezoidal": [round(x, 3) for x in orders["trapezoidal"]],
        "finest_error_2s_dirk": lib.sig(errs["2s-dirk"][-1]),
        "finest_error_trapezoidal": lib.sig(errs["trapezoidal"][-1]),
    }
    return metrics, steps, errs


def _interpolated_switching_preview(ctx: lib.Context) -> tuple[dict, np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    """Tests expression preview under interpolated switching (PWM + nonlinear u^2 crossing)."""
    h = 1e-4

    def pwm_model(controls: list[dict]) -> dict:
        return lib.model(
            {"time_step_s": h, "end_time_s": 0.02, "switching": "interpolated"},
            [
                {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 24.0}},
                {"type": "controlled_switch", "name": "S1", "nodes": ["n1", "n2"], "gate": "pwm"},
                {"type": "resistor", "name": "R1", "nodes": ["n2", "n3"], "resistance_ohm": 4.0},
                {"type": "inductor", "name": "L1", "nodes": ["n3", "gnd"], "inductance_h": 0.01},
            ],
            [{"type": "branch_current", "component": "L1"}, {"type": "signal", "signal": "ref"}],
            controls=controls,
            version="0.8",
        )

    r_expr = lib.run(pwm_model([
        {"type": "sine_wave", "name": "u", "amplitude": 0.6, "frequency_hz": 50.0},
        {"type": "expression", "name": "ref", "inputs": ["u"], "expression": "0.5 * u + 0.1"},
        {"type": "triangle_wave", "name": "carrier", "frequency_hz": 1300.0, "minimum": -1.0, "maximum": 1.0},
        {"type": "comparator", "name": "pwm", "inputs": ["ref", "carrier"]},
    ]))
    r_blocks = lib.run(pwm_model([
        {"type": "sine_wave", "name": "u", "amplitude": 0.6, "frequency_hz": 50.0},
        {"type": "gain", "name": "scaled", "input": "u", "gain": 0.5},
        {"type": "constant", "name": "bias", "value": 0.1},
        {"type": "sum", "name": "ref", "inputs": ["scaled", "bias"], "signs": [1.0, 1.0]},
        {"type": "triangle_wave", "name": "carrier", "frequency_hz": 1300.0, "minimum": -1.0, "maximum": 1.0},
        {"type": "comparator", "name": "pwm", "inputs": ["ref", "carrier"]},
    ]))
    pwm_i_diff = float(np.max(np.abs(r_expr["I(L1)"] - r_blocks["I(L1)"])))
    ev_expr = r_expr.summary["events"]
    ev_blocks = r_blocks.summary["events"]
    pwm_t_diff = max(abs(a["time_s"] - b["time_s"]) for a, b in zip(ev_expr, ev_blocks))

    # Nonlinear u^2 crossing threshold 0.537^2 -> exact t* = 0.537 ms on RL load (tau = 0.1 ms).
    t_star_exact = 0.537e-3
    threshold = 0.537 ** 2

    def nl_model(switching: str) -> dict:
        return lib.model(
            {"time_step_s": h, "end_time_s": 1e-3, "switching": switching, "method": "trapezoidal"},
            [
                {"type": "voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "waveform": {"type": "dc", "value": 10.0}},
                {"type": "controlled_switch", "name": "S1", "nodes": ["n1", "n2"], "gate": "gate",
                 "on_resistance_ohm": 1e-6, "off_resistance_ohm": 1e9},
                {"type": "resistor", "name": "R1", "nodes": ["n2", "n3"], "resistance_ohm": 10.0},
                {"type": "inductor", "name": "L1", "nodes": ["n3", "gnd"], "inductance_h": 1e-3},
            ],
            [{"type": "branch_current", "component": "L1"}, {"type": "signal", "signal": "sq"}],
            controls=[
                {"type": "ramp", "name": "u", "slope_per_s": 1000.0},
                {"type": "expression", "name": "sq", "inputs": ["u"], "expression": "u^2"},
                {"type": "constant", "name": "th", "value": threshold},
                {"type": "comparator", "name": "gate", "inputs": ["sq", "th"]},
            ],
            version="0.8",
        )

    r_interp = lib.run(nl_model("interpolated"))
    r_grid = lib.run(nl_model("grid"))
    t_nl = r_interp.time
    i_exact = np.where(t_nl > t_star_exact, 1.0 - np.exp(-(t_nl - t_star_exact) / 1e-4), 0.0)
    t_star_interp = float(r_interp.summary["events"][0]["time_s"])
    t_star_grid = float(r_grid.summary["events"][0]["time_s"])

    lib.write_csv(
        ctx,
        "expression_control_switching.csv",
        ["time_s", "i_l_interpolated_a", "i_l_grid_a", "i_l_exact_a", "sq"],
        zip(t_nl, r_interp["I(L1)"], r_grid["I(L1)"], i_exact, r_interp["S(sq)"]),
    )

    metrics = {
        "pwm_split_steps": r_expr.summary["solver"]["split_steps"],
        "pwm_instants_within_steps": r_expr.summary["solver"]["instants_within_steps"],
        "pwm_max_i_diff_vs_blocks_a": lib.sig(pwm_i_diff),
        "pwm_max_event_time_diff_s": lib.sig(pwm_t_diff),
        "nonlinear_crossing_exact_s": t_star_exact,
        "nonlinear_crossing_interpolated_s": t_star_interp,
        "nonlinear_crossing_grid_s": t_star_grid,
        "nonlinear_crossing_error_s": lib.sig(abs(t_star_interp - t_star_exact)),
        "nonlinear_i_l_max_err_interpolated_a": lib.sig(float(np.max(np.abs(r_interp["I(L1)"] - i_exact)))),
        "nonlinear_i_l_max_err_grid_a": lib.sig(float(np.max(np.abs(r_grid["I(L1)"] - i_exact)))),
    }
    return metrics, t_nl, r_interp["I(L1)"], r_grid["I(L1)"], i_exact


def make(ctx: lib.Context) -> dict:
    alg_errors, t_alg, waves = _algebraic_verification(ctx)
    conv_metrics, steps, conv_errs = _closed_loop_convergence(ctx)
    sw_metrics, t_nl, i_interp, i_grid, i_exact = _interpolated_switching_preview(ctx)

    fig, ((ax1, ax2), (ax3, ax4)) = style.figure(2, 2, width="double", height_in=4.7)

    # (a) Single-input & multi-input expressions vs closed-form
    ms_alg = t_alg * 1e3
    stride = max(1, len(ms_alg) // 20)
    ax1.plot(ms_alg, waves["poly_exact"], **style.reference())
    ax1.plot(ms_alg, waves["hypot_exact"], **style.reference())
    ax1.plot(ms_alg, waves["atan2_exact"], **style.reference())
    ax1.plot(ms_alg[::stride], waves["poly_sin"][::stride], label=r"$2x^2 + \sin(x)$", **style.series(0, line=False))
    ax1.plot(ms_alg[::stride], waves["norm_hypot"][::stride], label=r"$\mathrm{hypot}(x, y)$", **style.series(1, line=False))
    ax1.plot(ms_alg[::stride], waves["ang_atan2"][::stride], label=r"$\mathrm{atan2}(y, x)$", **style.series(2, line=False))
    ax1.set_xlabel("Time (ms)")
    ax1.set_ylabel("Output signal")
    ax1.legend(loc="upper left")
    style.panel(ax1, "a")

    # (b) Machine-precision residuals across all algebraic/variable/constant checks
    check_labels = [
        ("poly_sin vs exact", alg_errors["poly_sin_vs_exact"]),
        ("poly_sin vs blocks", alg_errors["poly_sin_vs_blocks"]),
        ("hypot vs exact", alg_errors["hypot_vs_exact"]),
        ("hypot vs blocks", alg_errors["hypot_vs_blocks"]),
        ("atan2 vs exact", alg_errors["atan2_vs_exact"]),
        ("atan2 vs blocks", alg_errors["atan2_vs_blocks"]),
        ("dq.d/q vars vs exact", alg_errors["dq_variables_vs_exact"]),
        ("dq.d/q vars vs blocks", alg_errors["dq_variables_vs_blocks"]),
        ("pi, e, ^ vs exact", alg_errors["constants_pow_vs_exact"]),
    ]
    y_pos = np.arange(len(check_labels))[::-1]
    floor = 1e-17
    vals = [max(v, floor) for _, v in check_labels]
    ax2.semilogx(vals, y_pos, label="Measured max error", **style.series(0, line=False))
    ax2.axvline(1e-14, label="1e-14 tolerance", **style.reference(dotted=True))
    ax2.set_yticks(y_pos)
    ax2.set_yticklabels([lbl for lbl, _ in check_labels], fontsize=6.5)
    ax2.set_xlim(5e-18, 1e-13)
    ax2.set_xlabel("Max absolute error")
    ax2.legend(loc="lower right")
    style.panel(ax2, "b")

    # (c) Closed-loop ODE dx/dt = -x^2 second-order convergence
    hs = np.array(steps) * 1e6
    for k, (method, label) in enumerate(lib.METHODS):
        ax3.loglog(hs, conv_errs[method], label=label, **style.series(k))
    ref_line = conv_errs["2s-dirk"][0] * 2.0 * (np.array(steps) / steps[0]) ** 2
    ax3.loglog(hs, ref_line, label=r"$\mathcal{O}(h^2)$ slope", **style.reference(dotted=True))
    style.step_axis(ax3, hs, ylabel=r"Max error of $x(t)$ vs $1/(1+t)$")
    ax3.legend(loc="lower right")
    style.panel(ax3, "c")

    # (d) Interpolated switching preview: u^2 crossing threshold at t* = 0.537 ms
    t_fine = np.linspace(0.35e-3, 1.0e-3, 651)
    i_exact_fine = np.where(t_fine > 0.537e-3, 1.0 - np.exp(-(t_fine - 0.537e-3) / 1e-4), 0.0)
    ms_nl = t_nl * 1e3
    ax4.plot(t_fine * 1e3, i_exact_fine, label=r"Exact ($t^* = 0.537\;\mathrm{ms}$)", **style.reference())
    ax4.plot(ms_nl, i_interp, label=r"Interpolated ($h = 100\;\mu\mathrm{s}$)", **style.series(0))
    ax4.plot(ms_nl, i_grid, label=r"Grid ($h = 100\;\mu\mathrm{s}$)", **style.series(1))
    ax4.set_xlim(0.35, 1.0)
    ax4.set_xlabel("Time (ms)")
    ax4.set_ylabel("Inductor current (A)")
    ax4.legend(loc="upper left")
    style.panel(ax4, "d")

    lib.save_figure(ctx, fig, "expression_control")

    return {
        "max_algebraic_error": lib.sig(max(alg_errors.values())),
        "algebraic_errors": {k: lib.sig(v) for k, v in alg_errors.items()},
        "closed_loop_convergence": conv_metrics,
        "interpolated_switching_preview": sw_metrics,
    }
