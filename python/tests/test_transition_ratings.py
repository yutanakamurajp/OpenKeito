"""The rating of a generator that has none in the data, in a transition.

A time slice rates such a machine by the output of its time
(``openkeito.convert.machine_rating``). A transition needs one machine for
all its times: ``openkeito.convert.machine_rating_for``.
"""

from __future__ import annotations

import math

import pytest

import openkeito
from openkeito.convert import MACHINE_LOADING, TYPICAL_MACHINE, machine_rating_for
from openkeito.schedule import Schedule
from openkeito.transition import build, common_machine_ratings, machine_inputs, measure

pp = pytest.importorskip("pandapower")

from openkeito.pandapower import apply_schedule, from_pandapower, time_slices, transition  # noqa: E402

from test_transition import FAST, ring  # noqa: E402

TIMES = [f"2026-11-21T{hour:02d}:00:00+09:00" for hour in range(3)]


def unrated():
    """The ring with a second generator whose rating the data does not give."""
    net = ring()
    net.gen.at[1, "sn_mva"] = math.nan
    return net


def machine(conversion, name="gen1"):
    return next(c for c in conversion.model["components"] if c["name"] == name)


def test_machine_rating_for():
    # The largest apparent power is 85 % of the rating.
    assert machine_rating_for([(30 + 40j, 1.0), (60 + 0j, 1.02)]) == pytest.approx(60.0 / 0.85)
    # Absorbing 9 Mvar at 1.0 pu with Xd = 1.8: 9 * 1.8 / 0.85 = 19.06 MVA, and
    # not the 10.6 MVA of the apparent power. The field voltage of that point
    # is then V - Xd (-Q) / V = 1 - 0.85 = 0.15 pu, and not -0.53 pu.
    assert machine_rating_for([(-9j, 1.0)]) == pytest.approx(9.0 / 0.85)
    rating = machine_rating_for([(-9j, 1.0)], TYPICAL_MACHINE["xd_pu"])
    assert rating == pytest.approx(9.0 * 1.8 / 0.85)
    typical = {**TYPICAL_MACHINE, "ra_pu": 0.0}
    assert machine_inputs({**typical, "rated_power_mva": rating}, -9j, 1.0)[0] == pytest.approx(0.15)
    # (Rated at 10.6 MVA, ``machine_inputs`` gives +0.53 pu on a q axis that
    # points against the terminal voltage, since -Q is beyond V^2 / Xq as
    # well: the same state.)
    assert machine_inputs({**typical, "rated_power_mva": 9.0 / 0.85}, -9j, 1.0)[0] == pytest.approx(1.8 * 0.85 - 1)
    # At a lower voltage it can absorb less: V^2 / Xd.
    assert machine_rating_for([(-9j, 0.95)], 1.8) == pytest.approx(9.0 * 1.8 / 0.85 / 0.95 ** 2)
    # Delivering reactive power asks for nothing more.
    assert machine_rating_for([(9j, 1.0)], 1.8) == pytest.approx(9.0 / 0.85)


