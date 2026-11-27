"""Transitions: runs that follow a schedule from one time to the next.

The responses to a step of a set point are compared with the solutions of
the transfer functions of the controls; the start of a transition with the
power flow of its first time and its end with the power flow of its second
time. Each tolerance is derived where it is used.
"""

from __future__ import annotations

import json
import math

import numpy as np
import pytest

import openkeito
from openkeito.convert import Conversion, ConversionError, synchronous_machine
from openkeito.schedule import Schedule
from openkeito.transition import GOVERNOR, add_machine_controls, build, build_sequence, machine_inputs, measure

pp = pytest.importorskip("pandapower")
pn = pytest.importorskip("pandapower.networks")

from openkeito.pandapower import (  # noqa: E402
    from_pandapower, run_transitions, schedule_from_opf, transition)

TIMES = ["2026-11-21T00:00:00+09:00", "2026-11-21T01:00:00+09:00", "2026-11-21T02:00:00+09:00"]
#: A governor that is fast against the swings of the machines, for the tests
#: of the end of a transition: with the typical time constants the swings
#: take tens of seconds to decay (see test_typical_governor_...).
FAST = {"governor_time_constant_s": 0.02, "turbine_time_constant_s": 0.03}
#: Before the first change the bus voltage magnitudes, taken from the three
#: phase voltages of one instant, stay at their first value within the error
#: of the time step, which is below (w h)^2 = 2.5e-4 at 50 Hz and 50 us (the
#: bound that the time slices use; measured 9e-6 to 7e-5).
DRIFT = 2.5e-4


def changes(tr) -> dict:
    """{kind of change: number} of a transition."""
    return {what: int(n) for what, n in (line.rsplit(": ", 1) for line in tr.changes)}


# The set points of a machine ------------------------------------------------------


def island(time_step_s: float, end_time_s: float, controls: dict | None = None) -> tuple[dict, dict]:
    """A 555 MVA machine alone on a load of 400 MW and 100 Mvar, with the
    typical excitation system and governor."""
    machine, _ = synchronous_machine("G1", "b1", frequency_hz=60.0, rated_power_mva=555.0, rated_voltage_kv=24.0,
                                     s_mva=complex(400.0, 100.0), voltage_pu=1.0, angle_deg=0.0)
    model = {
        "format": "openkeito-model", "version": "0.5", "name": "island",
        "simulation": {"time_step_s": time_step_s, "end_time_s": end_time_s, "initialization": "steady_state",
                       "output_interval_steps": 10},
        "components": [machine, {"type": "three_phase_load", "name": "LOAD", "bus": "b1", "rated_voltage_kv": 24.0,
                                 "rated_frequency_hz": 60.0, "active_power_mw": 400.0, "reactive_power_mvar": 100.0}],
        "controls": [
            {"type": "measurement", "name": "p", "quantity": {"type": "component_signal", "component": "G1",
                                                              "signal": "active_power_pu"}},
            {"type": "measurement", "name": "efd0", "quantity": {"type": "component_signal", "component": "G1",
                                                                 "signal": "initial_field_voltage_pu"}},
            {"type": "measurement", "name": "tm0", "quantity": {"type": "component_signal", "component": "G1",
                                                                "signal": "initial_mechanical_torque_pu"}}],
        "outputs": [{"type": "bus_voltage", "bus": "b1"}] + [{"type": "signal", "signal": s}
                                                             for s in ("p", "efd0", "tm0")],
    }
    conv = Conversion(model=model, mapping={"elements": [], "buses": {}})
    add_machine_controls(conv, **(controls or {}))
    model["outputs"] += [{"type": "signal", "signal": s} for s in ("G1_speed", "G1_vt")]
    return model, machine


def test_the_format_version_is_raised_as_a_number_and_never_lowered():
    from openkeito.convert import version_rank
    from openkeito.transition import _format_version

    assert version_rank("0.10") > version_rank("0.9") > version_rank("0.5")
    assert version_rank("x") < version_rank("0.4")
    load = {"type": "resistor", "name": "R1", "nodes": ["a", "gnd"], "resistance_ohm": 1.0}
    tap = {**load, "tap_changes": [{"time_s": 0.1, "ratio": 1.0}]}
    gain = {"type": "gain", "name": "k", "input": "x", "gain": 1.0}
    # "0.10" < "0.5" as text: a model of version 0.10 with controls kept
    # its version only when compared as numbers.
    assert _format_version({"version": "0.10", "components": [load], "controls": [gain]}) == "0.10"
    assert _format_version({"version": "0.10", "components": [tap], "controls": []}) == "0.10"
    assert _format_version({"version": "0.4", "components": [load], "controls": [gain]}) == "0.5"
    assert _format_version({"version": "0.5", "components": [tap], "controls": [gain]}) == "0.6"
    assert _format_version({"version": "0.6", "components": [load], "controls": [gain]}) == "0.6"
    assert _format_version({"components": [load], "controls": []}) == "0.4"


def test_machine_inputs_are_those_of_the_machine():
    """The field voltage and the torque that ``machine_inputs`` computes for
    an operating point are those the machine itself starts with. They agree
    to rounding: both are the steady state of the same equations."""
    model, machine = island(1e-4, 1e-3)
    result = openkeito.run(model)
    efd, tm = machine_inputs(machine)
    assert result["S(efd0)"][0] == pytest.approx(efd, rel=1e-9)
    assert result["S(tm0)"][0] == pytest.approx(tm, rel=1e-9)
    # Without load at the rated voltage: Efd = 1, no torque.
    assert machine_inputs(machine, 0j, 1.0) == pytest.approx((1.0, 0.0))


def governor_step(t: np.ndarray, dp: float, h_s: float, g: dict) -> np.ndarray:
    """Speed deviation of a machine alone on a load of constant power after
    a step dp of the load reference: 2 H s dw = Pm, Pm = (dp - dw / R) /
    ((1 + s TG)(1 + s TCH)), so dw = dp / (s D(s)) with D = 2 H TG TCH s^3 +
    2 H (TG + TCH) s^2 + 2 H s + 1 / R, by partial fractions."""
    tg, tch, r = g["governor_time_constant_s"], g["turbine_time_constant_s"], g["droop_pu"]
    d = np.poly1d([2 * h_s * tg * tch, 2 * h_s * (tg + tch), 2 * h_s, 1 / r])
    y = np.full(t.shape, r, dtype=complex)
    for pole in d.roots:
        y += np.exp(pole * t) / (pole * d.deriv()(pole))
    return dp * y.real


@pytest.mark.parametrize("time_step_s", [1e-4, 5e-5])
def test_load_reference_step_of_an_islanded_machine_against_the_transfer_function(time_step_s):
    dp, t0 = 0.05, 0.5
    model, machine = island(time_step_s, 12.0)
    block = next(b for b in model["controls"] if b["name"] == "G1_dpref")
    block.clear()
    block.update({"type": "step", "name": "G1_dpref", "time_s": t0, "before": 0.0, "after": dp})
    result = openkeito.run(model)
    t, dw = result.time, result["S(G1_speed)"] - 1.0
    expected = np.where(t >= t0, governor_step(np.maximum(t - t0, 0.0), dp, machine["inertia_h_s"], GOVERNOR), 0.0)
    peak = float(np.max(np.abs(expected)))
    assert peak == pytest.approx(3.325e-3, rel=1e-3)
    # No drift before the step: the references are the values of t = 0.
    assert np.max(np.abs(dw[t < t0])) < 1e-5
    # The transfer function takes the electrical power for constant. It is
    # not quite: the load is an impedance, and the voltage moves while the
    # regulator follows (by 2e-4 pu, so the power by 2.6e-4 pu). That power
    # acts on the speed as a change of the reference of the same size would:
    # the error is at most (largest power deviation / dp) of the response,
    # here 0.5 %; the measured one is 0.6 % of the peak (1.8e-5), and it does
    # not change with the time step (5e-5: 2.0e-5), as a model error.
    dpe = float(np.max(np.abs(result["S(p)"] - result["S(p)"][0])))
    assert dpe < 0.01 * dp
    assert np.max(np.abs(dw - expected)) < 2.0 * dpe / dp * peak
    # The droop leaves R dp (0.25 %): within 1e-3 of it after 11.5 s (the
    # slowest pole, -0.45 1/s, has decayed to 6e-3 of the response of 1e-3).
    assert dw[-1] == pytest.approx(GOVERNOR["droop_pu"] * dp, rel=2e-3)


