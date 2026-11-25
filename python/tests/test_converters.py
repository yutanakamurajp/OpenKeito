"""Static generators, storage and batteries as averaged converters
(``inverter_model="converter"``) against the power flow they come from.

The converter injects the current of its operating point in the phasor
solution, so the steady state of the model is the power flow as with current
injections: voltages within the tolerance of the power flow's own solution
(1e-6 allowed, see test_pandapower.py). In the run the converter then holds
that operating point with its controls. What moves it is the chord of its
source, (w h)^2 / 8 = 3e-5 of its voltage at 50 us, which its current
control takes up: the powers stay within 1e-4 per unit of the power flow.
"""

from __future__ import annotations

import math
import warnings

import pytest

import openkeito

TOL = 1e-6
#: Departure of the powers of a converter from the power flow, per unit of
#: its rating.
HELD = 1e-4


def signals(model, names=("p", "q")):
    """Outputs: the bus voltages already there, and for every converter its
    currents and the signals `names`."""
    converters = [c for c in model["components"] if c["type"] == "grid_following_converter"]
    for c in converters:
        model["outputs"].append({"type": "branch_current", "component": c["name"]})
        model["outputs"] += [{"type": "signal", "signal": f"{c['name']}.{n}"} for n in names]
    return converters


def check_converters_hold_the_power_flow(conv, expected_warnings=()):
    """Runs the model and compares voltages and converter powers with the
    power flow, at the start and over the run."""
    converters = signals(conv.model)
    assert converters
    assert conv.model["version"] == "0.5"
    assert openkeito.validate(conv.model)["valid"]
    result = openkeito.run(conv.model)
    assert [w["code"] for w in result.warnings] == list(expected_warnings), result.warnings
    c = conv.compare(result)
    assert c["max_abs_dv_pu"] < TOL
    assert c["max_abs_dva_degree"] < TOL
    powers = conv.source_powers(result)
    for comp in converters:
        op = comp["operating_point"]
        s = complex(op["active_power_mw"], op["reactive_power_mvar"])
        # The phasor solution: the powers of the power flow.
        assert abs(powers[comp["name"]] - s) < TOL * max(abs(s), 1.0)
        # The run: the measured powers stay there.
        rated = comp["rated_power_mva"]
        for name, value in (("p", s.real), ("q", s.imag)):
            column = result[f"S({comp['name']}.{name})"]
            assert abs(column - value / rated).max() < HELD, (comp["name"], name)
    # The waveforms stay at the steady state, as without converters.
    w_h = 2 * math.pi * conv.mapping["frequency_hz"] * conv.model["simulation"]["time_step_s"]
    assert result.summary["initialization"]["relative_voltage_deviation"] < w_h ** 2
    return result


# pandapower -------------------------------------------------------------------

pp = pytest.importorskip("pandapower")
warnings.filterwarnings("ignore", module="pandapower")

from openkeito.pandapower import from_pandapower  # noqa: E402


def feeder():
    """A 110/20 kV transformer and a 20 kV feeder with a load, a static
    generator with a rating and a storage unit without one."""
    net = pp.create_empty_network(sn_mva=10.0)
    b = [pp.create_bus(net, vn_kv=v) for v in (110.0, 20.0, 20.0, 20.0)]
    pp.create_ext_grid(net, b[0], vm_pu=1.02, s_sc_max_mva=2000.0, rx_max=0.1)
    pp.create_transformer_from_parameters(
        net, b[0], b[1], sn_mva=25.0, vn_hv_kv=110.0, vn_lv_kv=20.0, vk_percent=12.0, vkr_percent=0.4,
        pfe_kw=14.0, i0_percent=0.07, shift_degree=150.0, vector_group="Dyn")
    for k in (1, 2):
        pp.create_line_from_parameters(net, b[k], b[k + 1], length_km=4.0, r_ohm_per_km=0.2, x_ohm_per_km=0.35,
                                       c_nf_per_km=10.0, max_i_ka=0.4)
    pp.create_load(net, b[2], p_mw=4.0, q_mvar=1.0)
    pp.create_sgen(net, b[3], p_mw=3.0, q_mvar=-0.4, sn_mva=4.0, name="PV")
    pp.create_storage(net, b[2], p_mw=1.5, q_mvar=0.3, max_e_mwh=6.0, name="BESS")
    return net