def test_a_machine_without_rating_is_rated_for_all_times():
    """gen/1 delivers 10, 40 and 25 MW. Rated at the first time alone (31 MVA
    for 10 MW and its reactive power) it would be asked for 1.3 pu at the
    second."""
    net = unrated()
    plan = Schedule.from_values(TIMES, {("gen/1", "active_power_mw"): [10.0, 40.0, 25.0],
                                        ("gen/0", "active_power_mw"): [60.0, 50.0, 60.0]})
    options = {"start_time_s": 0.2, "ramp_time_s": 1.0, "settle_time_s": 10.0, "governor": FAST}
    tr = transition(net, plan, times="all", **options)
    powers = [abs(complex(machine(c)["operating_point"]["active_power_mw"],
                          machine(c)["operating_point"]["reactive_power_mvar"]))
              for c in [tr.start] + [s.conversion for s in tr.stages]]
    rating = max(powers) / MACHINE_LOADING
    assert powers[1] == max(powers) and rating > 40.0 / 0.85
    # One machine for all times, in the model and in the conversions.
    assert machine(tr.conversion)["rated_power_mva"] == pytest.approx(rating, rel=1e-9)
    assert [machine(s.conversion)["rated_power_mva"] for s in tr.stages] == pytest.approx([rating] * 2, rel=1e-9)
    assert [w for w in tr.warnings if "rated for all times" in w] == [
        "no rating; rated for all times of the transition (the largest apparent power is 85 % of the rating, and "
        f"the reactive power absorbed at most 85 % of V^2 / Xd): gen/1 ({rating:.4g} MVA)"]
    assert not any("governor valve" in w or "field voltage" in w for w in tr.warnings)
    result = openkeito.run(tr.model)
    for k in (1, 2):
        row = measure(tr, result, time=k)
        # The bounds of the ring (test_transition.py); measured 7.7e-6 pu and
        # 1.2e-3 MW.
        assert row["end_max_abs_dv_pu"] < 1e-4 and row["end_max_abs_dp_mw"] < 0.02
    # A rating that is given is kept.
    kept = transition(net, plan, times="all", machine_parameters={"gen/1": {"rated_power_mva": 55.0}}, **options)
    assert machine(kept.conversion)["rated_power_mva"] == 55.0
    assert not any("rated for all times" in w for w in kept.warnings)

    # The time slices are as before: each rated by its own output.
    slices = time_slices(net, plan, generators="machine")
    assert [machine(s.conversion)["rated_power_mva"] for s in slices] == pytest.approx(
        [max(p, 1.0) / MACHINE_LOADING for p in powers], rel=1e-6)
    for s, time in zip(slices, TIMES):
        assert s.model == from_pandapower(apply_schedule(net, plan, time), generators="machine").model

    # Rated by the first time alone (``build`` of two conversions cannot
    # convert again): the second time asks for 1.3 pu of torque, the valve
    # stops at 1.1 pu, and a warning says so. The machine then ends short by
    # what is above 1.1 pu of its rating (6 MW).
    a, b = (from_pandapower(apply_schedule(net, plan, time), generators="machine") for time in TIMES[:2])
    assert machine(a)["rated_power_mva"] == pytest.approx(powers[0] / MACHINE_LOADING, rel=1e-6)
    assert common_machine_ratings([a, b]) == {"gen/1": (pytest.approx(rating, rel=1e-6), "gen1")}
    short = build(a, b, from_time=TIMES[0], to_time=TIMES[1], **options)
    (warning,) = [w for w in short.warnings if "governor valve" in w]
    assert "above the upper limit of the governor valve (1.1 pu of the rating)" in warning
    assert warning.endswith(f"gen/1 (1.3 pu at {TIMES[1]})")
    assert 40.0 / machine(a)["rated_power_mva"] == pytest.approx(1.3, abs=0.05)
    row = measure(short, openkeito.run(short.model))
    assert row["end_max_abs_dp_element"] == "gen/1"
    assert row["end_max_abs_dp_mw"] == pytest.approx(40.0 - 1.1 * machine(a)["rated_power_mva"], abs=0.5)


def test_a_machine_that_absorbs_reactive_power_is_rated_for_it():
    """A generator without active power that holds its bus at 0.98 pu absorbs
    reactive power (a synchronous condenser). Rated by its apparent power it
    would need a negative field voltage."""
    net = unrated()
    plan = Schedule.from_values(TIMES[:2], {("gen/1", "active_power_mw"): [0.0, 0.0],
                                            ("gen/1", "voltage_pu"): [0.98, 0.985],
                                            ("load/0", "active_power_mw"): [70.0, 75.0]})
    options = {"start_time_s": 0.2, "ramp_time_s": 1.0, "settle_time_s": 10.0, "governor": FAST}
    tr = transition(net, plan, **options)
    g = machine(tr.conversion)
    q = [machine(c)["operating_point"]["reactive_power_mvar"] for c in (tr.start, tr.end)]
    v = [machine(c)["operating_point"]["voltage_pu"] for c in (tr.start, tr.end)]
    assert max(q) < -10.0
    # The reactive power absorbed is 85 % of V^2 / Xd of the rating at the
    # time that asks for the most.
    assert g["rated_power_mva"] == pytest.approx(max(-x * g["xd_pu"] / (0.85 * u ** 2) for x, u in zip(q, v)), rel=1e-9)
    assert g["rated_power_mva"] > 1.5 * max(-x for x in q) / 0.85
    # The field voltage is 0.15 pu at that time (and positive at the other).
    fields = [machine_inputs(g, complex(0.0, x), u)[0] for x, u in zip(q, v)]
    # (0.147: the rule takes the machine without active power and without
    # stator resistance, and the power flow gives it 1e-3 MW.)
    assert min(fields) == pytest.approx(0.15, abs=5e-3) and not any("field voltage" in w for w in tr.warnings)
    row = measure(tr, openkeito.run(tr.model))
    # Measured 7.6e-6 pu and 1.0e-3 MW.
    assert row["status"] == "completed" and row["end_max_abs_dv_pu"] < 1e-4 and row["end_max_abs_dp_mw"] < 0.02

    # With the rating of the apparent power alone, given by hand: a warning.
    small = transition(net, plan, machine_parameters={"gen/1": {"rated_power_mva": -min(q) / 0.85}}, **options)
    (warning,) = [w for w in small.warnings if "field voltage" in w]
    assert "negative" in warning and "gen/1 (-0.85 pu of reactive power at" in warning