def test_typical_governor_weakens_the_damping_of_the_swings():
    """Why the tests of the end of a transition use a fast governor: at the
    frequency of the swings against the grid (0.8 Hz in case9) the typical
    governor and turbine lag the speed by about 90 degrees, so their torque
    takes away most of the damping that the rotor windings give. Measured
    after a load step in case9, without a stabilizer: the swing shrinks by
    0.70 every 2 s without a governor (time constant 5.6 s), by 0.90 with
    the typical one (19 s), and by 0.05 with the fast one of these tests. It
    stays stable. The stabilizer that a transition gives the machines unless
    told otherwise (``PSS``) brings the damping back: with it and the typical
    governor the swing shrinks to 0.07 over 4 s (test_pss.py has the
    comparison with the eigenvalues)."""
    net = pn.case9()
    plan = Schedule.from_values(TIMES[:2], {("load/0", "active_power_mw"): [80.0, 90.0]})

    def remaining(governor, pss=False):
        tr = transition(net, plan, start_time_s=0.2, ramp_time_s=0.0, settle_time_s=8.0, governor=governor,
                        pss=pss, time_step_s=1e-4)
        r = openkeito.run(tr.model)
        w = r["S(gen0_speed)"]

        def swing(a, b):
            part = w[(r.time >= a) & (r.time < b)]
            return float(part.max() - part.min())

        return swing(6.2, 8.2) / swing(2.2, 4.2)

    with_governor, without = remaining(True), remaining(False)
    # Over 4 s: 0.50 without, 0.83 with the typical governor.
    assert without < 0.6 and 0.7 < with_governor < 0.95
    # With the stabilizer: 0.07 (the swing mode decays in a second; what is
    # left is the slower common movement of the governors).
    assert remaining(True, pss=True) < 0.2


# The references of a converter ----------------------------------------------------


def strong_grid():
    """A 2 MVA inverter at 6.6 kV behind a short line from a grid of
    20 000 MVA: its voltage does not move with its power."""
    net = pp.create_empty_network(f_hz=50.0, sn_mva=10.0)
    b0, b1 = pp.create_bus(net, 6.6), pp.create_bus(net, 6.6)
    pp.create_ext_grid(net, b0, s_sc_max_mva=20000.0, rx_max=0.1)
    pp.create_line_from_parameters(net, b0, b1, 1.0, r_ohm_per_km=0.002, x_ohm_per_km=0.02, c_nf_per_km=0.0,
                                   max_i_ka=10.0)
    pp.create_sgen(net, b1, p_mw=1.0, q_mvar=0.0, sn_mva=2.0)
    return net


@pytest.mark.parametrize("ramp_time_s", [0.0, 0.02])
def test_converter_power_reference_against_the_current_loop(ramp_time_s):
    """The active power of a converter follows its reference as the current
    loop 1 / (tau s + 1) does (tau = 1 ms), since i_d = P / v and the voltage
    of a strong grid stays."""
    tau, t0, p0, dp = 1e-3, 0.05, 1.0, 0.6
    plan = Schedule.from_values(TIMES[:2], {("sgen/0", "active_power_mw"): [p0, p0 + dp]})

    def expected(t):
        x = np.maximum(t - t0, 0.0)
        if ramp_time_s == 0:
            return p0 + dp * (1 - np.exp(-x / tau))
        # The response to a ramp of slope k, and the same from its end on.
        k, x2 = dp / ramp_time_s, np.maximum(x - ramp_time_s, 0.0)
        return p0 + k * (x - tau * (1 - np.exp(-x / tau))) - k * (x2 - tau * (1 - np.exp(-x2 / tau)))

    errors = []
    for h in (5e-5, 2.5e-5):
        tr = transition(strong_grid(), plan, start_time_s=t0, ramp_time_s=ramp_time_s, settle_time_s=0.05,
                        time_step_s=h, output_interval_steps=1, sources="hold")
        assert changes(tr) == {"converters whose references move": 1}
        result = openkeito.run(tr.model)
        p = result["S(sgen0.p)"] * 2.0
        errors.append(float(np.max(np.abs(p - expected(result.time)))))
        # The start is the power flow, and the end the new reference: within
        # 5e-5 pu of the rating, the accuracy of the converter's steady state
        # at 50 us (README, grid-following converter).
        assert abs(p[0] - p0) < 1e-4 and abs(p[-1] - (p0 + dp)) < 1e-4
    if ramp_time_s == 0:
        # A step: the dead time of one step in the current loop keeps the
        # response within 0.74 h / tau of the step from the first-order lag
        # (README). Measured: 2.2 % of the step at 50 us, 1.1 % at 25 us.
        assert errors[0] < 0.74 * 5e-5 / tau * dp and errors[1] < 0.74 * 2.5e-5 / tau * dp
    else:
        # A ramp: the reference reaches the circuit within a step, so the
        # error is below what the ramp moves in one step, k h (1.5e-3 MW at
        # 50 us; measured 5.9e-4), plus the accuracy of the steady state.
        k = dp / ramp_time_s
        assert errors[0] < k * 5e-5 + 1e-4 and errors[1] < k * 2.5e-5 + 1e-4
    # First order in the step.
    assert errors[1] < 0.6 * errors[0]


# From a schedule ------------------------------------------------------------------


def opf_plan(net, factors=(0.8, 0.9, 1.0), sgen_factors=None):
    """Loads scaled by ``factors`` at the hours of TIMES, static generators
    by ``sgen_factors``, and the generator set points of the AC OPF."""
    values = {}
    for i, row in net.load.iterrows():
        values[(f"load/{i}", "active_power_mw")] = [row.p_mw * f for f in factors]
        values[(f"load/{i}", "reactive_power_mvar")] = [row.q_mvar * f for f in factors]
    for i, row in net.sgen.iterrows():
        values[(f"sgen/{i}", "active_power_mw")] = [row.p_mw * f for f in sgen_factors]
    plan, _ = schedule_from_opf(net, Schedule.from_values(TIMES[:len(factors)], values, name="loads"))
    return plan


def test_case9_transitions_start_and_end_at_the_power_flows():
    net = pn.case9()
    plan = opf_plan(net)
    run = run_transitions(net, plan, start_time_s=0.5, ramp_time_s=2.0, settle_time_s=10.0, governor=FAST)
    assert [r["case"] for r in run.rows] == ["i000", "i001"]
    for row, tr in zip(run.rows, run.transitions):
        assert row["status"] == "completed"
        assert changes(tr) == {"machines whose set points move": 2, "loads and shunts that change": 3,
                               "sources whose internal voltage moves": 1}
        # The start is the power flow of from_time, as for a time slice.
        assert row["max_abs_dv_pu"] < 1e-9 and row["max_abs_dva_degree"] < 1e-8
        # Before the change nothing moves (DRIFT; measured 9e-6).
        assert row["drift_before_change_pu"] < DRIFT
        # The end is the power flow of to_time: every set point has moved to
        # what that power flow asks for, the loads are its impedances and the
        # source has its internal voltage. With a fast governor and PSS decay
        # ~ -1.9 1/s over 10 s (exp(-19) ~ 5e-9), what is left is only the
        # step discretization error of order (w h)^2 = 3.55e-4 at 60 Hz, 50 us
        # (measured: 9e-6 pu, 4e-4 degrees, 2e-3 MW of 122 MW).
        assert row["end_max_abs_dv_pu"] < 5e-5 and row["end_max_abs_dva_degree"] < 5e-3
        assert row["end_max_abs_dp_mw"] < 0.02
        assert row["end_frequency_hz"] == pytest.approx(60.0, abs=1e-6)
        assert row["end_voltage_swing_pu"] < 1e-6 and row["end_frequency_swing_hz"] < 1e-6
        assert 0.95 < row["voltage_min_pu"] < row["voltage_max_pu"] < 1.1
        assert 59.9 < row["frequency_min_hz"] < 60.0 < row["frequency_max_hz"] < 60.1
        assert row["max_machine_current_pu"] < 1.0
    # The slack delivers what the power flow of to_time gives it.
    end = run.transitions[0].end
    result = openkeito.run(end.model | {"outputs": end.model["outputs"] + [
        {"type": "branch_current", "component": "ext_grid0"}]})
    assert run.rows[0]["end_source_power_mw"] == pytest.approx(end.source_powers(result)["ext_grid0"].real, abs=0.02)


