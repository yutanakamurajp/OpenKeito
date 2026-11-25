"""Transitions through more than two times of a schedule in one run.

What a run through the times t1, t2, ..., tn is compared with: the power
flow of every time at the end of the settling time of that time, as the
tests of test_transition.py do for the end of a transition between two
times, and the runs of the intervals as cases of their own. Each tolerance
is derived where it is used.
"""

from __future__ import annotations

import copy
import json

import numpy as np
import pytest

import openkeito
from openkeito.convert import ConversionError
from openkeito.schedule import Schedule
from openkeito.transition import build, build_sequence, measure

pp = pytest.importorskip("pandapower")
pn = pytest.importorskip("pandapower.networks")

from openkeito.pandapower import (  # noqa: E402
    from_pandapower, run_transitions, schedule_from_opf, transition)

from test_transition import DRIFT, FAST, changes, ring  # noqa: E402

TIMES = [f"2026-11-21T{hour:02d}:00:00+09:00" for hour in range(4)]


def opf_plan(net, factors):
    """Loads scaled by ``factors`` at the hours of TIMES, and the generator
    set points of the AC OPF."""
    values = {}
    for i, row in net.load.iterrows():
        values[(f"load/{i}", "active_power_mw")] = [row.p_mw * f for f in factors]
        values[(f"load/{i}", "reactive_power_mvar")] = [row.q_mvar * f for f in factors]
    plan, _ = schedule_from_opf(net, Schedule.from_values(TIMES[:len(factors)], values, name="loads"))
    return plan


def switches(tr, name: str) -> list[dict]:
    """What the switches of phase a whose names begin with ``name`` do."""
    return [{k: v for k, v in c.items() if k in ("initially_closed", "close_time_s", "open_time_s")}
            for c in tr.model["components"]
            if c["type"] == "switch" and c["name"].startswith(name) and c["name"].endswith("_a")]


def spans(tr) -> list[float]:
    """For every time after the first: when its changes begin, when they
    end, and when its settling time ends."""
    return [x for s in tr.stages for x in (s.change_time_s, s.changes_end_s, s.settled_s)]


# Two times: the same as before ----------------------------------------------------


def test_two_times_in_a_list_are_the_transition_between_two_times():
    """``times`` with two entries, ``build_sequence`` with two conversions
    and lists of one entry per interval give the model of the call with
    from_time and to_time, down to the text of its JSON, and so the same
    result bit for bit. (The models of sixteen transitions between two
    times, with breakers, machines that start and stop, taps, rates, held
    sources, an island and IEEE 14 from PowSyBl, were compared in the same
    way with those of the code before it took more than two times.)"""
    net = ring()
    plan = Schedule.from_values(TIMES[:2], {
        ("line/1", "in_service"): [1, 0], ("gen/1", "in_service"): [1, 0], ("gen/0", "active_power_mw"): [60.0, 85.0],
        ("load/2", "in_service"): [0, 1], ("shunt/0", "in_service"): [1, 0],
        ("sgen/0", "active_power_mw"): [10.0, 4.0], ("sgen/0", "reactive_power_mvar"): [0.0, 2.0]})
    options = {"start_time_s": 0.1, "ramp_time_s": 0.2, "settle_time_s": 0.3, "output_interval_steps": 1}
    one = transition(net, plan, TIMES[0], TIMES[1], **options)
    for other in (transition(net, plan, times=TIMES[:2], **options),
                  transition(net, plan, times="all", **{**options, "ramp_time_s": [0.2], "settle_time_s": [0.3]})):
        assert json.dumps(other.model) == json.dumps(one.model)
        assert other.warnings == one.warnings and other.changes == one.changes and other.expected == one.expected
        assert (other.change_time_s, other.changes_end_s, other.end_time_s) == (
            one.change_time_s, one.changes_end_s, one.end_time_s)
    assert one.times == TIMES[:2] and [s.time for s in one.stages] == [TIMES[1]]
    assert one.stage().settled_s == one.end_time_s and one.stage(1) is one.stage(TIMES[1]) is one.stage(-1)
    a, b = from_pandapower(net, generators="machine"), from_pandapower(copy.deepcopy(net), generators="machine")
    assert json.dumps(build_sequence([a, b]).model) == json.dumps(build(a, b).model)

    first, second = openkeito.run(one.model), openkeito.run(other.model)
    assert first.ok and first.columns == second.columns
    assert all(np.array_equal(first[c], second[c]) for c in first.columns)
    # The row of the end is the row of its time.
    row = measure(one, first)
    assert measure(one, first, time=1) == {**row, "time": TIMES[1], "time_s": float(first.time[-1])}


# The plan of an OPF, hour by hour -------------------------------------------------