def test_static_generators_and_storage_as_converters_hold_the_power_flow():
    net = feeder()
    conv = from_pandapower(net, inverter_model="converter", end_time_s=0.2)
    types = {e["element"]: e["type"] for e in conv.mapping["elements"]}
    assert types["sgen/0"] == types["storage/0"] == "grid_following_converter"
    by_name = {c["name"]: c for c in conv.model["components"]}
    pv, bess = by_name["sgen0"], by_name["storage0"]
    # The rating of the network, the rated voltage of the bus, and the
    # powers of the power flow as references (storage: charging is negative).
    assert (pv["rated_power_mva"], pv["rated_voltage_kv"], pv["rated_frequency_hz"]) == (4.0, 20.0, 50.0)
    assert pv["reference"] == {"type": "power", "active_power_mw": pytest.approx(3.0),
                               "reactive_power_mvar": pytest.approx(-0.4)}
    assert bess["reference"]["active_power_mw"] == pytest.approx(-1.5)
    assert bess["reference"]["reactive_power_mvar"] == pytest.approx(-0.3)
    # The operating point is the bus voltage of the power flow.
    vm, va = net.res_bus.vm_pu[3], net.res_bus.va_degree[3]
    assert pv["operating_point"]["voltage_kv"] == pytest.approx(20.0 * vm)
    assert pv["operating_point"]["angle_deg"] == pytest.approx(va)
    # Without sn_mva the rating is assumed and said.
    assert bess["rated_power_mva"] == pytest.approx(abs(complex(1.5, 0.3)) / 0.9)
    assert [w for w in conv.warnings if w.startswith("no rated power")] == [
        "no rated power (sn_mva, max_p); the converter is rated at its apparent power of the power flow / 0.9: "
        "storage/0"]
    check_converters_hold_the_power_flow(conv)


def test_the_default_is_the_current_injection_as_before():
    conv = from_pandapower(feeder())
    types = {e["element"]: e["type"] for e in conv.mapping["elements"]}
    assert types["sgen/0"] == types["storage/0"] == "three_phase_current_source"
    assert conv.model["version"] == "0.4"
    assert not any("converter" in w for w in conv.warnings)


def test_a_converter_answers_a_fault_with_its_current_limit():
    # A three-phase fault at the bus of the static generator. The current
    # injection keeps its current; the converter raises it to its limit of
    # 1.2 per unit (active current first), and no further.
    net = feeder()
    conv = from_pandapower(net, inverter_model="converter", end_time_s=0.5,
                           converter_parameters={"current_limit_pu": 1.2, "priority": "active"})
    pv = next(c for c in conv.model["components"] if c["name"] == "sgen0")
    assert (pv["current_limit_pu"], pv["priority"]) == (1.2, "active")
    conv.model["components"].append(
        {"type": "fault", "name": "F1", "bus": conv.bus(3), "phases": ["a", "b", "c"], "ground": True,
         "fault_resistance_ohm": 2.0, "start_time_s": 0.1, "clear_time_s": 0.2})
    conv.model["outputs"] += [{"type": "signal", "signal": f"sgen0.{n}"} for n in ("i_d", "i_q", "i_ref.d", "v_d", "p")]
    result = openkeito.run(conv.model)
    t = result.time
    during = (t > 0.16) & (t < 0.2)
    i_d, i_q = result["S(sgen0.i_d)"], result["S(sgen0.i_q)"]
    assert result["S(sgen0.v_d)"][during].max() < 0.6
    assert result["S(sgen0.i_ref.d)"][during] == pytest.approx(1.2)
    assert i_d[during] == pytest.approx(1.2, abs=5e-3)
    assert (i_d ** 2 + i_q ** 2).max() ** 0.5 < 1.2 + 0.07
    # Back at the power of the power flow, 0.3 s after the fault: what the
    # integrals of the current control still lack decays with L / R = 95 ms.
    assert result["S(sgen0.p)"][-1] == pytest.approx(3.0 / 4.0, abs=2e-3)


