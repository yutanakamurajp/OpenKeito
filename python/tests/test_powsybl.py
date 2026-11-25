"""PowSyBl (IIDM, CGMES) networks against their AC load flow.

The converter writes IIDM's equivalent circuits (README, "From a load flow
(PowSyBl, CGMES)") and gives what the load flow left unbalanced at a bus to a
generator or an injection there, so the load-flow voltages solve the model
exactly: the differences are of rounding, about 1e-12 pu. The tests allow
1e-9.
"""

from __future__ import annotations

import math
from datetime import datetime, timedelta, timezone

import numpy as np
import pytest

pn = pytest.importorskip("pypowsybl.network")

import openkeito  # noqa: E402
from openkeito.convert import ConversionError, fault  # noqa: E402
from openkeito.powsybl import (  # noqa: E402
    check_schedule,
    from_cgmes,
    from_powsybl,
    imbalance,
    run_time_slices,
)
from openkeito.schedule import Schedule  # noqa: E402

TOL = 1e-9
NETWORKS = ["create_micro_grid_be_network", "create_micro_grid_nl_network", "create_ieee14", "create_ieee57",
            "create_ieee118", "create_eurostag_tutorial_example1_network"]


@pytest.mark.parametrize("make", NETWORKS)
def test_the_steady_state_is_the_load_flow(make):
    net = getattr(pn, make)()
    conv = from_powsybl(net, outputs="all", end_time_s=0.02)
    result = openkeito.run(conv.model)
    c = conv.compare(result)
    assert c["max_abs_dv_pu"] < TOL and c["max_abs_dva_degree"] < TOL, c
    # Each generator delivers its load-flow power, plus what the load flow
    # left unbalanced at its bus when it took that.
    residual = imbalance(net)
    powers = conv.source_powers(result)
    gens = net.get_generators()
    for e in conv.mapping["elements"]:
        if not e["element"].startswith("generator/"):
            continue
        gid = e["element"].split("/", 1)[1]
        ref = -complex(gens.at[gid, "p"], gens.at[gid, "q"])
        allowed = abs(residual.get(gens.at[gid, "bus_id"], 0j)) + TOL * max(abs(ref), 1.0)
        assert abs(powers[e["component"]] - ref) <= allowed, (gid, powers[e["component"]], ref)
    w_h = 2 * math.pi * conv.mapping["frequency_hz"] * conv.model["simulation"]["time_step_s"]
    assert result.summary["initialization"]["relative_voltage_deviation"] < w_h ** 2
    assert not result.warnings


def test_micro_grid_be_has_every_kind_of_branch():
    conv = from_powsybl(pn.create_micro_grid_be_network())
    kinds = {e["element"].split("/")[0] for e in conv.mapping["elements"]}
    assert {"generator", "load", "shunt_compensator", "line", "two_windings_transformer",
            "three_windings_transformer", "dangling_line"} <= kinds
    shifters = [c for c in conv.model["components"]
                if c["type"] == "three_phase_ideal_transformer" and abs(c["phase_shift_deg"]) > 1e-9]
    assert len(shifters) == 1 and shifters[0]["phase_shift_deg"] == pytest.approx(2.1475854, abs=1e-6)
    # The three legs of the three-winding transformer meet at its star node.
    legs = [c for c in conv.model["components"] if c["type"] == "coupled_rl" and c["name"].startswith("tr3_0_")]
    assert {c["buses"][1] for c in legs} == {"tr3_0_star"}
    assert openkeito.validate(conv.model)["valid"]


def test_a_file_is_read_with_pypowsybl(tmp_path):
    path = tmp_path / "be.xiidm"
    pn.create_micro_grid_be_network().save(str(path), format="XIIDM")
    conv = from_cgmes(path)
    assert conv.compare(openkeito.run(conv.model))["max_abs_dv_pu"] < TOL


def test_what_cannot_be_converted_is_listed():
    with pytest.raises(ConversionError) as e:
        from_powsybl(pn.create_four_substations_node_breaker_network())
    assert any(p.startswith("hvdc_lines") for p in e.value.problems)