def test_case9_through_four_times_arrives_at_every_power_flow():
    """The loads of case9 at 0.8, 0.9, 1.0 and 0.85 of their values with the
    set points of the AC OPF of every hour, in one run of 36.5 s: 0.5 s of
    steady state, and for every hour a ramp of 2 s and 10 s of settling.
    The machines have what a transition gives them unless told otherwise:
    the typical excitation system and governor, and the stabilizer."""
    net = pn.case9()
    plan = opf_plan(net, (0.8, 0.9, 1.0, 0.85))
    options = {"start_time_s": 0.5, "ramp_time_s": 2.0, "settle_time_s": 10.0}
    chain = run_transitions(net, plan, chain=True, **options)
    (row,), (tr,) = chain.rows, chain.transitions
    assert row["case"] == "chain" and row["status"] == "completed"
    assert (row["from_time"], row["to_time"]) == (TIMES[0], TIMES[3]) and tr.times == TIMES
    assert spans(tr) == pytest.approx([0.5, 2.5, 12.5, 12.5, 14.5, 24.5, 24.5, 26.5, 36.5])
    assert tr.end_time_s == pytest.approx(36.5) and tr.model["simulation"]["end_time_s"] == pytest.approx(36.5)
    # The loads go up twice and down once, and the generators follow.
    assert changes(tr) == {"machines whose set points move": 2, "loads and shunts that change": 3,
                           "sources whose internal voltage moves": 1}
    block = next(b for b in tr.model["controls"] if b["name"] == "gen0_dpref")
    assert block["interpolation"] == "linear"
    assert block["times_s"] == pytest.approx([0.5, 2.5, 12.5, 14.5, 24.5, 26.5])
    assert block["values"][0] == 0.0 and block["values"][1] == block["values"][2] < block["values"][3] \
        and block["values"][5] < block["values"][4] == block["values"][3]
    # The start is the power flow of the first hour, and nothing moves before
    # the first change (as for two times).
    assert row["max_abs_dv_pu"] < 1e-9 and row["max_abs_dva_degree"] < 1e-8
    assert row["drift_before_change_pu"] < DRIFT
    assert 0.95 < row["voltage_min_pu"] < row["voltage_max_pu"] < 1.1 and row["max_machine_current_pu"] < 1.0

    # At the end of the settling time of every hour the run is at the power
    # flow of that hour. With PSS damping (-1.9 1/s) over 10 s, swings decay
    # below exp(-19) ~ 5e-9; what remains is a fraction of (w h)^2 = 3.55e-4
    # at 60 Hz, 50 us (test_transition.py: 5e-5 pu, 5e-3 deg, 0.02 MW).
    # Measured: 8.0e-6, 8.1e-6 and 9.7e-6 pu, 1.1e-4, 6.6e-5 and 1.2e-3 deg,
    # 3.8e-4, 8.1e-4 and 4.7e-3 MW.
    assert [r["time"] for r in chain.time_rows] == TIMES[1:]
    assert [r["time_s"] for r in chain.time_rows] == pytest.approx([12.5, 24.5, 36.5])
    for at in chain.time_rows:
        assert at["end_max_abs_dv_pu"] < 5e-5 and at["end_max_abs_dva_degree"] < 5e-3
        assert at["end_max_abs_dp_mw"] < 0.02
        # The governors (droop 5 %) are still 1.6e-5 Hz from the rated
        # frequency, and the last 0.1 s move by 1e-6 pu and 5e-6 Hz.
        assert at["end_frequency_hz"] == pytest.approx(60.0, abs=1e-4)
        assert at["end_voltage_swing_pu"] < 5e-6 and at["end_frequency_swing_hz"] < 2e-5
    assert chain.time_rows[-1] == {**row, "time": TIMES[3], "time_s": pytest.approx(36.5)}

    # Against the intervals as cases of their own (the default), which each
    # start from the exact steady state of their first hour. The first
    # interval of the chain is the same run but for rounding (the model has
    # the parts of the loads of the later hours in it as well). In the later
    # ones the chain starts from where the interval before it has come to
    # after 10 s, and not from the power flow: 8e-6 pu and 4e-4 MW away, with
    # the swings of the machines still at 1e-6 pu. That difference decays
    # over the next interval as the swings do (they shrink to 0.07 in 4 s
    # with the stabilizer, test_transition.py), so the ends differ by far
    # less than either differs from the power flow: measured 4.9e-10 and
    # 1.1e-9 pu, 3.3e-7 and 5.8e-7 degrees, 1.2e-6 and 1.7e-5 MW.
    apart = run_transitions(net, plan, **options)
    assert [r["case"] for r in apart.rows] == ["i000", "i001", "i002"]
    assert apart.time_rows == [{**r, "time": r["to_time"], "time_s": pytest.approx(12.5)} for r in apart.rows]
    differences = []
    for at, alone in zip(chain.time_rows, apart.rows):
        differences.append([abs(at[key] - alone[key])
                            for key in ("end_max_abs_dv_pu", "end_max_abs_dva_degree", "end_max_abs_dp_mw")])
        assert at["time"] == alone["to_time"]
    print("chain against the intervals:", differences)
    assert max(differences[0]) < 1e-9
    for dv, dva, dp in differences[1:]:
        assert dv < 1e-7 and dva < 1e-4 and dp < 1e-4


