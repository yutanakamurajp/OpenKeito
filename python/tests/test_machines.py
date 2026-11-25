"""Generators converted to synchronous machines (``generators="machine"``).

The machine is, in the phasor solution, a source behind its subtransient
impedance whose internal voltage follows from the power flow at its
terminals, so the model starts from the power flow as with sources: the
differences are of rounding. The rotor then starts from the solved terminal
values, and the run stays in the steady state but for the discretization
error, below (w h)^2.
"""

from __future__ import annotations

import math

import pytest

import openkeito
from openkeito.convert import TYPICAL_MACHINE, fault, place, synchronous_machine

TOL = 1e-6


def with_machine_currents(conv):
    """Adds the currents of the machines to the outputs of the model."""
    names = [c["name"] for c in conv.model["components"] if c["type"] == "synchronous_machine"]
    conv.model["outputs"] += [{"type": "branch_current", "component": n} for n in names]
    return names


def with_speeds(conv):
    """Adds a measurement of the speed of every machine; returns their names."""
    names = [c["name"] for c in conv.model["components"] if c["type"] == "synchronous_machine"]
    conv.model["controls"] = [
        {"type": "measurement", "name": f"speed_{n}",
         "quantity": {"type": "component_signal", "component": n, "signal": "speed_pu"}} for n in names]
    conv.model["outputs"] += [{"type": "signal", "signal": f"speed_{n}"} for n in names]
    return [f"S(speed_{n})" for n in names]


# pandapower -------------------------------------------------------------------


@pytest.mark.parametrize("case", ["case9", "case39"])
def test_pandapower_machines_start_from_the_power_flow(case):
    pn = pytest.importorskip("pandapower.networks")
    from openkeito.pandapower import from_pandapower

    net = getattr(pn, case)()
    conv = from_pandapower(net, end_time_s=0.05, generators="machine")
    machines = with_machine_currents(conv)
    assert len(machines) == len(net.gen) and conv.model["version"] == "0.5"
    # External grids stay sources.
    types = {e["element"].split("/")[0]: e["type"] for e in conv.mapping["elements"]}
    assert types["gen"] == "synchronous_machine" and types["ext_grid"] == "three_phase_source"

    result = openkeito.run(conv.model)
    c = conv.compare(result)
    assert c["max_abs_dv_pu"] < TOL and c["max_abs_dva_degree"] < TOL, c
    # Every machine delivers the power of the power flow.
    powers = conv.source_powers(result)
    for idx in net.gen.index:
        ref = complex(net.res_gen.at[idx, "p_mw"], net.res_gen.at[idx, "q_mvar"])
        assert abs(powers[f"gen{idx}"] - ref) < TOL * max(abs(ref), 1.0), (idx, powers[f"gen{idx}"], ref)
    # The run stays there: the rotors start in the steady state of the
    # solved terminal values.
    w_h = 2 * math.pi * conv.mapping["frequency_hz"] * conv.model["simulation"]["time_step_s"]
    init = result.summary["initialization"]
    assert init["relative_voltage_deviation"] < w_h ** 2
    assert init["relative_current_deviation"] < w_h ** 2
    assert not result.warnings
    # The stages of the machines need hardly any additional solution.
    solver = result.summary["solver"]
    assert solver["operating_point_iterations"] < solver["steps"]

    # What was assumed is said: the ratings, and the typical parameters.
    text = "\n".join(conv.warnings)
    assert "no rating; assumed that the output of the power flow (or max_p_mw) is 85 %" in text
    assert "typical machine parameters" in text and "inertia_h_s" in text