def test_schedules_and_time_slices_on_an_iidm_network():
    net = pn.create_ieee14()
    jst = timezone(timedelta(hours=9))
    times = [datetime(2026, 11, 21, h, tzinfo=jst) for h in range(3)]
    loads = net.get_loads()
    values = {(lid, "active_power_mw"): [loads.at[lid, "p0"] * f for f in (0.8, 0.9, 1.0)] for lid in loads.index}
    values[("B2-G", "active_power_mw")] = [30.0, 35.0, 40.0]
    values[("B2-G", "voltage_pu")] = [1.04, 1.045, 1.045]
    plan = Schedule.from_values(times, values, name="ieee14 hours")
    run = run_time_slices(net, plan, [fault("VL9_0", 0.01, 0.03)], end_time_s=0.04, jobs=2, keep_rows=True,
                          outputs="all")
    assert [r["case"] for r in run.rows] == ["t000.fault_VL9_0_abc", "t001.fault_VL9_0_abc",
                                             "t002.fault_VL9_0_abc"]
    for row, s in zip(run.rows, run.slices):
        assert row["status"] == "completed"
        assert row["max_abs_dv_pu"] < TOL and row["max_abs_dva_degree"] < TOL
        result = run.batch[s.name]
        v, t = result[f"V({s.conversion.bus('VL9_0')}.a)"], result.time
        assert np.max(np.abs(v[(t > 0.015) & (t < 0.025)])) < 0.01 * np.max(np.abs(v[t < 0.01]))
    # Generator B2-G delivers its scheduled active power in every slice.
    for s, p in zip(run.slices, [30.0, 35.0, 40.0]):
        gen = next(e["component"] for e in s.conversion.mapping["elements"] if e["element"] == "generator/B2-G")
        assert s.conversion.source_powers(run.batch[s.name])[gen].real == pytest.approx(p, rel=1e-9)
    with pytest.raises(ConversionError) as e:
        check_schedule(net, Schedule.from_values(times[:1], {("nope", "active_power_mw"): [1.0],
                                                             ("L1-2-1", "voltage_pu"): [1.0]}))
    assert len(e.value.problems) == 2


# Branches with an open end, dead buses and load flows that fail ---------------


def steady_state(net, **options):
    """(conversion, largest voltage difference in pu or degrees)."""
    conv = from_powsybl(net, outputs="all", end_time_s=0.02, **options)
    result = openkeito.run(conv.model)
    assert not result.warnings
    c = conv.compare(result)
    return conv, max(c["max_abs_dv_pu"], c["max_abs_dva_degree"])


def converted(conv):
    return {e["element"] for e in conv.mapping["elements"]}


@pytest.mark.parametrize("side", [1, 2])
def test_a_line_open_at_one_end_keeps_its_charging_current(side):
    net = pn.create_ieee14()
    net.update_lines(id="L2-3-1", **{f"connected{side}": False})
    conv, worst = steady_state(net)
    assert worst < TOL
    assert abs(net.get_lines().at["L2-3-1", f"q{3 - side}"]) > 4.0
    assert "line/L2-3-1" in converted(conv)
    # Open at both ends, the line is out.
    net.update_lines(id="L2-3-1", connected1=False, connected2=False)
    conv, worst = steady_state(net)
    assert worst < TOL and "line/L2-3-1" not in converted(conv)


@pytest.mark.parametrize("side", [1, 2])
def test_a_transformer_open_at_one_end_keeps_its_magnetizing_current(side):
    net = pn.create_ieee14()
    net.update_2_windings_transformers(id="T4-7-1", g=2e-4, b=-3e-3, **{f"connected{side}": False})
    conv, worst = steady_state(net)
    assert worst < TOL
    assert abs(net.get_2_windings_transformers().at["T4-7-1", f"q{3 - side}"]) > 0.1
    assert "two_windings_transformer/T4-7-1" in converted(conv)
    # Without a magnetizing branch nothing flows, and nothing is built.
    net.update_2_windings_transformers(id="T4-7-1", g=0.0, b=0.0)
    conv, worst = steady_state(net)
    assert worst < TOL and "two_windings_transformer/T4-7-1" not in converted(conv)


@pytest.mark.parametrize("leg", [1, 2, 3])
def test_a_three_winding_transformer_with_an_open_leg(leg):
    net = pn.create_micro_grid_be_network()
    tid = net.get_3_windings_transformers().index[0]
    net.update_3_windings_transformers(id=tid, b2=3e-6, g3=1e-4, b3=-2e-4, **{f"connected{leg}": False})
    conv, worst = steady_state(net)
    assert worst < TOL
    assert f"three_windings_transformer/{tid}" in converted(conv)


def test_a_network_without_load_flow_results_is_refused():
    with pytest.raises(ConversionError) as e:
        from_powsybl(pn.create_ieee14(), run_load_flow=False)
    assert "run_load_flow" in e.value.problems[0]