def test_held_sources_leave_the_end_off_the_power_flow():
    """With sources="hold" the external grid keeps its internal voltage, which
    the power flow of to_time has 0.11 % higher: the bus voltages end that
    far from it (measured 5e-4 pu), and a warning says so."""
    net = pn.case9()
    plan = opf_plan(net, (0.8, 0.9))
    tr = transition(net, plan, start_time_s=0.5, ramp_time_s=2.0, settle_time_s=10.0, governor=FAST, sources="hold")
    assert any("sources keep the internal voltage" in w and "ext_grid/0" in w for w in tr.warnings)
    row = measure(tr, openkeito.run(tr.model))
    assert 2e-4 < row["end_max_abs_dv_pu"] < 1.2e-3


def with_inverters(net):
    """Static generators at the three largest loads."""
    for i, row in net.load.sort_values("p_mw", ascending=False).head(3).iterrows():
        pp.create_sgen(net, int(row.bus), p_mw=0.2 * row.p_mw, q_mvar=0.0, sn_mva=0.3 * row.p_mw)
    return net


def test_case39_with_machines_and_converters():
    """With what a transition gives the machines unless told otherwise: the
    typical excitation system, governor and stabilizer."""
    net = with_inverters(pn.case39())
    plan = opf_plan(net, (0.9, 1.0), sgen_factors=(1.0, 0.5))
    run = run_transitions(net, plan, start_time_s=0.2, ramp_time_s=1.0, settle_time_s=10.0)
    (row,), (tr,) = run.rows, run.transitions
    assert row["status"] == "completed"
    assert changes(tr) == {"machines whose set points move": 9, "loads and shunts that change": 21,
                           "converters whose references move": 3, "sources whose internal voltage moves": 1}
    assert row["max_abs_dv_pu"] < 1e-8 and row["max_abs_dva_degree"] < 1e-7
    assert row["drift_before_change_pu"] < DRIFT
    # The end is the power flow of to_time within the time-step error
    # (order (w h)^2 = 3.55e-4 at 60 Hz, 50 us) plus the residual PSS-damped
    # inter-area swing after 10 s (~ exp(-1.9 * 10) ~ 5e-9 for local modes,
    # slower common-mode governor settling). Measured: 3.3e-6 pu, 2.9e-3 deg
    # and 0.017 MW (of 650 MW), with voltages still moving by 2e-5 pu; 20 s
    # later 4.7e-6 pu, 1.8e-3 deg and 0.014 MW.
    assert row["end_max_abs_dv_pu"] < 1e-4 and row["end_max_abs_dva_degree"] < 0.01
    assert row["end_max_abs_dp_mw"] < 0.1 and row["end_voltage_swing_pu"] < 5e-4
    assert row["end_frequency_hz"] == pytest.approx(60.0, abs=1e-3)
    assert row["max_converter_current_pu"] < 1.1 and row["max_machine_current_pu"] < 1.1
    # Half way there (5 s after the change) it is 3.0e-4 pu, 0.09 degrees
    # and 0.18 MW away.
    half = measure(tr, run.batch[tr.name], end_time_s=tr.changes_end_s + 5.0)
    assert row["end_max_abs_dp_mw"] < half["end_max_abs_dp_mw"] < 2.0
    with pytest.raises(ValueError, match="end_time_s"):
        measure(tr, run.batch[tr.name], end_time_s=tr.end_time_s + 1.0)

    # Without the stabilizer the swings between the machines last: 10 s after
    # the change the run is 4e-3 pu, 3 degrees and 20 MW from the power flow
    # (60 s after it: 6e-4 pu, 0.1 degrees and 3 MW).
    plain = transition(net, plan, start_time_s=0.2, ramp_time_s=1.0, settle_time_s=10.0, pss=False)
    row = measure(plain, openkeito.run(plain.model))
    assert row["status"] == "completed" and row["max_abs_dv_pu"] < 1e-8
    assert row["end_max_abs_dp_mw"] > 5.0 and row["end_max_abs_dva_degree"] > 0.5


def test_the_stabilizer_lets_case9_settle_with_the_typical_governor():
    """How far the run is from the power flow of to_time 10 s and 30 s after
    the change, with the typical governor, without and with the stabilizer
    (the first hour of the plan of the test above)."""
    net = pn.case9()
    plan = opf_plan(net)
    table = {}
    for pss in (False, True):
        tr = transition(net, plan, start_time_s=0.5, ramp_time_s=2.0, settle_time_s=30.0, pss=pss)
        result = openkeito.run(tr.model)
        rows = [measure(tr, result, end_time_s=tr.changes_end_s + after) for after in (10.0, 30.0)]
        # The stabilizer is at rest at the start: the same power flow.
        assert rows[0]["max_abs_dv_pu"] < 1e-9 and rows[0]["drift_before_change_pu"] < DRIFT
        table[pss] = [(r["end_max_abs_dv_pu"], r["end_max_abs_dva_degree"], r["end_max_abs_dp_mw"],
                       r["end_frequency_swing_hz"]) for r in rows]
        print("pss", pss, [tuple(f"{x:.3g}" for x in line) for line in table[pss]])
    # Measured (pu, degrees, MW, Hz of swing within the last 0.1 s):
    #   without, 10 s: 7.8e-5, 0.12, 0.52, 1.3e-3      30 s: 5.1e-6, 0.015, 0.066, 1.7e-4
    #   with,    10 s: 8.0e-6, 1.1e-4, 3.8e-4, 4.9e-6   30 s: 8.6e-6, 4.0e-4, 2.0e-3, 2e-11
    # With the stabilizer the run is, from 10 s on, as close as the time step
    # lets it be: the bounds are those of the test with the fast governor
    # above, whose end (8.6e-6 pu, 4.0e-4 degrees, 2.0e-3 MW) it reaches.
    for dv, dva, dp, _ in table[True]:
        assert dv < 5e-5 and dva < 5e-3 and dp < 0.02
    assert table[True][1][3] < 1e-6
    # Without it the swings are still there: 250 times the power and the
    # frequency swing at 10 s, 30 times the power at 30 s.
    assert table[False][0][2] > 0.2 and table[False][0][1] > 0.05 and table[False][0][3] > 5e-4
    assert table[False][1][2] > 0.03 > table[True][1][2]


def test_parallel_and_sequential_runs_agree_bit_for_bit():
    net = with_inverters(pn.case9())
    plan = opf_plan(net, sgen_factors=(1.0, 0.6, 0.8))
    options = {"start_time_s": 0.1, "ramp_time_s": 0.2, "settle_time_s": 0.3, "output_interval_steps": 1}
    one = run_transitions(net, plan, jobs=1, **options)
    many = run_transitions(net, plan, jobs=4, **options)
    assert len(one.rows) == 2 and one.rows == many.rows
    for name, a in one.batch.cases.items():
        b = many.batch[name]
        assert a.ok and a.columns == b.columns and np.array_equal(a.time, b.time)
        assert all(np.array_equal(a[c], b[c]) for c in a.columns)