def test_a_short_settling_time_carries_the_swings_into_the_next_interval():
    """What one run shows and the intervals as cases of their own do not:
    with 1 s of settling, the second change begins while the machines still
    swing from the first. At the end of its settling time the chain is then
    further from the power flow than the interval that started at rest."""
    net = pn.case9()
    plan = opf_plan(net, (0.8, 0.9, 1.0))
    options = {"start_time_s": 0.5, "ramp_time_s": 1.0, "settle_time_s": 1.0}
    chain = run_transitions(net, plan, chain=True, **options)
    apart = run_transitions(net, plan, **options)
    first, second = chain.time_rows
    # 1 s after the first ramp (measured): 0.52 MW from the power flow of the
    # second hour in both, to rounding.
    assert first["end_max_abs_dp_mw"] == pytest.approx(apart.rows[0]["end_max_abs_dp_mw"], rel=1e-6)
    assert first["end_max_abs_dp_mw"] > 0.1
    # 1 s after the second ramp: the chain is 0.34 MW from the power flow of
    # the third hour, the interval on its own 0.47 MW. They differ by what
    # was left of the first interval when the second began (0.52 MW then, a
    # quarter of it a ramp and a second later), which here happens to work
    # against the swing of the second ramp.
    difference = abs(second["end_max_abs_dp_mw"] - apart.rows[1]["end_max_abs_dp_mw"])
    print("short settling:", first["end_max_abs_dp_mw"], second["end_max_abs_dp_mw"],
          apart.rows[1]["end_max_abs_dp_mw"])
    assert 0.02 < difference < first["end_max_abs_dp_mw"]


# Loads that go up and down, breakers that open and close --------------------------


def test_a_load_that_rises_and_falls():
    """load/0 goes from 70 MW and 20 Mvar to 90 and 25, then to 60 and -5
    (capacitive). What all three hours have (58 MW at the voltages of the
    power flows) stays connected; the rest is parts behind switches."""
    net = ring()
    plan = Schedule.from_values(TIMES[:3], {("load/0", "active_power_mw"): [70.0, 90.0, 60.0],
                                            ("load/0", "reactive_power_mvar"): [20.0, 25.0, -5.0]})
    tr = transition(net, plan, times="all", start_time_s=0.2, ramp_time_s=1.0, settle_time_s=10.0, governor=FAST)
    assert changes(tr)["loads and shunts that change"] == 2
    parts = {c["name"]: c for c in tr.model["components"] if c["name"].startswith("load0")
             and c["type"] == "three_phase_load"}
    # The part of all hours has no reactive power: the sign changes.
    assert parts["load0"]["reactive_power_mvar"] == 0.0 and 55.0 < parts["load0"]["active_power_mw"] < 60.0
    # In four parts each: what the first hour has and the third does not
    # (connected at the start, out in the second interval), what only the
    # second hour has (in, then out), and the capacitors of the third (in).
    assert sorted(parts) == sorted(["load0"] + [f"load0_{kind}{k}" for kind in ("out", "in", "in2_")
                                                for k in range(4)])
    assert switches(tr, "load0_out0_sw") == [{"initially_closed": True, "open_time_s": pytest.approx(11.325)}]
    assert switches(tr, "load0_in3_sw") == [{"close_time_s": pytest.approx(1.075),
                                            "open_time_s": pytest.approx(12.075)}]
    assert switches(tr, "load0_in2_1_sw") == [{"close_time_s": pytest.approx(11.575)}]
    assert parts["load0_in2_0"]["reactive_power_mvar"] < 0 and parts["load0_in2_0"]["active_power_mw"] == 0.0
    # Together they are the admittance of the load of every hour (the power
    # of a part is at its rated voltage, which is that of the power flow of
    # the hour it was made from).
    def siemens(c):
        return complex(c["active_power_mw"], c["reactive_power_mvar"]) / c["rated_voltage_kv"] ** 2

    for k, at in enumerate([tr.start] + [s.conversion for s in tr.stages]):
        want = siemens(next(c for c in at.model["components"] if c["name"] == "load0"))
        on = [parts["load0"]] + [c for name, c in parts.items() if name != "load0" and (
            name.startswith("load0_out") and k < 2 or name.startswith("load0_in2_") and k == 2
            or name.startswith("load0_in") and not name.startswith("load0_in2_") and k == 1)]
        assert sum(siemens(c) for c in on) == pytest.approx(want, rel=1e-12)
    assert openkeito.validate(tr.model)["valid"]
    result = openkeito.run(tr.model)
    rows = [measure(tr, result, time=k) for k in (1, 2)]
    assert rows[0]["max_abs_dv_pu"] < 1e-9 and rows[0]["drift_before_change_pu"] < DRIFT
    # Every switch has done what it was to do: the parts of load/0 (twelve
    # that open, twelve that close and open, twelve that close) and those of
    # the small change that the other load at 110 kV has with the voltage.
    actions = sum(("close_time_s" in c) + ("open_time_s" in c) for c in tr.model["components"]
                  if c["type"] == "switch")
    assert rows[0]["events"] == actions and actions >= 3 * 4 * (1 + 2 + 1)
    # The bounds of the ring between two times (test_transition.py: 1e-4 pu,
    # 0.01 degrees, 0.02 MW). Measured 7.6e-6 and 8.3e-6 pu, 1e-4 degrees,
    # 1.1e-3 MW.
    for row in rows:
        assert row["end_max_abs_dv_pu"] < 1e-4 and row["end_max_abs_dva_degree"] < 0.01
        assert row["end_max_abs_dp_mw"] < 0.02 and row["end_frequency_hz"] == pytest.approx(50.0, abs=1e-4)