def test_dead_buses_are_named():
    net = pn.create_ieee14()
    for lid in ("L6-12-1", "L12-13-1"):
        net.update_lines(id=lid, connected1=False, connected2=False)
    conv, worst = steady_state(net)
    assert worst < TOL and "VL12_0" not in conv.mapping["buses"]
    dead = [w for w in conv.warnings if "VL12_0" in w]
    assert len(dead) == 1 and "left out" in dead[0]
    assert not any("left out" in w for w in from_powsybl(pn.create_ieee14()).warnings)


def test_a_load_flow_that_fails_says_how_to_change_its_parameters():
    # Without a distributed slack, OpenLoadFlow does not solve this case.
    net = pn.create_ieee14()
    net.update_lines(id="L1-2-1", connected2=False)
    with pytest.raises(ConversionError) as e:
        from_powsybl(net)
    assert "load_flow_parameters" in e.value.problems[0]
    import pypowsybl.loadflow as lf

    # With the default parameters it does, less exactly: the generators and
    # the injections take what is left, and the warnings give the amounts.
    conv, worst = steady_state(net, load_flow_parameters=lf.Parameters())
    assert worst < TOL and any("unbalanced" in w for w in conv.warnings)


def test_a_tap_schedule_needs_a_ratio_tap_changer():
    net = pn.create_ieee14()
    time = ["2026-11-21T00:00:00Z"]
    with pytest.raises(ConversionError) as e:
        check_schedule(net, Schedule.from_values(time, {("T4-7-1", "tap_position"): [1.0]}))
    assert "ratio tap changer" in e.value.problems[0]
    net = pn.create_micro_grid_be_network()
    changers = net.get_ratio_tap_changers()
    tid = next(t for t in changers.index if t in net.get_2_windings_transformers().index)
    check_schedule(net, Schedule.from_values(time, {(tid, "tap_position"): [float(changers.at[tid, "low_tap"])]}))
    with pytest.raises(ConversionError) as e:
        check_schedule(net, Schedule.from_values(time, {(tid, "tap_position"): [changers.at[tid, "high_tap"] + 1.0]}))
    assert "tap_position" in e.value.problems[0]


def test_ieee118_negative_conductance_splits_capacitance_for_second_order_accuracy():
    """P1: when active power / conductance is negative and reactive power != 0,
    the reactive part stays a constant-impedance load and only active power
    becomes a current source. On IEEE 118 this restores 2nd-order convergence
    at VL87_0 and keeps the worst bus angle error <= 4.1e-3 deg."""
    import cmath

    net = pn.create_ieee118()
    errs = []
    for h in (5e-5, 2.5e-5):
        conv = from_powsybl(net, end_time_s=0.04, time_step_s=h, outputs="buses")
        r = openkeito.run(conv.model)
        c = conv.compare(r)
        assert c["max_abs_dv_pu"] < 1e-12 and c["max_abs_dva_degree"] < 1e-11
        t = r.time
        mask = t >= 0.02
        w0 = 2 * math.pi * 50.0
        a = np.column_stack([np.cos(w0 * t[mask]), -np.sin(w0 * t[mask]), np.ones(mask.sum())])
        worst = 0.0
        vl87_err = 0.0
        for key, bus in conv.mapping["buses"].items():
            coef, *_ = np.linalg.lstsq(a, r[f"V({bus}.a)"][mask], rcond=None)
            fit = complex(coef[0], coef[1])
            _, va, _ = conv.power_flow[key]
            d = abs((math.degrees(cmath.phase(fit)) - va + 180.0) % 360.0 - 180.0)
            worst = max(worst, d)
            if key == "VL87_0":
                vl87_err = d
        errs.append((worst, vl87_err))
    # Previously VL87_0 was 2.43e-2 deg with 1st-order convergence.
    assert errs[0][1] < 1e-3 and errs[0][0] <= 4.1e-3
    assert errs[0][0] / errs[1][0] > 3.5 and errs[0][1] / errs[1][1] > 3.5


def test_powsybl_machine_parameters_accept_element_keys_and_reject_unknown():
    """D1: machine_parameters accepts both 'generator/<id>' and '<id>', and
    raises ValueError on keys matching no generator."""
    net = pn.create_ieee9()
    gid = str(net.get_generators().index[0])
    conv = from_powsybl(net, generators="machine", machine_parameters={f"generator/{gid}": {"inertia_h_s": 7.5}})
    comp = next(e["component"] for e in conv.mapping["elements"] if e["element"] == f"generator/{gid}")
    m = next(c for c in conv.model["components"] if c["name"] == comp)
    assert m["inertia_h_s"] == 7.5
    with pytest.raises(ValueError, match="machine_parameters"):
        from_powsybl(net, generators="machine", machine_parameters={"generator/NOPE": {"inertia_h_s": 5.0}})