# Switching, ramps and what cannot be followed -------------------------------------


def ring():
    """Four 110 kV buses in a ring with an external grid, two generators, a
    static generator, loads, a shunt and a transformer with a tap changer."""
    net = pp.create_empty_network(f_hz=50.0, sn_mva=100.0)
    b = [pp.create_bus(net, 110.0) for _ in range(4)]
    low = pp.create_bus(net, 20.0)
    pp.create_ext_grid(net, b[0], vm_pu=1.02, s_sc_max_mva=5000.0, rx_max=0.1)
    for x, y in ((0, 1), (1, 2), (2, 3), (3, 0)):
        pp.create_line_from_parameters(net, b[x], b[y], 30.0, r_ohm_per_km=0.06, x_ohm_per_km=0.4,
                                       c_nf_per_km=9.0, max_i_ka=1.0)
    pp.create_transformer_from_parameters(net, b[2], low, sn_mva=63.0, vn_hv_kv=110.0, vn_lv_kv=20.0, vk_percent=12.0,
                                          vkr_percent=0.3, pfe_kw=30.0, i0_percent=0.05, vector_group="YNyn",
                                          tap_side="hv", tap_neutral=0, tap_min=-9, tap_max=9, tap_step_percent=1.5,
                                          tap_pos=0, tap_changer_type="Ratio")
    pp.create_gen(net, b[1], p_mw=60.0, vm_pu=1.02, sn_mva=100.0)
    pp.create_gen(net, b[3], p_mw=40.0, vm_pu=1.02, sn_mva=80.0)
    pp.create_load(net, b[2], p_mw=70.0, q_mvar=20.0)
    pp.create_load(net, low, p_mw=30.0, q_mvar=8.0)
    pp.create_load(net, b[3], p_mw=25.0, q_mvar=5.0)
    pp.create_shunt(net, b[2], q_mvar=-10.0, p_mw=0.0)
    pp.create_sgen(net, low, p_mw=10.0, q_mvar=0.0, sn_mva=15.0)
    return net


def test_breakers_follow_in_service_and_the_end_is_the_power_flow():
    """A line and a generator go out, a load comes in and the shunt is
    switched off; the other generator and the grid take over."""
    net = ring()
    plan = Schedule.from_values(TIMES[:2], {
        ("line/1", "in_service"): [1, 0], ("gen/1", "in_service"): [1, 0], ("gen/0", "active_power_mw"): [60.0, 85.0],
        ("load/2", "in_service"): [0, 1], ("shunt/0", "in_service"): [1, 0],
        ("sgen/0", "active_power_mw"): [10.0, 4.0], ("sgen/0", "reactive_power_mvar"): [0.0, 2.0]})
    tr = transition(net, plan, start_time_s=0.2, ramp_time_s=1.0, settle_time_s=10.0, governor=FAST)
    # Every load changes a little: it takes its scheduled power at the
    # voltage of each power flow, and the voltages differ.
    assert changes(tr) == {
        "sources whose internal voltage moves": 1, "machines whose set points move": 1,
        "machines that stop (breaker opens)": 1, "loads and shunts that change": 4,
        "converters whose references move": 1, "branches switched out": 1}
    assert openkeito.validate(tr.model)["valid"]
    result = openkeito.run(tr.model)
    row = measure(tr, result)
    assert row["max_abs_dv_pu"] < 1e-9 and row["drift_before_change_pu"] < DRIFT
    # The breakers of the line and of the machine open at the current zeros
    # after 0.2 s; the new load comes in and the shunt goes out in four parts
    # each, as does the small change of the two other loads.
    assert row["events"] == 3 * (2 + 1 + 4 + 4 + 4 + 4)
    # The end: the error of the time step, and what the stopped machine and
    # the open line still take (their breakers leave 1 W leaks).
    assert row["end_max_abs_dv_pu"] < 1e-4 and row["end_max_abs_dva_degree"] < 0.01
    assert row["end_max_abs_dp_mw"] < 0.02 and row["end_max_abs_dp_element"] in ("gen/0", "sgen/0")
    assert row["end_frequency_hz"] == pytest.approx(50.0, abs=1e-4)
    # The stopped machine runs on without load. Its load reference went to
    # zero with the breaker, and the valve closed while the speed rose by
    # 0.3 %; with the valve at its lower limit and neither load nor damping,
    # nothing brings the speed back (with the reference kept, the droop would
    # leave it 2.5 % high).
    speed = result["S(gen1_speed)"]
    assert abs(result["S(gen1_p)"][-1]) < 1e-6 and 1.0 < speed[-1] < 1.005 and speed.max() < 1.005

    # The other way round: the line, the machine and the shunt come in.
    back = Schedule.from_values(TIMES[:2], {
        ("line/1", "in_service"): [0, 1], ("gen/1", "in_service"): [0, 1], ("gen/1", "active_power_mw"): [0.0, 40.0],
        ("shunt/0", "in_service"): [0, 1]})
    tr = transition(net, back, start_time_s=0.2, ramp_time_s=1.0, settle_time_s=10.0, governor=FAST)
    found = changes(tr)
    # The machine that starts and the one that takes less.
    assert found["branches switched in"] == 1 and found["machines whose set points move"] == 2
    assert found["machines that start (connected without load from the beginning)"] == 1
    row = measure(tr, openkeito.run(tr.model))
    assert row["max_abs_dv_pu"] < 1e-9
    assert row["end_max_abs_dv_pu"] < 1e-4 and row["end_max_abs_dp_mw"] < 0.02


def test_stabilizers_go_to_the_machines_that_carry_load():
    """A machine gets a stabilizer when it delivers active power at one of
    the two times (a tenth of its rating or more); one that stays without
    is named in a warning, and pss=False gives none."""
    def stabilizers(values, **options):
        tr = transition(ring(), Schedule.from_values(TIMES[:2], values), settle_time_s=0.1, **options)
        return (sorted(b["name"] for b in tr.model["controls"] if b["name"].endswith("_pss")),
                [w for w in tr.warnings if "stabilizer" in w])

    assert stabilizers({("gen/0", "active_power_mw"): [60.0, 80.0]}) == (["gen0_pss", "gen1_pss"], [])
    assert stabilizers({("gen/0", "active_power_mw"): [60.0, 80.0]}, pss=False) == ([], [])
    # gen/1 (80 MVA) takes load during the run, or gives it up.
    assert stabilizers({("gen/1", "active_power_mw"): [0.0, 40.0]})[0] == ["gen0_pss", "gen1_pss"]
    assert stabilizers({("gen/1", "active_power_mw"): [40.0, 0.0]})[0] == ["gen0_pss", "gen1_pss"]
    # It starts during the run.
    assert stabilizers({("gen/1", "in_service"): [0, 1], ("gen/1", "active_power_mw"): [0.0, 40.0]}) == (
        ["gen0_pss", "gen1_pss"], [])
    # It stays at 5 MW of 80 MVA: none, unless asked for by name.
    idle = {("gen/1", "active_power_mw"): [5.0, 5.0], ("load/0", "active_power_mw"): [70.0, 75.0]}
    names, warnings = stabilizers(idle)
    assert names == ["gen0_pss"] and len(warnings) == 1 and warnings[0].endswith(": gen/1")
    assert stabilizers(idle, machine_controls={"gen/1": {"pss": True}}) == (["gen0_pss", "gen1_pss"], [])
    names, warnings = stabilizers(idle, machine_controls={"gen/0": {"pss": {"gain": 5.0}}})
    assert names == ["gen0_pss"] and len(warnings) == 1