def test_breakers_that_open_and_close_again():
    """A line, a shunt and a load go out at the second hour and come back at
    the third, while a generator takes more and gives it back."""
    net = ring()
    plan = Schedule.from_values(TIMES[:3], {
        ("line/1", "in_service"): [1, 0, 1], ("shunt/0", "in_service"): [1, 0, 1],
        ("load/2", "in_service"): [1, 0, 1], ("gen/0", "active_power_mw"): [60.0, 85.0, 60.0]})
    tr = transition(net, plan, times="all", start_time_s=0.2, ramp_time_s=1.0, settle_time_s=10.0, governor=FAST)
    assert changes(tr) == {"sources whose internal voltage moves": 1, "machines whose set points move": 2,
                           "loads and shunts that change": 4, "branches switched out": 1}
    # One set of switches at each end of the line opens and closes again.
    for end in (0, 1):
        assert switches(tr, f"line1_brk{end}") == [{"initially_closed": True, "open_time_s": pytest.approx(0.2),
                                                    "close_time_s": pytest.approx(11.2)}]
    report = openkeito.validate(tr.model)
    assert report["valid"] and not report.get("diagnostics")
    result = openkeito.run(tr.model)
    rows = [measure(tr, result, time=time) for time in TIMES[1:3]]
    assert rows[0]["max_abs_dv_pu"] < 1e-9 and rows[0]["drift_before_change_pu"] < DRIFT
    # Measured 7.7e-6 pu, 1.7e-4 degrees and 1.4e-3 MW; bounds as above.
    for row in rows:
        assert row["status"] == "completed"
        assert row["end_max_abs_dv_pu"] < 1e-4 and row["end_max_abs_dva_degree"] < 0.01
        assert row["end_max_abs_dp_mw"] < 0.02
    # The third hour is the first again: the same power flow, and the run is
    # back at it.
    for key, voltage in tr.start.power_flow.items():
        assert tr.stages[1].conversion.power_flow[key] == pytest.approx(voltage)
    back = [c["name"] for c in tr.model["components"] if c["type"] == "switch" and "close_time_s" in c
            and "open_time_s" in c]
    assert len(back) == 3 * (2 + 4 + 4 + 2 * 4)


def split_ring(closed: bool):
    """The ring with line/1 on a bus of its own that a bus-bus switch joins
    to bus 1."""
    net = ring()
    bus = pp.create_bus(net, 110.0)
    net.line.at[1, "from_bus"] = bus
    pp.create_switch(net, 1, bus, et="b", closed=closed)
    return from_pandapower(net, generators="machine", inverter_model="converter")


def test_a_bus_that_is_split_and_joined_again():
    joined, split = split_ring(True), split_ring(False)
    options = {"start_time_s": 0.2, "ramp_time_s": 0.5, "settle_time_s": 10.0, "governor": FAST}
    for sequence, expected in (
            ([joined, split, joined], {"initially_closed": True, "open_time_s": pytest.approx(0.2),
                                       "close_time_s": pytest.approx(10.7)}),
            ([split, joined, split], {"close_time_s": pytest.approx(0.2), "open_time_s": pytest.approx(10.7)})):
        tr = build_sequence(sequence, **options)
        assert switches(tr, "brk_") == [expected]
        result = openkeito.run(tr.model)
        rows = [measure(tr, result, time=k) for k in (1, 2)]
        assert rows[0]["max_abs_dv_pu"] < 1e-9
        # 10 s after the breaker: measured 7.8e-6 pu and 1.1e-3 MW (after
        # 5 s the swings are still at 2e-4 pu).
        for row in rows:
            assert row["status"] == "completed" and row["end_max_abs_dv_pu"] < 1e-4
            assert row["end_max_abs_dp_mw"] < 0.02


# Machines that start and stop -----------------------------------------------------