def test_pandapower_machine_parameters_come_from_the_caller_then_the_network():
    pytest.importorskip("pandapower")
    pn = pytest.importorskip("pandapower.networks")
    from openkeito.pandapower import from_pandapower

    net = pn.case9()
    net.gen["sn_mva"] = [250.0, float("nan")]
    net.gen["xdss_pu"] = [0.18, float("nan")]
    net.gen["rdss_ohm"] = [0.0, float("nan")]
    conv = from_pandapower(net, generators="machine", machine_parameters={
        "inertia_h_s": 6.0,
        "gen/1": {"rated_power_mva": 300.0, "xd_pu": 1.2, "xq_transient_pu": None, "inertia_h_s": 3.0},
    })
    g0, g1 = (c for c in conv.model["components"] if c["type"] == "synchronous_machine")
    # gen/0: rating and subtransient values of the network, the common
    # inertia, the rest typical.
    assert g0["rated_power_mva"] == 250.0
    assert g0["xd_subtransient_pu"] == g0["xq_subtransient_pu"] == 0.18 and g0["ra_pu"] == 0.0
    assert g0["inertia_h_s"] == 6.0 and g0["xd_pu"] == TYPICAL_MACHINE["xd_pu"]
    # gen/1: its own values win; without xq_transient_pu it has one damper
    # winding on the q axis.
    assert g1["rated_power_mva"] == 300.0 and g1["xd_pu"] == 1.2 and g1["inertia_h_s"] == 3.0
    assert "xq_transient_pu" not in g1 and "tq0_transient_s" not in g1
    # The operating point is the power flow at the terminals.
    bus = int(net.gen.at[1, "bus"])
    assert g1["operating_point"] == {
        "active_power_mw": pytest.approx(net.res_gen.at[1, "p_mw"]),
        "reactive_power_mvar": pytest.approx(net.res_gen.at[1, "q_mvar"]),
        "voltage_pu": pytest.approx(net.res_bus.at[bus, "vm_pu"]),
        "angle_deg": pytest.approx(net.res_bus.at[bus, "va_degree"]),
    }
    text = "\n".join(conv.warnings)
    assert "no rating" not in text  # both ratings are known now
    result = openkeito.run(conv.model)
    assert conv.compare(result)["max_abs_dv_pu"] < TOL

    with pytest.raises(ValueError, match="generators is 'machines'"):
        from_pandapower(pn.case9(), generators="machines")
    # The default is unchanged: sources, format 0.4.
    conv = from_pandapower(pn.case9())
    assert conv.model["version"] == "0.4"
    assert not any(c["type"] == "synchronous_machine" for c in conv.model["components"])


def test_typical_reactances_keep_their_order_around_the_given_ones():
    def machine(**data):
        return synchronous_machine("G", "b", frequency_hz=50.0, rated_power_mva=100.0, rated_voltage_kv=10.0,
                                   s_mva=50 + 10j, voltage_pu=1.0, angle_deg=0.0, data=data)

    # A subtransient reactance below the typical leakage, and one above the
    # typical transient reactance.
    for xdss in (0.1, 0.4):
        m, notes = machine(xd_subtransient_pu=xdss, xq_subtransient_pu=xdss)
        assert m["xd_subtransient_pu"] == xdss
        for axis in "dq":
            chain = [m[f"x{axis}_pu"], m[f"x{axis}_transient_pu"], m[f"x{axis}_subtransient_pu"], m["xl_pu"], 0.0]
            assert all(a > b for a, b in zip(chain, chain[1:])), chain
        assert any("moved" in n for n in notes)
    m, notes = machine()
    assert not any("moved" in n for n in notes)
    assert openkeito.validate({
        "format": "openkeito-model", "version": "0.5",
        "simulation": {"time_step_s": 1e-4, "end_time_s": 0.01},
        "components": [m, {"type": "resistor", "name": "R", "nodes": ["b.a", "gnd"], "resistance_ohm": 1.0}],
    })["valid"]


def test_machines_swing_together_after_a_fault():
    pn = pytest.importorskip("pandapower.networks")
    from openkeito.pandapower import from_pandapower

    net = pn.case9()
    conv = from_pandapower(net, end_time_s=1.0, time_step_s=1e-4, generators="machine",
                           output_interval_steps=50)
    speeds = with_speeds(conv)
    # A three-phase fault for 80 ms at a load bus.
    disturbance = fault(int(net.load.bus.iloc[0]), 0.1, 0.18)
    conv.model["components"] += [place(conv, c) for c in disturbance["components"]]
    result = openkeito.run(conv.model)
    assert result.summary["status"] == "completed"
    time = result.time
    for name in speeds:
        values = list(result[name])
        # At rated speed until the fault; then the rotors accelerate (the
        # fault takes their electrical power) by less than 2 %, and they
        # are still in step a second later.
        before = [v for t, v in zip(time, values) if t <= 0.1]
        assert max(abs(v - 1.0) for v in before) < 1e-5, name
        assert 1.0005 < max(values) < 1.02, (name, max(values))
        assert abs(values[-1] - 1.0) < 0.02, (name, values[-1])
    # Each stage of the step settles in a few solutions, also with two
    # machines that act on each other.
    assert result.summary["solver"]["max_operating_point_iterations_per_stage"] <= 8