def test_tap_changes_step_to_new_ratio_without_warning():
    net = ring()
    plan = Schedule.from_values(TIMES[:2], {("trafo/0", "tap_position"): [0, 2],
                                            ("load/0", "active_power_mw"): [70.0, 75.0]})
    tr = transition(net, plan, start_time_s=0.1, ramp_time_s=0.0, settle_time_s=0.5)
    assert tr.model["version"] == "0.6"
    assert not any("tap change" in w for w in tr.warnings), tr.warnings
    assert changes(tr)["transformers whose taps move"] == 1
    trafo = next(c for c in tr.model["components"] if c["name"] == "trafo0")
    assert trafo["tap_changes"] == [{
        "time_s": pytest.approx(0.1),
        "rated_voltage_kv": pytest.approx([113.3, 20.0]),
        "impedance_percent": pytest.approx(12.0),
        "resistance_percent": pytest.approx(0.3),
    }]
    result = openkeito.run(tr.model)
    row = measure(tr, result)
    assert row["status"] == "completed" and row["end_max_abs_dv_pu"] < 1e-4 and row["end_max_abs_dp_mw"] < 0.05

    ppl = pytest.importorskip("pypowsybl")
    from openkeito.powsybl import transition as powsybl_transition

    e_net = ppl.network.create_eurostag_tutorial_example1_network()
    e_plan = Schedule.from_values(TIMES[:2], {("NHV2_NLOAD", "tap_position"): [1, 2]})
    e_tr = powsybl_transition(e_net, e_plan, start_time_s=0.1, ramp_time_s=0.0, settle_time_s=0.5, governor=FAST)
    assert e_tr.model["version"] == "0.6"
    assert not any("tap change" in w for w in e_tr.warnings), e_tr.warnings
    assert changes(e_tr)["transformers whose taps move"] == 1
    e_row = measure(e_tr, openkeito.run(e_tr.model))
    assert e_row["status"] == "completed" and e_row["end_max_abs_dv_pu"] < 1e-4, e_row


def test_ramp_times_and_rates():
    net = ring()
    plan = Schedule.from_values(TIMES[:2], {("gen/0", "active_power_mw"): [60.0, 80.0],
                                            ("sgen/0", "active_power_mw"): [10.0, 4.0]}, interpolation="linear")

    def blocks(tr):
        return {b["name"]: b for b in tr.model["controls"]}

    # 20 MW at 120 MW/min take 10 s; the converter has its own time.
    tr = transition(net, plan, start_time_s=1.0, ramp_rate_mw_per_min={"gen/0": 120.0},
                    ramp_time_s={"sgen/0": 0.0, "default": 3.0}, settle_time_s=2.0)
    b = blocks(tr)
    assert b["gen0_dpref"]["times_s"] == pytest.approx([1.0, 11.0]) and b["gen0_dpref"]["values"][0] == 0.0
    # The reference moves by the torque: the power and the stator loss, per
    # unit of the rating (100 MVA).
    assert b["gen0_dpref"]["values"][1] == pytest.approx(0.2, rel=0.01)
    assert b["sgen0_p_ref"] == {"type": "step", "name": "sgen0_p_ref", "time_s": 1.0, "before": 10.0, "after": 4.0}
    assert b["ext_grid0_ed"]["times_s"] == pytest.approx([1.0, 4.0])
    assert tr.changes_end_s == pytest.approx(11.0) and tr.end_time_s == pytest.approx(13.0)
    assert tr.model["simulation"]["end_time_s"] == pytest.approx(13.0)

    # The real time between two times of a linear schedule: 30 s within the hour.
    tr = transition(net, plan, "2026-11-21T00:10:00+09:00", "2026-11-21T00:10:30+09:00", ramp_time_s="schedule",
                    settle_time_s=1.0)
    b = blocks(tr)
    assert b["gen0_dpref"]["times_s"] == pytest.approx([1.0, 31.0])
    assert b["gen0_dpref"]["values"][1] == pytest.approx(0.2 * 30 / 3600, rel=0.01)
    assert tr.end_time_s == pytest.approx(32.0)
    step_plan = Schedule.from_values(TIMES[:2], {("gen/0", "active_power_mw"): [60.0, 80.0]})
    with pytest.raises(ValueError, match="interpolation"):
        transition(net, step_plan, ramp_time_s="schedule")
    with pytest.raises(ValueError, match="unknown option nope"):
        transition(net, step_plan, nope=1)
    with pytest.raises(ValueError, match="not after"):
        transition(net, step_plan, TIMES[1], TIMES[0])


def test_sources_and_current_injections_as_generators():
    """With generators="source" a generator follows as a source whose
    internal voltage moves; an inverter that is a current injection cannot
    follow, which a warning says."""
    net = ring()
    plan = Schedule.from_values(TIMES[:2], {("gen/0", "active_power_mw"): [60.0, 80.0],
                                            ("sgen/0", "active_power_mw"): [10.0, 4.0]})
    tr = transition(net, plan, start_time_s=0.1, ramp_time_s=0.5, settle_time_s=1.0, generators="source",
                    inverter_model="current_source")
    assert changes(tr)["sources whose internal voltage moves"] == 3
    assert any("cannot follow a set point" in w and "sgen/0" in w for w in tr.warnings)
    row = measure(tr, openkeito.run(tr.model))
    assert row["status"] == "completed" and row["max_abs_dv_pu"] < 1e-9
    # All sources are at their end values and the network is linear: the end
    # is the power flow of to_time but for the injection that stayed, 6 MW
    # of 125 (measured 2e-3 pu).
    assert row["end_max_abs_dv_pu"] < 5e-3


def test_an_island_of_machines_returns_to_the_rated_frequency():
    """Without an external grid (the slack is a generator) nothing holds the
    frequency but the governors. Their references move by what the two
    power flows ask for, so the frequency comes back to 50 Hz."""
    net = ring()
    net.ext_grid.in_service = False
    net.gen.at[0, "slack"] = True
    plan = Schedule.from_values(TIMES[:2], {("load/0", "active_power_mw"): [70.0, 80.0],
                                            ("gen/1", "active_power_mw"): [40.0, 45.0]})
    tr = transition(net, plan, start_time_s=0.2, ramp_time_s=1.0, settle_time_s=15.0, governor=FAST)
    assert not any(c["type"].startswith("three_phase_source") for c in tr.model["components"])
    row = measure(tr, openkeito.run(tr.model))
    assert row["max_abs_dv_pu"] < 1e-9 and row["drift_before_change_pu"] < DRIFT
    assert row["frequency_min_hz"] < 49.995
    # The loads are impedances at the voltages of the power flow of to_time,
    # which the regulators restore: the powers balance at 50 Hz but for the
    # error of the time step in them (measured 2e-5 Hz with a droop of 5 %).
    assert row["end_frequency_hz"] == pytest.approx(50.0, abs=1e-3)
    assert row["end_max_abs_dv_pu"] < 1e-4 and row["end_max_abs_dp_mw"] < 0.05


def test_build_refuses_what_it_cannot_follow():
    net = ring()
    a = from_pandapower(net, generators="machine")
    with pytest.raises(ValueError, match="load_steps"):
        build(a, a, load_steps=0)
    with pytest.raises(ValueError, match="sources"):
        build(a, a, sources="ignore")
    same = build(a, a)
    assert same.changes == [] and any("nothing changes" in w for w in same.warnings)
    # A machine cannot start at a bus that is dead at from_time.
    dead = ring()
    extra = pp.create_bus(dead, 110.0)
    pp.create_line_from_parameters(dead, 3, extra, 10.0, r_ohm_per_km=0.06, x_ohm_per_km=0.4, c_nf_per_km=9.0,
                                   max_i_ka=1.0)
    pp.create_gen(dead, extra, p_mw=5.0, vm_pu=1.0, sn_mva=20.0)
    plan = Schedule.from_values(TIMES[:2], {("line/4", "in_service"): [0, 1], ("gen/2", "in_service"): [0, 1]})
    with pytest.raises(ConversionError, match="no voltage"):
        transition(dead, plan)