def test_machines_start_and_stop_on_the_way():
    net = ring()
    # gen/1 gives up half its power, then stops.
    plan = Schedule.from_values(TIMES[:3], {
        ("gen/1", "in_service"): [1, 1, 0], ("gen/1", "active_power_mw"): [40.0, 20.0, 20.0],
        ("gen/0", "active_power_mw"): [60.0, 70.0, 85.0]})
    tr = transition(net, plan, times="all", start_time_s=0.2, ramp_time_s=1.0, settle_time_s=10.0, governor=FAST)
    assert changes(tr)["machines that stop (breaker opens)"] == 1 and tr.stopped == ["gen1"]
    assert switches(tr, "gen1_brk") == [{"initially_closed": True, "open_time_s": pytest.approx(11.2)}]
    block = next(b for b in tr.model["controls"] if b["name"] == "gen1_dpref")
    # The load reference: the ramp to 20 MW (0.25 pu of 80 MVA), then to zero
    # with the breaker, within the time step before it.
    assert block["times_s"] == pytest.approx([0.2, 1.2, 11.2 - 5e-5, 11.2])
    assert block["values"] == pytest.approx([0.0, -0.25, -0.25, -0.5], rel=0.01)
    assert "gen1" in tr.stages[0].expected and "gen1" not in tr.stages[1].expected
    result = openkeito.run(tr.model)
    for k in (1, 2):
        row = measure(tr, result, time=k)
        # Measured 6.9e-6 and 8.7e-6 pu, 1.4e-3 MW.
        assert row["end_max_abs_dv_pu"] < 1e-4 and row["end_max_abs_dp_mw"] < 0.02
    assert abs(result["S(gen1_p)"][-1]) < 1e-6

    # gen/1 starts at the third hour: it is in the model from the beginning
    # without load, follows the voltage of its bus to the second hour, and
    # takes its load at the third.
    plan = Schedule.from_values(TIMES[:3], {
        ("gen/1", "in_service"): [0, 0, 1], ("gen/1", "active_power_mw"): [0.0, 0.0, 40.0],
        ("load/0", "active_power_mw"): [70.0, 80.0, 80.0]})
    tr = transition(net, plan, times="all", start_time_s=0.2, ramp_time_s=1.0, settle_time_s=10.0, governor=FAST)
    assert changes(tr)["machines that start (connected without load from the beginning)"] == 1
    blocks = {b["name"]: b for b in tr.model["controls"]}
    assert blocks["gen1_dpref"]["times_s"] == pytest.approx([11.2, 12.2])
    assert blocks["gen1_dvref"]["times_s"] == pytest.approx([0.2, 1.2, 11.2, 12.2])
    assert tr.stages[0].expected["gen1"] == {"active_power_mw": 0.0, "reactive_power_mvar": 0.0}
    result = openkeito.run(tr.model)
    for k in (1, 2):
        row = measure(tr, result, time=k)
        # Measured 7.8e-6 and 8.9e-6 pu, 1.1e-3 MW.
        assert row["end_max_abs_dv_pu"] < 1e-4 and row["end_max_abs_dp_mw"] < 0.02

    # Out and in again without governor or exciter: refused with E700 and hint.
    plan = Schedule.from_values(TIMES[:3], {("gen/1", "in_service"): [1, 0, 1],
                                            ("gen/1", "active_power_mw"): [40.0, 40.0, 35.0],
                                            ("load/0", "active_power_mw"): [70.0, 50.0, 70.0]})
    with pytest.raises(ConversionError, match=r"E700:.*gen/1.*governor=True"):
        transition(net, plan, times="all", governor=False)
    with pytest.raises(ConversionError, match=r"E700:.*gen/1.*exciter=True"):
        transition(net, plan, times="all", exciter=False)

    # With governor and exciter: trips, synchronizes to the bus voltage/phase, closes smoothly and takes load.
    tr = transition(net, plan, times="all", start_time_s=0.2, ramp_time_s=0.5, settle_time_s=10.0, governor=FAST)
    assert changes(tr)["machines that stop (breaker opens)"] == 1
    assert changes(tr)["machines that re-synchronize (breaker closes)"] == 1
    assert tr.stopped == []
    assert switches(tr, "gen1_brk") == [{
        "initially_closed": True,
        "open_time_s": pytest.approx(0.2),
        "close_time_s": pytest.approx(10.7),
    }]
    result = openkeito.run(tr.model)
    r_mid = measure(tr, result, time=1)
    r_end = measure(tr, result, time=2)
    assert r_mid["status"] == "completed" and r_mid["end_max_abs_dv_pu"] < 1e-4
    assert "gen1" not in tr.stages[0].expected and "gen1" in tr.stages[1].expected
    # Measured 9.2e-6 pu, 1.1e-3 MW; closing current below 0.01 pu.
    assert r_end["end_max_abs_dv_pu"] < 1e-4 and r_end["end_max_abs_dp_mw"] < 0.02, (r_mid, r_end)
    t = result.time
    closing = (t >= 10.69) & (t <= 10.75)
    assert float(np.max(np.hypot(result["S(gen1_id)"][closing], result["S(gen1_iq)"][closing]))) < 0.05