# PowSyBl ----------------------------------------------------------------------


@pytest.mark.parametrize("make", ["create_ieee9", "create_ieee14", "create_eurostag_tutorial_example1_network"])
def test_powsybl_machines_start_from_the_load_flow(make):
    pn = pytest.importorskip("pypowsybl.network")
    from openkeito.powsybl import from_powsybl, imbalance

    net = getattr(pn, make)()
    conv = from_powsybl(net, end_time_s=0.05, generators="machine")
    machines = with_machine_currents(conv)
    assert machines and conv.model["version"] == "0.5"
    result = openkeito.run(conv.model)
    c = conv.compare(result)
    assert c["max_abs_dv_pu"] < 1e-9 and c["max_abs_dva_degree"] < 1e-9, c
    residual = imbalance(net)
    powers = conv.source_powers(result)
    gens = net.get_generators()
    for e in conv.mapping["elements"]:
        if not e["element"].startswith("generator/"):
            continue
        assert e["type"] == "synchronous_machine"
        gid = e["element"].split("/", 1)[1]
        ref = -complex(gens.at[gid, "p"], gens.at[gid, "q"])
        allowed = abs(residual.get(gens.at[gid, "bus_id"], 0j)) + 1e-9 * max(abs(ref), 1.0)
        assert abs(powers[e["component"]] - ref) <= allowed, (gid, powers[e["component"]], ref)
    w_h = 2 * math.pi * conv.mapping["frequency_hz"] * conv.model["simulation"]["time_step_s"]
    assert result.summary["initialization"]["relative_voltage_deviation"] < w_h ** 2
    assert not result.warnings


def test_powsybl_machine_parameters_and_short_circuit_data():
    pn = pytest.importorskip("pypowsybl.network")
    pd = pytest.importorskip("pandas")
    from openkeito.powsybl import from_powsybl

    net = pn.create_eurostag_tutorial_example1_network()
    gid = net.get_generators().index[0]
    rated = 1150.0
    net.update_generators(id=gid, rated_s=rated)
    # Reactances of the extension are in ohm: 0.2 and 0.35 pu of the rating
    # at the nominal voltage.
    vn = float(net.get_voltage_levels().at[net.get_generators().at[gid, "voltage_level_id"], "nominal_v"])
    z_base = vn ** 2 / rated
    net.create_extensions("generatorShortCircuit", pd.DataFrame(
        {"direct_sub_trans_x": [0.2 * z_base], "direct_trans_x": [0.35 * z_base], "step_up_transformer_x": [0.0]},
        index=pd.Index([gid], name="id")))
    conv = from_powsybl(net, generators="machine", machine_parameters={gid: {"inertia_h_s": 5.5}})
    name = next(e["component"] for e in conv.mapping["elements"] if e["element"] == f"generator/{gid}")
    m = next(c for c in conv.model["components"] if c["name"] == name)
    assert m["rated_power_mva"] == rated and m["inertia_h_s"] == 5.5
    assert m["xd_subtransient_pu"] == pytest.approx(0.2) and m["xq_subtransient_pu"] == pytest.approx(0.2)
    assert m["xd_transient_pu"] == pytest.approx(0.35)
    text = "\n".join(conv.warnings)
    assert "typical machine parameters" in text
    result = openkeito.run(conv.model)
    assert conv.compare(result)["max_abs_dv_pu"] < 1e-9
    with pytest.raises(ValueError, match="generators is"):
        from_powsybl(pn.create_ieee9(), generators="vbr")