# PowSyBl --------------------------------------------------------------------------


def test_powsybl_transition():
    ppl = pytest.importorskip("pypowsybl")
    from openkeito.powsybl import run_transitions as run_powsybl

    network = ppl.network.create_ieee14()
    loads = network.get_loads()
    values = {}
    for lid in loads.index[:4]:
        values[(lid, "active_power_mw")] = [float(loads.at[lid, "p0"]), 1.2 * float(loads.at[lid, "p0"])]
    values[("B2-G", "active_power_mw")] = [40.0, 55.0]
    values[("B2-G", "voltage_pu")] = [1.045, 1.04]
    plan = Schedule.from_values(TIMES[:2], values)
    run = run_powsybl(network, plan, frequency_hz=60.0, start_time_s=0.2, ramp_time_s=1.0, settle_time_s=10.0,
                      governor=FAST)
    (row,), (tr,) = run.rows, run.transitions
    assert row["status"] == "completed"
    assert changes(tr)["loads and shunts that change"] >= 4 and changes(tr)["machines whose set points move"] == 5
    assert row["max_abs_dv_pu"] < 1e-9 and row["drift_before_change_pu"] < DRIFT
    # All generators are machines (no source): the end is the load flow of
    # to_time within the error of the time step.
    assert row["end_max_abs_dv_pu"] < 1e-4 and row["end_max_abs_dp_mw"] < 0.02
    assert row["end_frequency_hz"] == pytest.approx(60.0, abs=1e-3)
    expected = tr.expected[next(e["component"] for e in tr.conversion.mapping["elements"]
                                if e["element"].endswith("/B2-G"))]
    assert expected["active_power_mw"] == pytest.approx(55.0, abs=1e-6)


def test_measure_reports_a_failed_run():
    net = ring()
    plan = Schedule.from_values(TIMES[:2], {("gen/0", "active_power_mw"): [60.0, 80.0]})
    tr = transition(net, plan, start_time_s=0.1, ramp_time_s=0.1, settle_time_s=0.1)
    tr.model["components"].append({"type": "resistor", "name": "bad", "nodes": ["x", "x2"], "resistance_ohm": -1.0})
    result = openkeito.run(tr.model, check=False)
    row = measure(tr, result)
    assert row["status"] != "completed" and "end_max_abs_dv_pu" not in row
    assert math.isfinite(tr.end_time_s)



# Parts of a load that are almost nothing -------------------------------------------


def test_a_plan_of_the_active_power_alone_runs_and_ends_at_the_power_flow():
    """Only the active power of the loads of case9 is scheduled (0.8, 0.9 and
    0.85 of it), the reactive power stays. From the second hour to the third
    the conductance of a load falls while its susceptance rises a little with
    the falling voltage (Q / V^2): the part that comes in is an inductance
    alone, 0.014 to 0.057 Mvar per step at 345 kV, 5900 H and more. Behind
    its open switch nothing else ties its node to ground, and the solver took
    the equations for singular in the first part of the first step (E200 at
    t = 0: there an inductance counts as h / (1e6 L), which is below its
    threshold of 1e-13 S from L = 1e7 h on, 500 H at 50 us). Such a part now
    has 1 W of active power (LEAK_MW), as the nodes behind the breakers of a
    branch have."""
    from openkeito.transition import LEAK_MW

    net = pn.case9()
    hours = [f"2026-11-21T{hour:02d}:00:00+09:00" for hour in (8, 9, 10)]
    inputs = Schedule.from_values(hours, {(f"load/{k}", "active_power_mw"): [f * float(net.load.p_mw[k])
                                                                             for f in (0.8, 0.9, 0.85)]
                                          for k in net.load.index})
    plan, _ = schedule_from_opf(net, inputs)
    options = {"start_time_s": 0.5, "ramp_time_s": 1.0, "settle_time_s": 10.0}
    tr = transition(net, plan, hours[1], hours[2], **options)
    coming = [c for c in tr.model["components"] if c["type"] == "three_phase_load" and "_in" in c["name"]]
    assert len(coming) == 3 * 4
    for c in coming:
        assert c["active_power_mw"] == LEAK_MW and 0.01 < c["reactive_power_mvar"] < 0.06
        # Above 1e7 h: what the solver could not start with.
        henry = (c["rated_voltage_kv"] * 1e3) ** 2 / (2 * math.pi * 60.0 * c["reactive_power_mvar"] * 1e6)
        assert henry > 10 * 1e7 * 5e-5
    assert openkeito.validate(tr.model)["valid"]
    run = run_transitions(net, plan, **options)
    assert [row["status"] for row in run.rows] == ["completed", "completed"]
    # The end is the power flow as in the other transitions of case9 (5e-5 pu,
    # 5e-3 degrees, 0.02 MW: the error of the time step; the watts of the
    # leaks are 1e-6 of that). Measured 7.9e-6 and 9.1e-6 pu, 5e-5 and
    # 6.2e-4 degrees, 8.9e-4 and 2.7e-3 MW.
    for row in run.rows:
        # The start: 1.2e-9 and 2.5e-11 pu from the power flow (what the
        # power flow of this plan leaves of its own tolerance of 1e-8 MVA).
        assert row["max_abs_dv_pu"] < 1e-8 and row["drift_before_change_pu"] < DRIFT
        assert row["end_max_abs_dv_pu"] < 5e-5 and row["end_max_abs_dva_degree"] < 5e-3
        assert row["end_max_abs_dp_mw"] < 0.02
    # All three hours in one run: the same ends (there the inductances are
    # connected at the start and go out).
    chain = run_transitions(net, plan, chain=True, **options)
    for at, alone in zip(chain.time_rows, run.rows):
        assert at["status"] == "completed"
        assert at["end_max_abs_dv_pu"] == pytest.approx(alone["end_max_abs_dv_pu"], abs=1e-7)
        assert at["end_max_abs_dp_mw"] == pytest.approx(alone["end_max_abs_dp_mw"], abs=1e-4)