# What cannot be followed ----------------------------------------------------------


def test_taps_and_power_flows_that_fail_name_their_time():
    net = ring()
    plan = Schedule.from_values(TIMES[:3], {("trafo/0", "tap_position"): [0, 0, 2],
                                            ("load/0", "active_power_mw"): [70.0, 75.0, 70.0]})
    tr = transition(net, plan, times="all", start_time_s=0.1, ramp_time_s=0.0, settle_time_s=1.0)
    assert tr.model["version"] == "0.6"
    assert not any("tap change" in w for w in tr.warnings), tr.warnings
    assert changes(tr)["transformers whose taps move"] == 1
    # Steps only: one series that holds its values (excluding resync/other controls).
    block = next(b for b in tr.model["controls"] if b["name"] == "gen0_dpref")
    assert block["interpolation"] == "step" and block["times_s"] == pytest.approx([0.0, 0.1, 1.1])
    assert block["values"][0] == 0.0
    result = openkeito.run(tr.model)
    for k in (1, 2):
        row = measure(tr, result, time=k)
        assert row["status"] == "completed" and row["end_max_abs_dv_pu"] < 1e-4, row

    # A time whose power flow does not converge stops the chain, and is named.
    plan = Schedule.from_values(TIMES[:3], {("load/0", "active_power_mw"): [70.0, 75.0, 70000.0]})
    with pytest.raises(ConversionError, match=f"{TIMES[2][:13]}.*did not converge"):
        transition(net, plan, times="all")


def test_lengths_per_interval():
    net = ring()
    plan = Schedule.from_values(TIMES[:3], {("gen/0", "active_power_mw"): [60.0, 80.0, 70.0],
                                            ("line/1", "in_service"): [1, 1, 0]}, interpolation="linear")

    def series(tr, name):
        return next(b for b in tr.model["controls"] if b["name"] == name)["times_s"]

    tr = transition(net, plan, times="all", start_time_s=1.0, ramp_time_s=[2.0, {"gen/0": 0.5, "default": 1.0}],
                    settle_time_s=[3.0, 4.0], switch_delay_s=[None, 0.25])
    assert series(tr, "gen0_dpref") == pytest.approx([1.0, 3.0, 6.0, 6.5])
    assert series(tr, "ext_grid0_ed") == pytest.approx([1.0, 3.0, 6.0, 7.0])
    assert switches(tr, "line1_brk0") == [{"initially_closed": True, "open_time_s": pytest.approx(6.25)}]
    assert spans(tr) == pytest.approx([1.0, 3.0, 6.0, 6.0, 7.0, 11.0])
    # A rate for one interval, and the real time of the schedule for another
    # (30 s within the first hour; the breaker then acts at its end).
    times = [TIMES[0], "2026-11-21T00:00:30+09:00", TIMES[1], TIMES[2]]
    tr = transition(net, plan, times=times, start_time_s=1.0, ramp_time_s=["schedule", 1.0, 2.0],
                    ramp_rate_mw_per_min=[None, {"gen/0": 600.0}, None], settle_time_s=2.0, end_time_s=50.0)
    # 20 MW * 30 / 3600 in 30 s; the other 19.83 MW at 600 MW/min in 1.98 s.
    # The last interval begins 2 s after that.
    late = 33.0 + 19.8333 / 10.0 + 2.0
    assert series(tr, "gen0_dpref") == pytest.approx([1.0, 31.0, 33.0, late - 2.0, late, late + 2.0], abs=1e-3)
    assert tr.stages[2].settled_s == tr.end_time_s == pytest.approx(50.0)
    assert switches(tr, "line1_brk0") == [{"initially_closed": True, "open_time_s": pytest.approx(late, abs=1e-3)}]

    for options, message in (
            ({"ramp_time_s": [1.0]}, "ramp_time_s has 1 entries for 2 intervals"),
            ({"settle_time_s": [1.0, 2.0, 3.0]}, "settle_time_s has 3 entries for 2 intervals"),
            ({"switch_time_s": 2.0}, "switch_delay_s"),
            ({"settle_time_s": [0.0, 1.0]}, "at least two time steps"),
            ({"ramp_time_s": "plan"}, "not 'plan'"),
            ({"end_time_s": 3.0}, "before the end of the changes")):
        with pytest.raises(ValueError, match=message):
            transition(net, plan, times="all", **options)
    with pytest.raises(ValueError, match="not both"):
        transition(net, plan, TIMES[0], times=TIMES[:2])
    with pytest.raises(ValueError, match="do not go forward"):
        transition(net, plan, times=[TIMES[0], TIMES[2], TIMES[1]])
    with pytest.raises(ValueError, match="two times"):
        transition(net, plan, times=[TIMES[0]])
    with pytest.raises(ValueError, match="two times or more"):
        build_sequence([from_pandapower(net)])
    step_plan = Schedule.from_values(TIMES[:3], {("gen/0", "active_power_mw"): [60.0, 80.0, 70.0]})
    with pytest.raises(ValueError, match="interpolation"):
        transition(net, step_plan, times="all", ramp_time_s=[1.0, "schedule"])


