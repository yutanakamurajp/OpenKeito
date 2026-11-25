"""Python callback control interface verification: closed-loop equivalence
with built-in blocks and periodic zero-order hold (ZOH) actuation.

(a) Series RLC driven by a controlled voltage source under PI regulation
    (Kp = 2.0, Ki = 250.0 1/s, trapezoidal integration): Python step callback
    compared against OpenKeito's built-in sum/pi blocks at every grid point
    (20 us step) within 1e-12.
(b) Periodic control updates with zero-order hold (`control_interval_steps`
    in {1, 10, 25}, corresponding to 20 us, 200 us, and 500 us control periods)
    and exact callback count verification (`1 + steps // interval`).
"""

from __future__ import annotations

import numpy as np
import openkeito

import lib
import style

TITLE = "Python AI control interface: closed-loop parity and periodic ZOH updates"


def _rlc_pi_model(h: float, end: float, kp: float, ki: float, use_builtin_pi: bool) -> dict:
    components = [
        {"type": "controlled_voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "signal": "u"},
        {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 2.0},
        {"type": "inductor", "name": "L1", "nodes": ["n2", "n3"], "inductance_h": 2e-3},
        {"type": "capacitor", "name": "C1", "nodes": ["n3", "gnd"], "capacitance_f": 2e-4},
    ]
    controls: list[dict] = [
        {"type": "measurement", "name": "vc", "quantity": {"type": "node_voltage", "node": "n3"}},
    ]
    if use_builtin_pi:
        controls.extend([
            {"type": "constant", "name": "vref", "value": 10.0},
            {"type": "sum", "name": "err", "inputs": ["vref", "vc"], "signs": [1.0, -1.0]},
            {"type": "pi", "name": "u", "input": "err", "proportional_gain": kp, "integral_gain_per_s": ki},
        ])
    else:
        controls.append({"type": "constant", "name": "u", "value": 0.0})
    return lib.model(
        {"time_step_s": h, "end_time_s": end, "method": "2s-dirk"},
        components,
        [
            {"type": "node_voltage", "node": "n3"},
            {"type": "signal", "signal": "vc"},
            {"type": "signal", "signal": "u"},
        ],
        controls=controls,
        version="0.8",
    )


def _run_python_pi(model: dict, h: float, kp: float, ki: float, interval_steps: int) -> tuple[openkeito.Result, int]:
    dt = h * interval_steps
    integral = 0.0
    prev_err: float | None = None
    calls = 0

    def ctrl(_t: float, state: dict[str, float]) -> dict[str, float]:
        nonlocal integral, prev_err, calls
        calls += 1
        err = 10.0 - state["vc"]
        if prev_err is not None:
            integral += 0.5 * ki * dt * (err + prev_err)
        prev_err = err
        return {"u": kp * err + integral}

    res = openkeito.run(model, controller=ctrl, control_interval_steps=interval_steps)
    return res, calls


def make(ctx: lib.Context) -> dict:
    h = 2e-5
    end = 0.008 if ctx.quick else 0.02
    kp, ki = 1.5, 200.0

    ref_model = _rlc_pi_model(h, end, kp, ki, use_builtin_pi=True)
    ref_res = lib.run(ref_model)

    py_model = _rlc_pi_model(h, end, kp, ki, use_builtin_pi=False)
    py_step1, calls_step1 = _run_python_pi(py_model, h, kp, ki, interval_steps=1)

    diff_vc = np.abs(py_step1["V(n3)"] - ref_res["V(n3)"])
    diff_u = np.abs(py_step1["S(u)"] - ref_res["S(u)"])
    max_vc_diff = float(np.max(diff_vc))
    max_u_diff = float(np.max(diff_u))

    lib.write_csv(
        ctx,
        "ai_control.csv",
        ["time_s", "v_c_builtin_v", "v_c_python_v", "u_builtin_v", "u_python_v", "abs_diff_vc_v"],
        zip(
            ref_res.time,
            ref_res["V(n3)"],
            py_step1["V(n3)"],
            ref_res["S(u)"],
            py_step1["S(u)"],
            diff_vc,
        ),
    )

    intervals = [1, 10, 25]
    period_results = {}
    period_summary = {}
    for iv in intervals:
        res_iv, calls_iv = _run_python_pi(py_model, h, kp, ki, interval_steps=iv)
        steps = int(res_iv.summary["solver"]["steps"])
        expected_calls = 1 + steps // iv
        assert calls_iv == expected_calls, (iv, calls_iv, expected_calls)
        period_results[iv] = res_iv
        period_summary[str(iv)] = {
            "control_period_s": iv * h,
            "calls": calls_iv,
            "final_v_c_v": float(res_iv["V(n3)"][-1]),
        }

    lib.write_csv(
        ctx,
        "ai_control_periods.csv",
        ["time_s", "v_c_step1_v", "v_c_step10_v", "v_c_step25_v", "u_step1_v", "u_step10_v", "u_step25_v"],
        zip(
            ref_res.time,
            period_results[1]["V(n3)"],
            period_results[10]["V(n3)"],
            period_results[25]["V(n3)"],
            period_results[1]["S(u)"],
            period_results[10]["S(u)"],
            period_results[25]["S(u)"],
        ),
    )

    fig, (ax1, ax2) = style.figure(2, 1, width="double", height_in=2.7)
    t_ms = ref_res.time * 1e3
    ax1.plot(t_ms, ref_res["V(n3)"], label="Built-in PI blocks", **style.reference())
    ax1.plot(t_ms, py_step1["V(n3)"], label="Python callback (1 step)", **style.series(0, marker=False))
    ax1.axhline(10.0, **style.reference(dotted=True))
    ax1.set_xlabel("Time (ms)")
    ax1.set_ylabel("Capacitor voltage (V)")
    style.legend_below(ax1, ncols=1)
    style.panel(ax1, "a")

    for k, iv in enumerate(intervals):
        period_us = int(round(iv * h * 1e6))
        ax2.plot(
            t_ms,
            period_results[iv]["S(u)"],
            label=f"Period {period_us} us ({iv} steps)",
            **style.series(k, marker=False),
        )
    ax2.set_xlabel("Time (ms)")
    ax2.set_ylabel("Controller output u (V)")
    style.legend_below(ax2, ncols=1)
    style.panel(ax2, "b")
    lib.save_figure(ctx, fig, "ai_control")

    return {
        "max_abs_diff_vc_v": max_vc_diff,
        "max_abs_diff_u_v": max_u_diff,
        "step1_calls": calls_step1,
        "periodic_zoh": period_summary,
    }