def test_parts_that_are_an_inductance_a_capacitance_or_a_conductance_alone():
    """The same load at two times with a small change of one kind, as two
    conversions whose second is the first with another load. What is switched
    must tie its node to ground by more than the solver's 1e-13 S."""
    from openkeito.transition import LEAK_MW

    a = from_pandapower(ring(), generators="machine", inverter_model="converter")

    def changed(dp_mw: float, dq_mvar: float, name: str = "load0") -> Conversion:
        b = Conversion(model=json.loads(json.dumps(a.model)), mapping=a.mapping, warnings=[],
                       power_flow=a.power_flow)
        load = next(c for c in b.model["components"] if c["name"] == name)
        load["active_power_mw"] += dp_mw
        load["reactive_power_mvar"] += dq_mvar
        return b

    def parts(tr) -> list[tuple[float, float]]:
        return [(c["active_power_mw"], c["reactive_power_mvar"]) for c in tr.model["components"]
                if c["type"] == "three_phase_load" and ("_in" in c["name"] or "_out" in c["name"])]

    options = {"start_time_s": 0.02, "ramp_time_s": 0.02, "settle_time_s": 0.02}
    # 10 kvar more of inductive load at 110 kV: 2.5 kvar per step, 15 000 H.
    # Each part carries the 1 W leak; with column scaling in the LU singularity
    # check (#34), even the pure inductance (active_power_mw = 0) completes.
    tr = build(a, changed(0.0, 0.01), **options)
    assert parts(tr) == [(LEAK_MW, pytest.approx(0.0025))] * 4
    assert openkeito.run(tr.model).ok
    for c in tr.model["components"]:
        if c["type"] == "three_phase_load" and "_in" in c["name"]:
            c["active_power_mw"] = 0.0
    assert openkeito.run(tr.model).ok
    # And less: 10 kvar less, which go out.
    tr = build(a, changed(0.0, -0.01), **options)
    assert parts(tr) == [(LEAK_MW, pytest.approx(0.0025))] * 4 and openkeito.run(tr.model).ok
    # Three times, with levels of the susceptance 4 and 6 kvar apart.
    tr = build_sequence([a, changed(0.0, 0.01), changed(0.0, 0.004)], **options)
    assert parts(tr) == [(LEAK_MW, pytest.approx(0.001))] * 4 + [(LEAK_MW, pytest.approx(0.0015))] * 4
    assert openkeito.run(tr.model).ok
    # A conductance alone needs no leak: 100 W more.
    tr = build(a, changed(1e-4, 0.0), **options)
    assert parts(tr) == [(pytest.approx(2.5e-5), 0.0)] * 4 and openkeito.run(tr.model).ok
    # Nor does a capacitance: the 8 Mvar of load/1 go out (inductances, with
    # the leak) and 10 var of capacitors come in.
    q = next(c for c in a.model["components"] if c["name"] == "load1")["reactive_power_mvar"]
    tr = build(a, changed(0.0, -q - 1e-5, "load1"), **options)
    assert parts(tr) == [(LEAK_MW, pytest.approx(q / 4))] * 4 + [(0.0, pytest.approx(-2.5e-6, rel=1e-3))] * 4
    assert openkeito.run(tr.model).ok
    # A change below 1 W per step is not switched (2 W in four steps here):
    # the load stays as it is, and the model is that of no change.
    for dp, dq in ((2e-6, 0.0), (0.0, 2e-6), (0.0, -2e-6)):
        tr = build(a, changed(dp, dq), **options)
        assert parts(tr) == [] and tr.changes == []
        assert json.dumps(tr.model) == json.dumps(build(a, a, **options).model)
    # (At 1e-13 S a conductance alone is 1.2 mW at 110 kV: such a part, were
    # it switched, would stop the steady state with E203.)


def test_sources_only_network_with_follow_keeps_steady_state_frequency_reference():
    """A network with only an external grid, lines, transformers and loads
    has its source replaced by a controlled source when sources='follow';
    a 0 A current source holds the frequency reference so steady-state
    initialization starts from the power flow instead of 0 (W184)."""
    from openkeito.transition import run as run_transition

    net = pp.create_empty_network(f_hz=50.0, sn_mva=100.0)
    b0, b1, b2 = pp.create_bus(net, 110.0), pp.create_bus(net, 110.0), pp.create_bus(net, 20.0)
    pp.create_ext_grid(net, b0, vm_pu=1.02, s_sc_max_mva=3000.0, rx_max=0.1)
    pp.create_line_from_parameters(net, b0, b1, 20.0, r_ohm_per_km=0.06, x_ohm_per_km=0.4, c_nf_per_km=9.0,
                                   max_i_ka=1.0)
    pp.create_transformer_from_parameters(net, b1, b2, sn_mva=40.0, vn_hv_kv=110.0, vn_lv_kv=20.0,
                                          vk_percent=10.0, vkr_percent=0.5, pfe_kw=20.0, i0_percent=0.1,
                                          vector_group="YNyn")
    pp.create_load(net, b2, p_mw=15.0, q_mvar=5.0)
    plan = Schedule.from_values(TIMES[:2], {("load/0", "active_power_mw"): [15.0, 20.0]})
    tr = transition(net, plan, start_time_s=0.1, ramp_time_s=0.2, settle_time_s=0.3, sources="follow")
    assert any(c["type"] == "three_phase_controlled_source" for c in tr.model["components"])
    assert any(c["type"] == "three_phase_current_source" and c["current_rms_a"] == 0.0
               for c in tr.model["components"])
    run_res = run_transition([tr])
    row = run_res.rows[0]
    assert row["status"] == "completed"
    assert row["max_abs_dv_pu"] < 1e-9 and row["max_abs_dva_degree"] < 1e-8
    assert "start_checked" not in row

    # When initialization phasors are missing or W184 occurred, measure() flags start_checked=False.
    no_ref = transition(net, plan, start_time_s=0.05, ramp_time_s=0.05, settle_time_s=0.05, sources="follow")
    no_ref.model["components"] = [c for c in no_ref.model["components"] if c["name"] != "transition_freq_ref"]
    res_w184 = openkeito.run(no_ref.model)
    assert any(w.get("code") == "W184" for w in res_w184.warnings)
    w_row = measure(no_ref, res_w184)
    assert w_row["start_checked"] is False and "W184" in w_row["start_warning"]
    assert "max_abs_dv_pu" not in w_row


def test_powsybl_bus_rename_does_not_collide_for_initially_dead_bus():
    """IEEE 14 with L7-8-1 going 0 -> 1 leaves VL8_0 dead at t0 and live at t1.
    VL8_0 gets a fresh bus name that does not collide with VL10_0."""
    ppl = pytest.importorskip("pypowsybl")
    from openkeito.powsybl import transition as powsybl_transition

    net = ppl.network.create_ieee14()
    plan_machine = Schedule.from_values(TIMES[:2], {("L7-8-1", "in_service"): [0, 1]})
    # With B8-G as a starting synchronous machine at dead VL8_0, it must fail
    # the dead-bus check rather than silently attaching to VL10_0.
    with pytest.raises(ConversionError, match="no voltage"):
        powsybl_transition(net, plan_machine, frequency_hz=60.0)

    # With B8-G out of service, L7-8-1 switches in to VL8_0's own fresh bus.
    plan = Schedule.from_values(TIMES[:3], {("L7-8-1", "in_service"): [0, 1, 1],
                                            ("B8-G", "in_service"): [0, 0, 0]})
    tr = powsybl_transition(net, plan, times="all", frequency_hz=60.0, start_time_s=0.1, ramp_time_s=0.2,
                            settle_time_s=0.3, governor=FAST)
    assert tr.end_buses["VL8_0"] != tr.end_buses["VL10_0"]
    assert tr.end_buses["VL8_0"] not in set(tr.start.mapping["buses"].values())
    assert len(set(tr.end_buses.values())) == len(tr.end_buses)
    assert tr.stages[0].buses["VL8_0"] == tr.stages[1].buses["VL8_0"] == tr.end_buses["VL8_0"]
    row = measure(tr, openkeito.run(tr.model))
    assert row["status"] == "completed" and row["max_abs_dv_pu"] < 1e-9


def test_switched_branch_leak_uses_terminal_bus_rated_voltage():
    """Bus 0 is 0.4 kV and a 110 kV line is switched out: the 1 W leak on each
    switched line end uses 110 kV rather than bus 0's 0.4 kV."""
    net = pp.create_empty_network(f_hz=50.0, sn_mva=100.0)
    b0 = pp.create_bus(net, 0.4)
    b1, b2, b3 = pp.create_bus(net, 110.0), pp.create_bus(net, 110.0), pp.create_bus(net, 110.0)
    pp.create_ext_grid(net, b1, vm_pu=1.0, s_sc_max_mva=5000.0, rx_max=0.1)
    pp.create_transformer_from_parameters(net, b1, b0, sn_mva=1.0, vn_hv_kv=110.0, vn_lv_kv=0.4,
                                          vk_percent=6.0, vkr_percent=0.5, pfe_kw=1.0, i0_percent=0.1,
                                          vector_group="Dyn")
    pp.create_line_from_parameters(net, b1, b2, 20.0, r_ohm_per_km=0.06, x_ohm_per_km=0.4, c_nf_per_km=9.0,
                                   max_i_ka=1.0)
    pp.create_line_from_parameters(net, b2, b3, 20.0, r_ohm_per_km=0.06, x_ohm_per_km=0.4, c_nf_per_km=9.0,
                                   max_i_ka=1.0)
    pp.create_line_from_parameters(net, b1, b3, 20.0, r_ohm_per_km=0.06, x_ohm_per_km=0.4, c_nf_per_km=9.0,
                                   max_i_ka=1.0)
    pp.create_load(net, b3, p_mw=20.0, q_mvar=5.0)
    pp.create_load(net, b0, p_mw=0.1, q_mvar=0.02)
    plan = Schedule.from_values(TIMES[:2], {("line/0", "in_service"): [1, 0]})
    tr = transition(net, plan, start_time_s=0.1, ramp_time_s=0.2, settle_time_s=0.5, sources="hold")
    leaks = [c for c in tr.model["components"] if c["name"].startswith("line0_x") and c["name"].endswith("_leak")]
    assert len(leaks) == 2 and all(c["rated_voltage_kv"] == pytest.approx(110.0) for c in leaks)
    row = measure(tr, openkeito.run(tr.model))
    assert row["status"] == "completed" and row["max_abs_dv_pu"] < 1e-9