def test_chain_or_intervals():
    """``run_transitions`` runs the intervals as cases of their own unless
    it is told to make one chain of them."""
    net = pn.case9()
    plan = opf_plan(net, (0.8, 0.9, 1.0, 0.85))
    options = {"start_time_s": 0.1, "ramp_time_s": 0.2, "settle_time_s": 0.3}
    apart = run_transitions(net, plan, **options)
    assert [t.name for t in apart.transitions] == ["i000", "i001", "i002"] and len(apart.time_rows) == 3
    chain = run_transitions(net, plan, chain=True, **options)
    assert [t.name for t in chain.transitions] == ["chain"] and chain.transitions[0].times == TIMES
    assert [r["time"] for r in chain.time_rows] == TIMES[1:]
    # The times given, or intervals that join.
    some = run_transitions(net, plan, times=[TIMES[0], TIMES[2], TIMES[3]], **options)
    assert some.transitions[0].times == [TIMES[0], TIMES[2], TIMES[3]]
    joined = run_transitions(net, plan, [[TIMES[1], TIMES[2]], [TIMES[2], TIMES[3]]], chain=True, **options)
    assert joined.transitions[0].times == TIMES[1:]
    assert joined.to_pandas().shape[0] == 1
    with pytest.raises(ValueError, match="do not join into one chain"):
        run_transitions(net, plan, [[TIMES[0], TIMES[1]], [TIMES[2], TIMES[3]]], chain=True, **options)
    with pytest.raises(ValueError, match="not both"):
        run_transitions(net, plan, [[TIMES[0], TIMES[1]]], times=TIMES[:2], **options)
    # A time of the chain by its position or its name.
    tr, result = chain.transitions[0], chain.batch["chain"]
    assert measure(tr, result, time=2) == measure(tr, result, time=TIMES[2]) == chain.time_rows[1]
    assert measure(tr, result, time=-1) == chain.time_rows[2]
    for wrong in (0, 4, TIMES[0], "2026-11-22T00:00:00+09:00"):
        with pytest.raises(ValueError, match="time"):
            measure(tr, result, time=wrong)


# PowSyBl --------------------------------------------------------------------------


def test_powsybl_through_three_times():
    ppl = pytest.importorskip("pypowsybl")
    from openkeito.powsybl import run_transitions as run_powsybl

    network = ppl.network.create_ieee14()
    loads = network.get_loads()
    values = {}
    for lid in loads.index[:4]:
        p0 = float(loads.at[lid, "p0"])
        values[(lid, "active_power_mw")] = [p0, 1.2 * p0, 0.9 * p0]
    values[("B2-G", "active_power_mw")] = [40.0, 55.0, 35.0]
    values[("B2-G", "voltage_pu")] = [1.045, 1.04, 1.045]
    plan = Schedule.from_values(TIMES[:3], values)
    run = run_powsybl(network, plan, chain=True, frequency_hz=60.0, start_time_s=0.2, ramp_time_s=1.0,
                      settle_time_s=10.0, governor=FAST)
    (row,), (tr,) = run.rows, run.transitions
    assert row["status"] == "completed" and tr.times == TIMES[:3]
    assert changes(tr)["loads and shunts that change"] >= 4 and changes(tr)["machines whose set points move"] == 5
    assert row["max_abs_dv_pu"] < 1e-9 and row["drift_before_change_pu"] < DRIFT
    # The bounds of the transition between two times of the same network
    # (test_transition.py). Measured: 5.5e-6 and 6.1e-6 pu, 1.1e-3 and
    # 1.9e-3 MW.
    for at, time, power in zip(run.time_rows, TIMES[1:], (55.0, 35.0)):
        assert at["time"] == time
        assert at["end_max_abs_dv_pu"] < 1e-4 and at["end_max_abs_dp_mw"] < 0.02
        assert at["end_frequency_hz"] == pytest.approx(60.0, abs=1e-3)
        expected = tr.stage(time).expected[next(e["component"] for e in tr.conversion.mapping["elements"]
                                                if e["element"].endswith("/B2-G"))]
        assert expected["active_power_mw"] == pytest.approx(power, abs=1e-6)


# The example ----------------------------------------------------------------------


def same(a, b, path="model"):
    """a and b are the same JSON but for the rounding of the power flow."""
    if isinstance(a, dict):
        assert isinstance(b, dict) and list(a) == list(b), path
        for key in a:
            same(a[key], b[key], f"{path}.{key}")
    elif isinstance(a, list):
        assert isinstance(b, list) and len(a) == len(b), path
        for k, (x, y) in enumerate(zip(a, b)):
            same(x, y, f"{path}[{k}]")
    elif isinstance(a, float) or isinstance(b, float):
        assert a == pytest.approx(b, rel=1e-6, abs=1e-9), path
    else:
        assert a == b, path


