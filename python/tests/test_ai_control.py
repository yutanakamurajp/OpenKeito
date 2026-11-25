"""Tests for the Python step/periodic control callback (`openkeito.run(..., controller=...)`)."""

from __future__ import annotations

import copy
import json
import math
import time
from pathlib import Path
from types import MappingProxyType

import numpy as np
import pytest

import openkeito

ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = ROOT / "examples"


def _rlc_p_models(end_time_s: float = 0.005, kp: float = 3.5, v_ref: float = 10.0):
    base = {
        "format": "openkeito-model",
        "version": "0.5",
        "simulation": {"time_step_s": 1e-4, "end_time_s": end_time_s},
        "components": [
            {"type": "controlled_voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "signal": "u"},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 2.0},
            {"type": "inductor", "name": "L1", "nodes": ["n2", "n3"], "inductance_h": 1e-2},
            {"type": "capacitor", "name": "C1", "nodes": ["n3", "gnd"], "capacitance_f": 1e-4},
        ],
        "outputs": [
            {"type": "node_voltage", "node": "n3"},
            {"type": "branch_current", "component": "L1"},
            {"type": "signal", "signal": "u"},
        ],
    }
    builtin = copy.deepcopy(base)
    builtin["controls"] = [
        {"type": "constant", "name": "v_ref", "value": v_ref},
        {"type": "measurement", "name": "v_c", "quantity": {"type": "node_voltage", "node": "n3"}},
        {"type": "sum", "name": "err", "inputs": ["v_ref", "v_c"], "signs": [1.0, -1.0]},
        {"type": "gain", "name": "u", "input": "err", "gain": kp},
    ]
    controlled = copy.deepcopy(base)
    controlled["controls"] = [
        {"type": "measurement", "name": "v_c", "quantity": {"type": "node_voltage", "node": "n3"}},
        {"type": "constant", "name": "u", "value": 0.0},
    ]
    return builtin, controlled


def test_python_p_controller_matches_builtin_gain():
    kp, v_ref = 3.5, 10.0
    builtin, controlled = _rlc_p_models(kp=kp, v_ref=v_ref)
    ref = openkeito.run(builtin)

    calls = 0

    def p_ctrl(t: float, state):
        nonlocal calls
        calls += 1
        assert "V(n3)" in state and "v_c" in state and "S(u)" in state
        return {"u": kp * (v_ref - state["v_c"])}

    res = openkeito.run(controlled, controller=p_ctrl)
    assert calls == len(ref.time)
    assert np.allclose(res["V(n3)"], ref["V(n3)"], rtol=0.0, atol=1e-12)
    assert np.allclose(res["I(L1)"], ref["I(L1)"], rtol=0.0, atol=1e-12)
    assert np.allclose(res["S(u)"], ref["S(u)"], rtol=0.0, atol=1e-12)


def test_python_pi_controller_matches_builtin_pi():
    v_ref, kp, ki, u_min, u_max = 10.0, 2.0, 150.0, -5.0, 15.0
    base = {
        "format": "openkeito-model",
        "version": "0.5",
        "simulation": {"time_step_s": 1e-4, "end_time_s": 0.01},
        "components": [
            {"type": "controlled_voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "signal": "u"},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "n2"], "resistance_ohm": 2.0},
            {"type": "inductor", "name": "L1", "nodes": ["n2", "n3"], "inductance_h": 1e-2},
            {"type": "capacitor", "name": "C1", "nodes": ["n3", "gnd"], "capacitance_f": 1e-4},
        ],
        "outputs": [
            {"type": "node_voltage", "node": "n3"},
            {"type": "signal", "signal": "u"},
        ],
    }
    builtin = copy.deepcopy(base)
    builtin["controls"] = [
        {"type": "constant", "name": "v_ref", "value": v_ref},
        {"type": "measurement", "name": "v_c", "quantity": {"type": "node_voltage", "node": "n3"}},
        {"type": "sum", "name": "err", "inputs": ["v_ref", "v_c"], "signs": [1.0, -1.0]},
        {
            "type": "pi",
            "name": "u",
            "input": "err",
            "proportional_gain": kp,
            "integral_gain_per_s": ki,
            "output_min": u_min,
            "output_max": u_max,
        },
    ]
    controlled = copy.deepcopy(base)
    controlled["controls"] = [
        {"type": "measurement", "name": "v_c", "quantity": {"type": "node_voltage", "node": "n3"}},
        {"type": "constant", "name": "u", "value": 0.0},
    ]

    ref = openkeito.run(builtin)

    integral = 0.0
    prev = None

    def pi_ctrl(t: float, state):
        nonlocal integral, prev
        e = v_ref - state["v_c"]
        if prev is not None:
            e_prev, t_prev = prev
            delta = ki * 0.5 * (e + e_prev) * (t - t_prev)
            unclamped = kp * e + integral + delta
            held = (unclamped > u_max and delta > 0.0) or (unclamped < u_min and delta < 0.0)
            if not held:
                integral += delta
        prev = (e, t)
        u = min(max(kp * e + integral, u_min), u_max)
        return {"u": u}

    res = openkeito.run(controlled, controller=pi_ctrl)
    assert np.allclose(res["V(n3)"], ref["V(n3)"], rtol=0.0, atol=1e-11)
    assert np.allclose(res["S(u)"], ref["S(u)"], rtol=0.0, atol=1e-11)


def test_python_buck_pi_switching_converter_matches_builtin():
    builtin = json.loads((EXAMPLES / "buck_pi.json").read_text(encoding="utf-8"))
    builtin["simulation"]["end_time_s"] = 0.02
    ref = openkeito.run(builtin)

    controlled = copy.deepcopy(builtin)
    controlled["controls"] = [
        {"type": "measurement", "name": "v_out", "quantity": {"type": "node_voltage", "node": "out"}},
        {"type": "constant", "name": "duty", "value": 0.0},
        {"type": "constant", "name": "error", "value": 0.0},
        {"type": "triangle_wave", "name": "carrier", "frequency_hz": 10000.0},
        {"type": "comparator", "name": "pwm", "inputs": ["duty", "carrier"]},
    ]

    v_ref, kp, ki, d_min, d_max = 30.0, 0.001, 3.0, 0.0, 0.95
    integral = 0.0
    prev = None

    def buck_ctrl(t: float, state):
        nonlocal integral, prev
        e = v_ref - state["v_out"]
        if prev is not None:
            e_prev, t_prev = prev
            delta = ki * 0.5 * (e + e_prev) * (t - t_prev)
            unclamped = kp * e + integral + delta
            held = (unclamped > d_max and delta > 0.0) or (unclamped < d_min and delta < 0.0)
            if not held:
                integral += delta
        prev = (e, t)
        duty = min(max(kp * e + integral, d_min), d_max)
        return {"duty": duty, "error": e}

    res = openkeito.run(controlled, controller=buck_ctrl)
    assert np.allclose(res["V(out)"], ref["V(out)"], rtol=0.0, atol=1e-11)
    assert np.allclose(res["I(L1)"], ref["I(L1)"], rtol=0.0, atol=1e-11)
    assert np.allclose(res["S(duty)"], ref["S(duty)"], rtol=0.0, atol=1e-11)


def test_python_interpolated_switching_keeps_overridden_signal():
    builtin = json.loads((EXAMPLES / "buck_interpolated.json").read_text(encoding="utf-8"))
    builtin["simulation"]["end_time_s"] = 0.005
    ref = openkeito.run(builtin)

    # Override a gain block (whose preview() would otherwise recompute from its input)
    # to verify preview_with_overrides preserves ZOH overrides within split steps.
    controlled = copy.deepcopy(builtin)
    controlled["controls"] = [
        {"type": "constant", "name": "zero_in", "value": 0.0},
        {"type": "gain", "name": "duty", "input": "zero_in", "gain": 1.0},
        {"type": "triangle_wave", "name": "carrier", "frequency_hz": 10000.0},
        {"type": "comparator", "name": "pwm", "inputs": ["duty", "carrier"]},
    ]
    res = openkeito.run(controlled, controller=lambda t, s: {"duty": 0.405})
    assert np.allclose(res["V(out)"], ref["V(out)"], rtol=0.0, atol=1e-12)
    assert np.allclose(res["I(L1)"], ref["I(L1)"], rtol=0.0, atol=1e-12)


def test_python_synchronous_machine_avr_matches_builtin():
    builtin = json.loads((EXAMPLES / "machine_avr_governor.json").read_text(encoding="utf-8"))
    builtin["simulation"]["end_time_s"] = 1.5
    ref = openkeito.run(builtin)

    controlled = copy.deepcopy(builtin)
    # Replace the AVR lag block (`efd`) with a constant placeholder driven by Python.
    controlled["controls"] = [
        c if c["name"] != "efd" else {"type": "constant", "name": "efd", "value": 0.0}
        for c in controlled["controls"]
    ]

    h = controlled["simulation"]["time_step_s"]
    ka, ta = 200.0, 0.02
    a = h / (2.0 * ta)
    efd_min, efd_max = -6.4, 7.0
    y = 0.0
    u_prev = 0.0
    started = False

    def avr_ctrl(t: float, state):
        nonlocal y, u_prev, started
        u = state["v_error"]
        if not started:
            y = min(max(ka * u, efd_min), efd_max)
            started = True
        else:
            y_raw = ((1.0 - a) * y + a * ka * (u_prev + u)) / (1.0 + a)
            y = min(max(y_raw, efd_min), efd_max)
        u_prev = u
        return {"efd": y}

    res = openkeito.run(controlled, controller=avr_ctrl)
    assert np.allclose(res["V(b1.a)"], ref["V(b1.a)"], rtol=0.0, atol=1e-11)
    assert np.allclose(res["I(G1.a)"], ref["I(G1.a)"], rtol=0.0, atol=1e-11)
    assert np.allclose(res["S(vt)"], ref["S(vt)"], rtol=0.0, atol=1e-11)
    assert np.allclose(res["S(speed)"], ref["S(speed)"], rtol=0.0, atol=1e-11)
    assert np.allclose(res["S(efd)"], ref["S(efd)"], rtol=0.0, atol=1e-11)


def test_periodic_zoh_call_counts_and_partial_updates():
    model = {
        "format": "openkeito-model",
        "version": "0.5",
        "simulation": {"time_step_s": 1e-4, "end_time_s": 3.5e-3},
        "components": [
            {"type": "controlled_voltage_source", "name": "E1", "nodes": ["n1", "gnd"], "signal": "u1"},
            {"type": "resistor", "name": "R1", "nodes": ["n1", "gnd"], "resistance_ohm": 10.0},
            {"type": "controlled_voltage_source", "name": "E2", "nodes": ["n2", "gnd"], "signal": "u2"},
            {"type": "resistor", "name": "R2", "nodes": ["n2", "gnd"], "resistance_ohm": 10.0},
        ],
        "controls": [
            {"type": "constant", "name": "u1", "value": 0.0},
            {"type": "constant", "name": "u2", "value": 0.0},
        ],
        "outputs": [
            {"type": "node_voltage", "node": "n1"},
            {"type": "node_voltage", "node": "n2"},
            {"type": "signal", "signal": "u1"},
            {"type": "signal", "signal": "u2"},
        ],
    }

    calls_steps: list[float] = []

    def ctrl_steps(t: float, state):
        k = len(calls_steps) + 1
        calls_steps.append(t)
        if k == 1:
            return MappingProxyType({"u1": 1.0, "u2": 5.0})
        if k == 2:
            return None  # Hold both u1 and u2
        if k == 3:
            return {"u1": 3.0}  # Update u1 only; hold u2=5.0
        return {}  # Hold u1=3.0 and u2=5.0

    res1 = openkeito.run(model, controller=ctrl_steps, control_interval_steps=10)
    steps = 35
    assert len(calls_steps) == 1 + steps // 10 == 4

    calls_period: list[float] = []

    def ctrl_period(t: float, state):
        k = len(calls_period) + 1
        calls_period.append(t)
        if k == 1:
            return {"u1": 1.0, "u2": 5.0}
        if k == 2:
            return None
        if k == 3:
            return {"u1": 3.0}
        return {}

    res2 = openkeito.run(model, controller=ctrl_period, control_period_s=1e-3)
    assert len(calls_period) == 4
    assert np.array_equal(res1["S(u1)"], res2["S(u1)"])
    assert np.array_equal(res1["S(u2)"], res2["S(u2)"])

    expected_u1 = np.array([1.0 if i < 20 else 3.0 for i in range(36)])
    expected_u2 = np.full(36, 5.0)
    assert np.array_equal(res1["S(u1)"], expected_u1)
    assert np.array_equal(res1["S(u2)"], expected_u2)


def test_controller_argument_validation():
    _, model = _rlc_p_models(end_time_s=0.001)

    with pytest.raises(ValueError):
        openkeito.run(model, control_interval_steps=2)
    with pytest.raises(ValueError):
        openkeito.run(model, control_period_s=1e-4)
    with pytest.raises(ValueError):
        openkeito.run(model, controller=lambda t, s: None, control_period_s=1e-4, control_interval_steps=1)
    with pytest.raises(ValueError):
        openkeito.run(model, controller="not_callable")  # type: ignore[arg-type]

    for bad_steps in (0, -1, 1.5, True, False):
        with pytest.raises(ValueError):
            openkeito.run(model, controller=lambda t, s: None, control_interval_steps=bad_steps)  # type: ignore[arg-type]

    for bad_period in (0.0, -1e-4, float("nan"), float("inf"), True):
        with pytest.raises(ValueError):
            openkeito.run(model, controller=lambda t, s: None, control_period_s=bad_period)  # type: ignore[arg-type]

    with pytest.raises(ValueError) as exc_info:
        openkeito.run(model, controller=lambda t, s: None, control_period_s=1.5e-4)
    msg = str(exc_info.value)
    assert "control_period_s" in msg and "time_step_s" in msg

    invalid_model = copy.deepcopy(model)
    invalid_model["simulation"]["time_step_s"] = -1.0
    with pytest.raises(openkeito.OpenKeitoError) as mod_err:
        openkeito.run(invalid_model, controller=lambda t, s: None, control_period_s=1e-4)
    assert mod_err.value.diagnostics[0]["code"] == "E110"


def test_controller_runtime_errors_produce_e252_and_keep_partial_rows(tmp_path):
    _, model = _rlc_p_models(end_time_s=0.03)  # 300 steps (> OUTPUT_BATCH = 256)

    # 1. Python exception after 280 updates -> check=True raises, check=False keeps 280 rows.
    calls = 0

    def fail_at_280(t: float, state):
        nonlocal calls
        if calls == 280:
            raise RuntimeError("controller boom")
        calls += 1
        return {"u": 1.0}

    with pytest.raises(openkeito.OpenKeitoError) as exc:
        openkeito.run(model, controller=fail_at_280)
    assert exc.value.diagnostics[0]["code"] == "E252"
    assert exc.value.diagnostics[0]["path"] == "controller"
    assert "controller boom" in exc.value.diagnostics[0]["message"]

    calls = 0
    out_dir = tmp_path / "partial"
    failed = openkeito.run(model, out_dir, controller=fail_at_280, check=False)
    assert failed.status == "failed" and failed.code == 1
    assert failed.summary["errors"][0]["code"] == "E252"
    assert len(failed.time) == 280
    assert failed.summary["result"]["rows"] == 280
    csv_lines = (out_dir / "result.csv").read_text(encoding="utf-8").splitlines()
    assert len(csv_lines) == 281  # header + 280 rows

    # 2. Unknown signal name -> E252
    with pytest.raises(openkeito.OpenKeitoError) as exc2:
        openkeito.run(model, controller=lambda t, s: {"no_such_signal": 1.0})
    assert exc2.value.diagnostics[0]["code"] == "E252"
    assert "no_such_signal" in exc2.value.diagnostics[0]["message"]

    # 3. NaN / inf return -> E252
    for bad_val in (float("nan"), float("inf"), float("-inf")):
        with pytest.raises(openkeito.OpenKeitoError) as exc3:
            openkeito.run(model, controller=lambda t, s, v=bad_val: {"u": v})
        assert exc3.value.diagnostics[0]["code"] == "E252"

    # 4. Non-mapping return -> E252
    with pytest.raises(openkeito.OpenKeitoError) as exc4:
        openkeito.run(model, controller=lambda t, s: [1.0])  # type: ignore[return-value]
    assert exc4.value.diagnostics[0]["code"] == "E252"

    # 5. Failure at t = 0.0 -> 0 rows
    def fail_immediately(t: float, state):
        raise ValueError("fail at t=0")

    failed_t0 = openkeito.run(model, controller=fail_immediately, check=False)
    assert failed_t0.status == "failed"
    assert failed_t0.summary["errors"][0]["code"] == "E252"
    assert len(failed_t0.time) == 0


def test_controller_with_empty_controls_and_threads():
    model = json.loads((EXAMPLES / "line_areas.json").read_text(encoding="utf-8"))
    model["simulation"]["end_time_s"] = 0.002
    ref = openkeito.run(model, threads=2)

    seen_times: list[float] = []

    def observer(t: float, state):
        seen_times.append(t)
        assert "V(a1.a)" in state
        return None

    res = openkeito.run(model, threads=2, controller=observer, control_interval_steps=10)
    steps = round(model["simulation"]["end_time_s"] / model["simulation"]["time_step_s"])
    assert len(seen_times) == 1 + steps // 10
    assert math.isclose(seen_times[0], 0.0)
    for col in ref.columns:
        assert np.array_equal(res[col], ref[col])


def test_controller_overhead_measurement(capsys):
    builtin, controlled = _rlc_p_models(end_time_s=0.2)  # 2000 steps

    t0 = time.perf_counter()
    res_base = openkeito.run(builtin)
    dt_base = time.perf_counter() - t0

    t1 = time.perf_counter()
    res_step1 = openkeito.run(
        controlled,
        controller=lambda t, s: {"u": 3.5 * (10.0 - s["v_c"])},
        control_interval_steps=1,
    )
    dt_step1 = time.perf_counter() - t1

    t2 = time.perf_counter()
    res_step10 = openkeito.run(
        controlled,
        controller=lambda t, s: {"u": 3.5 * (10.0 - s["v_c"])},
        control_interval_steps=10,
    )
    dt_step10 = time.perf_counter() - t2

    assert res_base.ok and res_step1.ok and res_step10.ok
    print(
        f"ai_ctrl_timing steps=2000 base={dt_base*1e3:.2f}ms "
        f"interval1={dt_step1*1e3:.2f}ms interval10={dt_step10*1e3:.2f}ms"
    )
    captured = capsys.readouterr()
    assert "ai_ctrl_timing" in captured.out


def test_controller_base_exceptions_and_interrupt_main_propagate():
    import _thread
    import threading

    _, model = _rlc_p_models(end_time_s=0.05)  # 500 steps

    class CustomBase(BaseException):
        pass

    for check in (True, False):
        calls = 0

        def raise_ki(t: float, state):
            nonlocal calls
            calls += 1
            if calls == 5:
                raise KeyboardInterrupt("user interrupt")
            return {"u": 1.0}

        with pytest.raises(KeyboardInterrupt):
            openkeito.run(model, controller=raise_ki, check=check)
        assert calls == 5

        calls = 0

        def raise_exit(t: float, state):
            nonlocal calls
            calls += 1
            if calls == 3:
                raise SystemExit(7)
            return {"u": 1.0}

        with pytest.raises(SystemExit) as sys_exc:
            openkeito.run(model, controller=raise_exit, check=check)
        assert sys_exc.value.code == 7
        assert calls == 3

        calls = 0

        def raise_custom(t: float, state):
            nonlocal calls
            calls += 1
            if calls == 2:
                raise CustomBase("fatal")
            return {"u": 1.0}

        with pytest.raises(CustomBase):
            openkeito.run(model, controller=raise_custom, check=check)
        assert calls == 2

    # Simulated Ctrl+C via _thread.interrupt_main() both inside callback and from another thread.
    for check in (True, False):
        calls = 0

        def interrupt_on_step_10(t: float, state):
            nonlocal calls
            calls += 1
            if calls == 10:
                _thread.interrupt_main()
            return {"u": 1.0}

        with pytest.raises(KeyboardInterrupt):
            openkeito.run(model, controller=interrupt_on_step_10, check=check)
        assert calls == 10

        calls = 0
        ready = threading.Event()
        sent = threading.Event()

        def trigger_ctrl_c():
            ready.wait(timeout=5.0)
            _thread.interrupt_main()
            sent.set()

        worker = threading.Thread(target=trigger_ctrl_c, daemon=True)
        worker.start()

        def sync_and_wait(t: float, state):
            nonlocal calls
            calls += 1
            if calls == 4:
                ready.set()
                sent.wait(timeout=5.0)
            return {"u": 1.0}

        try:
            with pytest.raises(KeyboardInterrupt):
                openkeito.run(model, controller=sync_and_wait, check=check)
        finally:
            ready.set()
            worker.join(timeout=5.0)
        assert calls == 4