def test_converter_options_are_checked_and_an_overloaded_converter_is_named():
    with pytest.raises(ValueError, match="inverter_model is 'inverter'"):
        from_pandapower(feeder(), inverter_model="inverter")
    with pytest.raises(ValueError, match="converter_parameters cannot set bus, reference"):
        from_pandapower(feeder(), inverter_model="converter",
                        converter_parameters={"reference": {}, "bus": "x"})
    # 3 MW from a rating of 2 MVA is 1.5 per unit of current.
    net = feeder()
    net.sgen.at[0, "sn_mva"] = 2.0
    conv = from_pandapower(net, inverter_model="converter")
    assert any(w.startswith("the current of the power flow is above the current limit of the converter (1.1 pu")
               and "sgen/0 (1.5" in w for w in conv.warnings), conv.warnings


def test_machines_and_converters_together_start_from_the_power_flow_and_stay_there():
    # case9 with a static generator: the generators as synchronous machines
    # and the static generator as a converter, in one conversion.
    pn = pytest.importorskip("pandapower.networks")
    net = pn.case9()
    pp.create_sgen(net, 4, p_mw=40.0, q_mvar=10.0, sn_mva=50.0, name="PV")
    conv = from_pandapower(net, generators="machine", machine_parameters={"inertia_h_s": 5.0},
                           inverter_model="converter", converter_parameters={"current_limit_pu": 1.2},
                           end_time_s=0.5)
    types = {e["element"]: e["type"] for e in conv.mapping["elements"]}
    assert types["sgen/0"] == "grid_following_converter"
    assert [types[f"gen/{k}"] for k in net.gen.index] == ["synchronous_machine"] * len(net.gen)
    assert types["ext_grid/0"] == "three_phase_source"
    by_name = {c["name"]: c for c in conv.model["components"]}
    assert by_name["sgen0"]["current_limit_pu"] == 1.2
    machines = [f"gen{k}" for k in net.gen.index]
    assert all(by_name[m]["inertia_h_s"] == 5.0 for m in machines)
    # The speeds of the machines, next to what the converters are checked by.
    conv.model["controls"] = [
        {"type": "measurement", "name": f"speed_{m}",
         "quantity": {"type": "component_signal", "component": m, "signal": "speed_pu"}} for m in machines]
    conv.model["outputs"] += [{"type": "signal", "signal": f"speed_{m}"} for m in machines]
    conv.model["outputs"] += [{"type": "branch_current", "component": m} for m in machines]
    # W183: the model has control blocks of the caller (the measurements),
    # which the phasor solution does not cover.
    result = check_converters_hold_the_power_flow(conv, expected_warnings=["W183"])
    # The machines deliver the powers of the power flow, and keep their speed.
    powers = conv.source_powers(result)
    for k in net.gen.index:
        ref = complex(net.res_gen.at[k, "p_mw"], net.res_gen.at[k, "q_mvar"])
        assert abs(powers[f"gen{k}"] - ref) < TOL * max(abs(ref), 1.0)
        speed = result[f"S(speed_gen{k})"]
        assert abs(speed - 1.0).max() < 1e-6
        # No drift: the second half of the run is where the first is.
        half = len(speed) // 2
        assert abs(speed[half:].mean() - speed[:half].mean()) < 1e-7
    # The converter: the 3e-5 that the chord of its source takes at the start
    # decays (L / R = 95 ms); the end of the run is at the power flow.
    for name, value in (("p", 40.0 / 50.0), ("q", 10.0 / 50.0)):
        column = result[f"S(sgen0.{name})"]
        assert abs(column[-len(column) // 10:] - value).max() < 2e-6, name


# PowSyBl ----------------------------------------------------------------------

pypowsybl = pytest.importorskip("pypowsybl")

from openkeito.powsybl import from_powsybl  # noqa: E402
from openkeito.convert import ConversionError  # noqa: E402


def with_batteries():
    """IEEE 14 with a battery that discharges 10 MW + j 2 Mvar (limits of
    20 MW) and one that charges 5 MW, whose limits are the placeholder 9999."""
    network = pypowsybl.network.create_ieee14()
    network.create_batteries(id="BAT1", voltage_level_id="VL9", bus_id="B9", max_p=20.0, min_p=-20.0,
                             target_p=10.0, target_q=2.0)
    network.create_batteries(id="BAT2", voltage_level_id="VL13", bus_id="B13", max_p=9999.0, min_p=-9999.0,
                             target_p=-5.0, target_q=0.0)
    return network


def test_batteries_as_converters_hold_the_load_flow():
    network = with_batteries()
    conv = from_powsybl(network, inverter_model="converter", end_time_s=0.2)
    component = {e["element"]: e["component"] for e in conv.mapping["elements"]}
    by_name = {c["name"]: c for c in conv.model["components"]}
    first, second = by_name[component["battery/BAT1"]], by_name[component["battery/BAT2"]]
    assert first["type"] == second["type"] == "grid_following_converter"
    # IIDM gives the terminal powers with the load sign; the converter
    # injects their negative.
    batteries = network.get_batteries(all_attributes=True)
    assert first["reference"]["active_power_mw"] == pytest.approx(-batteries.at["BAT1", "p"])
    assert first["reference"]["active_power_mw"] == pytest.approx(10.0, abs=1e-6)
    assert first["reference"]["reactive_power_mvar"] == pytest.approx(2.0, abs=1e-6)
    assert second["reference"]["active_power_mw"] == pytest.approx(-5.0, abs=1e-6)
    assert (first["rated_power_mva"], first["rated_voltage_kv"]) == (20.0, 12.0)
    assert second["rated_power_mva"] == pytest.approx(5.0 / 0.9, rel=1e-6)
    assert any(w.startswith("no rated power") and w.endswith(": BAT2") for w in conv.warnings), conv.warnings
    check_converters_hold_the_power_flow(conv)
    # The default keeps them as they were: a current injection and a load.
    default = from_powsybl(with_batteries())
    assert default.model["version"] == "0.4"
    kinds = {c["type"] for c in default.model["components"]}
    assert "grid_following_converter" not in kinds


def test_chosen_generators_as_converters_hold_the_load_flow():
    network = pypowsybl.network.create_ieee14()
    chosen = ["B2-G", "B3-G"]
    conv = from_powsybl(network, converter_generators=chosen, end_time_s=0.2)
    kinds = {e["element"]: e["type"] for e in conv.mapping["elements"] if e["element"].startswith("generator/")}
    assert {k for k, v in kinds.items() if v == "grid_following_converter"} == {f"generator/{g}" for g in chosen}
    assert sum(v == "three_phase_source" for v in kinds.values()) == len(kinds) - 2
    check_converters_hold_the_power_flow(conv)
    with pytest.raises(ConversionError, match="converter_generators names no generator.*: NONE"):
        from_powsybl(pypowsybl.network.create_ieee14(), converter_generators=["NONE"])


def test_generators_as_machines_beside_generators_as_converters():
    # Those named in converter_generators are converters; the others follow
    # `generators`.
    network = pypowsybl.network.create_ieee14()
    conv = from_powsybl(network, generators="machine", converter_generators=["B3-G"], end_time_s=0.2)
    kinds = {e["element"]: e["type"] for e in conv.mapping["elements"] if e["element"].startswith("generator/")}
    assert kinds.pop("generator/B3-G") == "grid_following_converter"
    assert len(kinds) == 4 and set(kinds.values()) == {"synchronous_machine"}
    assert conv.model["version"] == "0.5"
    result = check_converters_hold_the_power_flow(conv)
    init = result.summary["initialization"]
    w_h = 2 * math.pi * conv.mapping["frequency_hz"] * conv.model["simulation"]["time_step_s"]
    assert init["relative_current_deviation"] < w_h ** 2