def test_switched_branch_shunt_admittances_move_behind_the_breaker():
    """When a PowSyBl or pandapower branch is switched out, its shunt
    admittances (line_y1/y2, line_g0/g1, tr_y) sit on the branch side of the
    breaker rather than ramping in 4 load steps on the bus."""
    ppl = pytest.importorskip("pypowsybl")
    from openkeito.powsybl import transition as powsybl_transition

    # PowSyBl line switched out: L3-4-1 has shunt susceptance at both ends.
    net = ppl.network.create_ieee14()
    plan = Schedule.from_values(TIMES[:2], {("L3-4-1", "in_service"): [1, 0]})
    tr = powsybl_transition(net, plan, frequency_hz=60.0, start_time_s=0.2, ramp_time_s=1.0,
                            settle_time_s=8.0, governor=FAST)
    line_comp = next(e["component"] for e in tr.conversion.mapping["elements"] if e["element"] == "line/L3-4-1")
    shunts = [c for c in tr.model["components"] if c["name"].startswith(f"{line_comp}_y")]
    assert len(shunts) == 2 and {c["bus"] for c in shunts} == {f"{line_comp}_x0", f"{line_comp}_x1"}
    assert not any(c["name"].startswith(f"{line_comp}_y") and ("_in" in c["name"] or "_out" in c["name"])
                   for c in tr.model["components"])
    row = measure(tr, openkeito.run(tr.model))
    assert row["status"] == "completed" and row["max_abs_dv_pu"] < 1e-9

    # PowSyBl two-winding transformer with magnetizing branch switched out.
    net.update_2_windings_transformers(id="T4-9-1", g=2e-4, b=-3e-3)
    tr_plan = Schedule.from_values(TIMES[:2], {("T4-9-1", "in_service"): [1, 0]})
    tr_xf = powsybl_transition(net, tr_plan, frequency_hz=60.0, start_time_s=0.1, ramp_time_s=0.2,
                               settle_time_s=0.5)
    tr_comp = next(e["component"] for e in tr_xf.conversion.mapping["elements"]
                   if e["element"] == "two_windings_transformer/T4-9-1")
    assert not any(c["name"].startswith(f"{tr_comp}_y_") or c["name"].startswith(f"{tr_comp}_z_brk")
                   for c in tr_xf.model["components"])
    assert any(c["name"] == f"{tr_comp}_y" and c["bus"] == f"{tr_comp}_m" for c in tr_xf.model["components"])
    assert openkeito.run(tr_xf.model).ok

    # pandapower line with g_us_per_km switched out.
    pp_net = ring()
    pp_net.line.at[1, "g_us_per_km"] = 40.0
    pp_plan = Schedule.from_values(TIMES[:2], {("line/1", "in_service"): [1, 0]})
    pp_tr = transition(pp_net, pp_plan, start_time_s=0.2, ramp_time_s=1.0, settle_time_s=8.0, governor=FAST)
    pp_shunts = [c for c in pp_tr.model["components"] if c["name"].startswith("line1_g")]
    assert len(pp_shunts) == 2 and {c["bus"] for c in pp_shunts} == {"line1_x0", "line1_x1"}
    assert not any(c["name"].startswith("line1_g") and ("_in" in c["name"] or "_out" in c["name"])
                   for c in pp_tr.model["components"])
    assert measure(pp_tr, openkeito.run(pp_tr.model))["end_max_abs_dv_pu"] < 1e-4


def test_unknown_keys_and_invalid_options_raise_value_error():
    net = ring()
    plan = Schedule.from_values(TIMES[:2], {("gen/0", "active_power_mw"): [60.0, 70.0]})
    with pytest.raises(ValueError, match="machine_parameters"):
        from_pandapower(net, generators="machine", machine_parameters={"gen/99": {"inertia_h_s": 5.0}})
    with pytest.raises(ValueError, match="machine_controls"):
        transition(net, plan, machine_controls={"gen/99": {"pss": False}})
    with pytest.raises(ValueError, match="ramp_time_s"):
        transition(net, plan, ramp_time_s={"nope": 1.0})
    with pytest.raises(ValueError, match="ramp_rate_mw_per_min"):
        transition(net, plan, ramp_rate_mw_per_min={"nope": 100.0})
    a = from_pandapower(net, generators="machine")
    with pytest.raises(ValueError, match="machines"):
        add_machine_controls(a, machines={"gen/99": {"pss": False}})
    with pytest.raises(ValueError, match="ramp_time_s must be a number"):
        build(a, a, ramp_time_s={"gen/0": "schedule"})
    # Repeated switching faster than one power cycle is refused.
    fast_plan = Schedule.from_values(TIMES[:3], {("line/1", "in_service"): [1, 0, 1]})
    with pytest.raises(ValueError, match="power cycle"):
        transition(net, fast_plan, times="all", start_time_s=0.1, ramp_time_s=0.0, settle_time_s=0.005)


def test_stopping_machine_and_starting_converter_follow_bus_splits():
    """B2 & D3: a stopping machine and a starting converter follow bus splits
    through moved_ends, and a starting converter checks dead buses."""
    net = ring()
    b_split = pp.create_bus(net, 110.0)
    net.gen.at[1, "bus"] = b_split
    net.line.at[2, "to_bus"] = b_split
    pp.create_switch(net, 3, b_split, et="b", closed=True)
    # Time 0: closed (b3); Time 1: split (b_split); Time 2: gen/1 stops.
    c0 = from_pandapower(net, generators="machine", inverter_model="converter")
    net.switch.at[0, "closed"] = False
    c1 = from_pandapower(net, generators="machine", inverter_model="converter")
    net.gen.at[1, "in_service"] = False
    c2 = from_pandapower(net, generators="machine", inverter_model="converter")
    tr = build_sequence([c0, c1, c2], times=TIMES, start_time_s=0.2, ramp_time_s=0.5, settle_time_s=4.0)
    brk = [c for c in tr.model["components"] if c["name"].startswith("gen1_brk")]
    assert brk and brk[0]["nodes"][0].startswith(f"b{b_split}.")
    assert openkeito.validate(tr.model)["valid"]

    # Starting converter on a dead bus is refused just like a starting machine.
    dead = ring()
    extra = pp.create_bus(dead, 110.0)
    pp.create_line_from_parameters(dead, 3, extra, 10.0, r_ohm_per_km=0.06, x_ohm_per_km=0.4, c_nf_per_km=9.0,
                                   max_i_ka=1.0)
    pp.create_sgen(dead, extra, p_mw=5.0, q_mvar=0.0, sn_mva=10.0)
    plan = Schedule.from_values(TIMES[:2], {("line/4", "in_service"): [0, 1], ("sgen/1", "in_service"): [0, 1]})
    with pytest.raises(ConversionError, match="no voltage"):
        transition(dead, plan)