def test_the_example_is_what_the_tool_makes():
    """examples/transition_three_times.json is the model of this call, with a
    name and a description written by hand: a generator and a load at a bus,
    30 km from a grid, through three hours."""
    from pathlib import Path

    net = pp.create_empty_network(f_hz=50.0, sn_mva=100.0)
    b0, b1 = pp.create_bus(net, 110.0), pp.create_bus(net, 110.0)
    pp.create_ext_grid(net, b0, vm_pu=1.02, s_sc_max_mva=5000.0, rx_max=0.1)
    pp.create_line_from_parameters(net, b0, b1, 30.0, r_ohm_per_km=0.06, x_ohm_per_km=0.4, c_nf_per_km=9.0,
                                   max_i_ka=1.0)
    pp.create_gen(net, b1, p_mw=60.0, vm_pu=1.02, sn_mva=100.0)
    pp.create_load(net, b1, p_mw=70.0, q_mvar=20.0)
    hours = [f"2026-11-21T{hour:02d}:00:00+09:00" for hour in (9, 10, 11)]
    plan = Schedule.from_values(hours, {("load/0", "active_power_mw"): [70.0, 90.0, 60.0],
                                        ("load/0", "reactive_power_mvar"): [20.0, 25.0, 15.0],
                                        ("gen/0", "active_power_mw"): [60.0, 75.0, 55.0]})
    tr = transition(net, plan, times="all", start_time_s=0.5, ramp_time_s=1.0, settle_time_s=6.0, load_steps=2,
                    time_step_s=1e-4, output_interval_steps=20)
    path = Path(__file__).resolve().parents[2] / "examples" / "transition_three_times.json"
    example = json.loads(path.read_text(encoding="utf-8"))
    same({**tr.model, "name": example["name"], "description": example["description"]}, example)
    assert changes(tr) == {"sources whose internal voltage moves": 1, "machines whose set points move": 1,
                           "loads and shunts that change": 1}
    assert spans(tr) == pytest.approx([0.5, 1.5, 7.5, 7.5, 8.5, 14.5])
    # What its description says of the two hours it arrives at (at 100 us,
    # 6 s after each ramp, with the typical governor: measured 5.4e-5 and
    # 7.1e-5 pu, 5.3e-3 and 2.5e-3 MW; the swing of the machine against the
    # grid is still at 5e-4 Hz).
    result = openkeito.run(example)
    for k in (1, 2):
        row = measure(tr, result, time=k)
        assert row["status"] == "completed" and row["max_abs_dv_pu"] < 1e-9
        assert row["end_max_abs_dv_pu"] < 1e-4 and row["end_max_abs_dp_mw"] < 0.01
        assert row["settled"] is True


def test_estimate_and_insufficient_settling_warnings():
    """P2: estimate() counts steps/elements and calibrates wall time; short
    intermediate settling times emit both pre-run and post-run warnings."""
    from openkeito.transition import estimate

    net = pn.case9()
    plan = opf_plan(net, (0.8, 0.9, 1.0))

    # Well-settled chain: no settling warnings, all stages settled=True.
    long_tr = transition(net, plan, times="all", start_time_s=0.5, ramp_time_s=1.0, settle_time_s=10.0)
    assert not any("settle_time_s" in w for w in long_tr.warnings)
    est_dry = long_tr.estimate(calibrate=False)
    assert est_dry["stages"] == 2 and est_dry["steps"] == int(round(long_tr.end_time_s / est_dry["time_step_s"]))
    assert est_dry["components"] > 0 and est_dry["controls"] > 0 and "estimated_wall_s" not in est_dry
    est = estimate(long_tr, calibration_steps=50)
    assert est["step_wall_us"] > 0 and est["estimated_wall_s"] == pytest.approx(
        est["steps"] * est["step_wall_us"] * 1e-6, rel=1e-6)
    with pytest.raises(ValueError, match="calibration_steps"):
        estimate(long_tr, calibration_steps=0)

    # Short intermediate settling (1.0 s < PSS washout 1.4 s / recommended 2.0 s):
    # pre-run warning on Transition.warnings, settled=False on time_rows, and
    # post-run warning on TransitionRun.warnings.
    short_run = run_transitions(net, plan, chain=True, start_time_s=0.5, ramp_time_s=1.0, settle_time_s=1.0)
    short_tr = short_run.transitions[0]
    assert any("intermediate stage" in w and TIMES[1] in w and "settle_time_s" in w for w in short_tr.warnings)
    assert short_run.time_rows[0]["settled"] is False
    assert any("did not settle" in w and TIMES[1] in w and "increase settle_time_s" in w
               for w in short_run.warnings)

    long_run = run_transitions(net, plan, chain=True, start_time_s=0.5, ramp_time_s=1.0, settle_time_s=10.0)
    assert all(r["settled"] is True for r in long_run.time_rows)
    assert not any("did not settle" in w for w in long_run.warnings)
