"""pandapower networks and schedules against pandapower's own power flow.

The converter makes the phasor solution of the EMT model equal to the power
flow (README, "From a power flow"), and the phasor solve is exact up to
rounding. What is left is the convergence tolerance of pandapower's Newton
method (tolerance_mva = 1e-8 on a base of sn_mva): differences of about
1e-10 pu in voltage and 1e-8 relative in branch power. The tests allow 1e-6,
far inside the 0.1 % of plan.md 6.2.
"""

from __future__ import annotations

import math
import warnings
from datetime import datetime, timedelta, timezone

import numpy as np
import pytest

pp = pytest.importorskip("pandapower")
pn = pytest.importorskip("pandapower.networks")

import openkeito  # noqa: E402
from openkeito.pandapower import (  # noqa: E402
    ConversionError,
    apply_schedule,
    branch_powers,
    check_schedule,
    fault,
    from_pandapower,
    run_time_slices,
    schedule_from_opf,
    time_slices,
)
from openkeito.schedule import Schedule, parse_time  # noqa: E402

warnings.filterwarnings("ignore", module="pandapower")

TOL = 1e-6
JST = timezone(timedelta(hours=9))


def worst_branch_difference(net, conv, result) -> float:
    """Largest |S_emt - S_pf| / max(|S_pf|, 1 MVA) over every line and
    transformer end."""
    powers = branch_powers(conv, result)
    worst = 0.0
    for e in conv.mapping["elements"]:
        table, idx = e["element"].split("/")
        if table not in ("line", "trafo"):
            continue
        r = net[f"res_{table}"].loc[int(idx)]
        ends = (("p_from_mw", "q_from_mvar"), ("p_to_mw", "q_to_mvar")) if table == "line" else \
            (("p_hv_mw", "q_hv_mvar"), ("p_lv_mw", "q_lv_mvar"))
        for k, (p, q) in enumerate(ends):
            ref = complex(r[p], r[q])
            worst = max(worst, abs(powers[e["component"]][k] - ref) / max(abs(ref), 1.0))
    return worst


def small_network():
    """Every supported element: a 110/20 kV transformer with a tap and a
    150-degree shift, a double line, closed bus-bus switches with and without
    impedance, an open line switch, a load, storage, a static generator, a
    shunt and a generator."""
    net = pp.create_empty_network(f_hz=50.0)
    b = [pp.create_bus(net, 110.0)] + [pp.create_bus(net, 20.0) for _ in range(5)]
    pp.create_ext_grid(net, b[0], vm_pu=1.02, va_degree=5.0, s_sc_max_mva=3000.0, rx_max=0.1)
    pp.create_transformer_from_parameters(
        net, b[0], b[1], sn_mva=40, vn_hv_kv=110, vn_lv_kv=20, vkr_percent=0.5, vk_percent=10, pfe_kw=30,
        i0_percent=0.1, shift_degree=150, vector_group="Dyn", tap_side="hv", tap_neutral=0, tap_min=-9,
        tap_max=9, tap_step_percent=2.5, tap_pos=2, tap_changer_type="Ratio")
    pp.create_line_from_parameters(net, b[1], b[2], length_km=5, r_ohm_per_km=0.1, x_ohm_per_km=0.3,
                                   c_nf_per_km=200, max_i_ka=1, parallel=2)
    pp.create_switch(net, b[2], b[3], et="b", closed=True)
    pp.create_switch(net, b[3], b[4], et="b", closed=True, z_ohm=0.5)
    spur = pp.create_line_from_parameters(net, b[4], b[5], length_km=3, r_ohm_per_km=0.2, x_ohm_per_km=0.35,
                                          c_nf_per_km=250, max_i_ka=1)
    pp.create_line_from_parameters(net, b[1], b[5], length_km=4, r_ohm_per_km=0.2, x_ohm_per_km=0.35,
                                   c_nf_per_km=250, max_i_ka=1)
    pp.create_switch(net, b[5], spur, et="l", closed=False)
    pp.create_load(net, b[2], p_mw=10, q_mvar=3)
    pp.create_storage(net, b[3], p_mw=2, max_e_mwh=10, q_mvar=0.5)
    pp.create_sgen(net, b[4], p_mw=1, q_mvar=-0.2)
    pp.create_shunt(net, b[4], q_mvar=-2.0, p_mw=0.01)
    pp.create_gen(net, b[4], p_mw=3, vm_pu=0.99, sn_mva=5.0)
    pp.create_load(net, b[5], p_mw=1.5, q_mvar=0.4)
    return net


@pytest.mark.parametrize("make", [small_network, pn.case9, pn.case39, pn.case118],
                         ids=["small", "case9", "case39", "case118"])
def test_the_steady_state_is_the_power_flow(make):
    net = make()
    conv = from_pandapower(net, end_time_s=0.02, outputs="all")
    result = openkeito.run(conv.model)
    c = conv.compare(result)
    assert c["max_abs_dv_pu"] < TOL
    assert c["max_abs_dva_degree"] < TOL
    assert worst_branch_difference(net, conv, result) < TOL
    # The waveforms stay at the steady state: their departure is the
    # discretization error, below (w h)^2.
    w_h = 2 * math.pi * conv.mapping["frequency_hz"] * conv.model["simulation"]["time_step_s"]
    assert result.summary["initialization"]["relative_voltage_deviation"] < w_h ** 2
    assert not result.warnings


def test_mapping_and_warnings():
    net = small_network()
    conv = from_pandapower(net)
    buses = conv.mapping["buses"]
    # Buses 2 and 3 are joined by a closed switch without impedance.
    assert buses["2"] == buses["3"] == "b2"
    assert conv.bus(4) == "b4"
    elements = {e["element"]: e for e in conv.mapping["elements"]}
    assert elements["trafo/0"]["type"] == "three_phase_transformer"
    assert elements["switch/1"]["type"] == "coupled_rl"
    assert elements["storage/0"]["type"] == "three_phase_current_source"
    trafo = next(c for c in conv.model["components"] if c["name"] == "trafo0")
    assert trafo["vector_group"] == "Dyn5"
    assert trafo["rated_voltage_kv"] == pytest.approx([115.5, 20.0])
    # The open switch leaves the spur line's end on a node of its own.
    spur = next(c for c in conv.model["components"] if c["name"] == "line1")
    assert spur["buses"] == ["b4", "sw2"]
    assert any(w.startswith("no zero-sequence data") for w in conv.warnings)
    assert openkeito.validate(conv.model)["valid"]


def test_what_cannot_be_converted_is_listed():
    net = pn.case9()
    pp.create_transformer3w_from_parameters(
        net, 0, 1, 2, vn_hv_kv=345, vn_mv_kv=345, vn_lv_kv=345, sn_hv_mva=100, sn_mv_mva=100, sn_lv_mva=100,
        vk_hv_percent=10, vk_mv_percent=10, vk_lv_percent=10, vkr_hv_percent=0.3, vkr_mv_percent=0.3,
        vkr_lv_percent=0.3, pfe_kw=0, i0_percent=0)
    with pytest.raises(ConversionError) as e:
        from_pandapower(net)
    assert any(p.startswith("trafo3w") for p in e.value.problems)

    net = small_network()
    net.trafo.at[0, "shift_degree"] = 15.0
    with pytest.raises(ConversionError) as e:
        from_pandapower(net)
    assert "not a multiple of 30" in e.value.problems[0]


def day_inputs(net, hours=24):
    times = [datetime(2026, 11, 21, h, tzinfo=JST) for h in range(hours)]
    shape = [0.8 - 0.2 * math.cos(2 * math.pi * (h - 3) / 24) for h in range(hours)]
    values = {}
    for i in net.load.index:
        values[(f"load/{i}", "active_power_mw")] = [net.load.p_mw[i] * f for f in shape]
        values[(f"load/{i}", "reactive_power_mvar")] = [net.load.q_mvar[i] * f for f in shape]
    return Schedule.from_values(times, values, name="day")


def test_every_hour_of_an_opf_schedule_starts_from_its_power_flow():
    net = pn.case9()
    plan, notes = schedule_from_opf(net, day_inputs(net), opf="ac")
    assert notes == []
    quantities = {(s["target"], s["quantity"]) for s in plan.series}
    assert {("gen/0", "active_power_mw"), ("gen/1", "voltage_pu"), ("ext_grid/0", "voltage_pu")} <= quantities
    run = run_time_slices(net, plan, end_time_s=0.02)
    assert len(run.rows) == 24
    for row in run.rows:
        assert row["status"] == "completed"
        assert row["max_abs_dv_pu"] < TOL and row["max_abs_dva_degree"] < TOL, row
    # The power flow of a slice is the OPF's: the slack gives what it planned.
    hour = plan.index(plan.times[12])
    p_gen = {s["target"]: s["values"][hour] for s in plan.series if s["quantity"] == "active_power_mw"}
    assert p_gen["gen/0"] > 0


def test_a_dc_opf_schedule_warns_about_the_slack():
    net = pn.case9()
    plan, notes = schedule_from_opf(net, day_inputs(net, hours=3), opf="dc", slack_tolerance=0.001)
    assert notes[0].startswith("DC optimal power flow")
    assert "slack moved" in notes[1]
    assert not any(s["quantity"] == "voltage_pu" for s in plan.series)


def test_time_slices_in_parallel_and_in_sequence_are_the_same():
    net = pn.case39()
    plan = day_inputs(net, hours=4)
    disturbances = [fault(16, 0.01, 0.03)]
    a = run_time_slices(net, plan, disturbances, end_time_s=0.04, jobs=1, keep_rows=True)
    b = run_time_slices(net, plan, disturbances, end_time_s=0.04, jobs=4, keep_rows=True)
    assert [r["case"] for r in a.rows] == ["t000.fault_b16_abc", "t001.fault_b16_abc",
                                           "t002.fault_b16_abc", "t003.fault_b16_abc"]
    for s in a.slices:
        x, y = a.batch[s.name], b.batch[s.name]
        assert x.columns == y.columns and np.array_equal(x.time, y.time)
        assert all(np.array_equal(x[c], y[c]) for c in x.columns)
        # The fault pulls the faulted bus to ground.
        v = a.batch[s.name]["V(b16.a)"]
        t = a.batch[s.name].time
        during = (t > 0.015) & (t < 0.025)
        assert np.max(np.abs(v[during])) < 0.01 * np.max(np.abs(v[t < 0.01]))


def test_schedules_are_checked():
    good = day_inputs(pn.case9(), hours=2)
    bad = dict(good.data)
    bad["times"] = ["2026-11-21T00:00:00", good.times[1]]
    with pytest.raises(openkeito.OpenKeitoError) as e:
        Schedule.load(bad)
    assert e.value.diagnostics[0]["code"] == "E502"

    unknown = Schedule.from_values(good.times, {("gen/7", "active_power_mw"): [1.0, 2.0],
                                                ("line/0", "voltage_pu"): [1.0, 1.0]})
    with pytest.raises(ConversionError) as e:
        check_schedule(pn.case9(), unknown)
    assert len(e.value.problems) == 2


def test_schedule_interpolation_and_frames():
    times = ["2026-11-21T00:00:00+09:00", "2026-11-21T01:00:00+09:00"]
    s = Schedule.from_values(times, {("load/0", "active_power_mw"): [10.0, 20.0],
                                     ("trafo/0", "tap_position"): [0.0, 2.0]}, interpolation="linear")
    half = parse_time("2026-11-20T15:30:00Z")
    assert s.values_at(half) == {("load/0", "active_power_mw"): 15.0, ("trafo/0", "tap_position"): 0.0}
    with pytest.raises(ValueError):
        s.values_at("2026-11-21T02:00:00+09:00")
    pytest.importorskip("pandas")
    frame = s.to_frame()
    assert list(frame.columns) == ["load/0:active_power_mw", "trafo/0:tap_position"]
    assert Schedule.from_frame(frame, interpolation="linear").data["series"] == s.data["series"]


# What the power flow does with taps, open ends and options ---------------------

#: These cases meet the power flow to its own tolerance.
TIGHT = 1e-8


def add_transformer(net, hv, lv, **changes):
    values = dict(sn_mva=40, vn_hv_kv=110, vn_lv_kv=20, vkr_percent=0.5, vk_percent=10, pfe_kw=30, i0_percent=0.1,
                  shift_degree=150, vector_group="Dyn", tap_side="hv", tap_neutral=0, tap_min=-9, tap_max=9,
                  tap_step_percent=2.5, tap_pos=0, tap_changer_type="Ratio")
    return pp.create_transformer_from_parameters(net, hv, lv, **{**values, **changes})


def radial(**trafo):
    """A 110/20 kV transformer (Dyn5, tap 2 of 2.5 % at the high-voltage
    side), two lines in a row and a load at the end of each."""
    net = pp.create_empty_network(f_hz=50.0)
    b = [pp.create_bus(net, 110.0)] + [pp.create_bus(net, 20.0) for _ in range(3)]
    pp.create_ext_grid(net, b[0], vm_pu=1.02, s_sc_max_mva=3000.0, rx_max=0.1)
    add_transformer(net, b[0], b[1], **{"tap_pos": 2, **trafo})
    for k in (1, 2):
        pp.create_line_from_parameters(net, b[k], b[k + 1], length_km=5, r_ohm_per_km=0.1, x_ohm_per_km=0.3,
                                       c_nf_per_km=200, max_i_ka=1)
    pp.create_load(net, b[2], p_mw=10, q_mvar=3)
    pp.create_load(net, b[3], p_mw=2, q_mvar=1)
    return net


def steady_state(net, **options):
    """(conversion, result, largest voltage difference in pu or degrees)."""
    conv = from_pandapower(net, end_time_s=0.02, outputs="all", **options)
    result = openkeito.run(conv.model)
    c = conv.compare(result)
    return conv, result, max(c["max_abs_dv_pu"], c["max_abs_dva_degree"])


def converted(conv):
    return {e["element"] for e in conv.mapping["elements"]}


@pytest.mark.parametrize("change, ignored", [
    ({"tap_changer_type": None}, True), ({"tap_changer_type": float("nan")}, True),
    ({"tap_changer_type": "Tabular"}, True), ({"tap_side": None}, True),
    ({"tap_changer_type": "Symmetrical"}, False), ({"tap_pos": float("nan")}, False), ({"tap_side": "lv"}, False),
], ids=str)
def test_a_tap_counts_when_it_counts_in_pandapower(change, ignored):
    net = radial()
    net.trafo["tap_changer_type"] = net.trafo["tap_changer_type"].astype(object)
    for column, value in change.items():
        net.trafo.at[0, column] = value
    conv, _, worst = steady_state(net)
    assert worst < TIGHT
    # pandapower leaves a tap without a type or a side out; the converter
    # does the same and says so.
    assert any(w.startswith("tap position") and "trafo/0" in w for w in conv.warnings) == ignored


def test_taps_of_networks_from_before_pandapower_3_and_second_taps():
    net = radial()
    del net.trafo["tap_changer_type"]
    conv, _, worst = steady_state(net)
    assert worst < TIGHT and any(w.startswith("tap position") for w in conv.warnings)

    net = radial()
    for column, value in {"tap2_pos": 1.0, "tap2_side": "lv", "tap2_neutral": 0.0, "tap2_step_percent": 1.0,
                          "tap2_changer_type": "Ratio", "tap2_step_degree": 0.0}.items():
        net.trafo[column] = value
    conv, _, worst = steady_state(net)
    assert worst < TIGHT
    trafo = next(c for c in conv.model["components"] if c["name"] == "trafo0")
    assert trafo["rated_voltage_kv"] == pytest.approx([115.5, 20.2])


def test_taps_that_shift_the_phase_are_refused():
    net = radial(tap_changer_type="Ideal", tap_step_percent=float("nan"), tap_step_degree=1.5)
    with pytest.raises(ConversionError) as e:
        from_pandapower(net)
    assert "not a multiple of 30" in e.value.problems[0]
    with pytest.raises(ConversionError):
        from_pandapower(radial(tap_step_degree=30.0))
    # At the neutral position an ideal phase shifter is a plain transformer.
    assert steady_state(radial(tap_changer_type="Ideal", tap_pos=0))[2] < TIGHT


def test_a_line_at_an_out_of_service_bus_stays_energized():
    net = radial()
    net.bus.at[3, "in_service"] = False
    conv, result, worst = steady_state(net)
    assert worst < TIGHT
    line = next(c for c in conv.model["components"] if c["name"] == "line1")
    assert line["buses"] == ["b2", "line1_open"]
    # Its charging power is that of pandapower's result.
    s = branch_powers(conv, result)["line1"][0]
    assert s.imag == pytest.approx(net.res_line.q_from_mvar[1], abs=1e-8) and s.imag < -0.1
    # With the other end open as well, the line is out.
    pp.create_switch(net, 2, 1, et="l", closed=False)
    conv, _, worst = steady_state(net)
    assert worst < TIGHT and "line/1" not in converted(conv)
    # A transformer at an out-of-service bus is out in pandapower.
    net = radial()
    add_transformer(net, 0, pp.create_bus(net, 20.0, in_service=False))
    conv, _, worst = steady_state(net)
    assert worst < TIGHT and "trafo/1" not in converted(conv)


@pytest.mark.parametrize("open_at", [[0], [1], [0, 1]], ids=["hv", "lv", "both"])
@pytest.mark.parametrize("group", ["Dyn", "YNd", "YNyn"])
def test_a_transformer_with_an_open_switch(open_at, group):
    net = radial()
    shift = 0 if group == "YNyn" else 150
    net.trafo.at[0, "vector_group"] = group
    net.trafo.at[0, "shift_degree"] = shift
    second = add_transformer(net, 0, 1, vector_group=group, shift_degree=shift)
    for bus in open_at:
        pp.create_switch(net, bus, second, et="t", closed=False)
    conv, result, worst = steady_state(net)
    assert worst < TIGHT and not result.warnings
    if len(open_at) == 2:
        assert "trafo/1" not in converted(conv)
        return
    # Energized from the closed side: the first transformer carries the
    # no-load power of the second, as in the power flow.
    assert "trafo/1" in converted(conv)
    side = "lv" if open_at == [0] else "hv"
    assert net.res_trafo[f"p_{side}_mw"][second] > 0.02
    s = branch_powers(conv, result)["trafo0"]
    for k, end in enumerate(("hv", "lv")):
        ref = complex(net.res_trafo[f"p_{end}_mw"][0], net.res_trafo[f"q_{end}_mvar"][0])
        assert abs(s[k] - ref) < TIGHT * abs(ref)


def test_open_switches_take_branches_out_when_the_power_flow_says_so():
    net = radial()
    second = add_transformer(net, 0, 1)
    pp.create_switch(net, 0, second, et="t", closed=False)
    pp.create_switch(net, 3, 1, et="l", closed=False)
    conv, _, worst = steady_state(net, power_flow_options={"neglect_open_switch_branches": True})
    assert worst < TIGHT
    assert not {"trafo/1", "line/1"} & converted(conv)


@pytest.mark.parametrize("c_nf_per_km", [200.0, 0.0])
def test_the_shunt_conductance_of_lines(c_nf_per_km):
    net = radial()
    net.line["c_nf_per_km"] = c_nf_per_km
    net.line["g_us_per_km"] = [50.0, 80.0]
    net.line.at[0, "parallel"] = 2
    pp.create_switch(net, 3, 1, et="l", closed=False)
    conv, result, worst = steady_state(net)
    assert worst < TIGHT
    assert not any("g_us_per_km" in w for w in conv.warnings)
    assert net.res_line.pl_mw[1] > 0.05
    # Each line takes the power of the power flow: what flows into the
    # series part and what its conductance at that end takes.
    powers = branch_powers(conv, result)
    for k in (0, 1):
        ref = complex(net.res_line.p_from_mw[k], net.res_line.q_from_mvar[k])
        shunt = next(c for c in conv.model["components"] if c["name"] == f"line{k}_g0")
        p_shunt = shunt["active_power_mw"] * (net.res_bus.vm_pu[k + 1] * 20 / shunt["rated_voltage_kv"]) ** 2
        assert abs(powers[f"line{k}"][0] + p_shunt - ref) < TIGHT * max(abs(ref), 1.0)


def test_every_table_the_converter_does_not_read_is_refused():
    import pandas as pd

    from openkeito.pandapower import UNSUPPORTED_TABLES

    read = {"bus", "load", "sgen", "storage", "gen", "switch", "shunt", "ext_grid", "line", "trafo"}
    # Not part of the power flow.
    other = {"measurement", "pwl_cost", "poly_cost", "controller", "group"}
    empty = pp.create_empty_network()
    tables = {k for k in empty.keys()
              if isinstance(empty[k], pd.DataFrame) and not k.startswith(("res_", "_")) and len(empty[k].columns)}
    assert tables - read - other <= set(UNSUPPORTED_TABLES)
    for table in ("vsc_stacked", "vsc_bipolar", "line_dc", "bus_dc", "source_dc", "load_dc"):
        net = pn.case9()
        net[table] = pd.concat([net[table], pd.DataFrame({"in_service": [True]})])
        with pytest.raises(ConversionError) as e:
            from_pandapower(net)
        assert e.value.problems[0].startswith(table)


def test_power_flow_options_and_a_power_flow_of_the_caller():
    net = pn.case9()
    from_pandapower(net, power_flow_options={"tolerance_mva": 1e-10, "max_iteration": 50})
    assert net._options["tolerance_mva"] == 1e-10 and net._options["max_iteration"] == 50
    # run_power_flow=False leaves the caller's results alone.
    pp.runpp(net, init="flat")
    before = net.res_bus.copy()
    _, _, worst = steady_state(net, run_power_flow=False)
    assert before.equals(net.res_bus) and worst < TIGHT
    # A power flow without the transformers' phase shifts, or with another
    # transformer model, is not the model's.
    net = radial()
    pp.runpp(net, calculate_voltage_angles=False)
    with pytest.raises(ConversionError) as e:
        from_pandapower(net, run_power_flow=False)
    assert "calculate_voltage_angles" in e.value.problems[0]
    with pytest.raises(ConversionError) as e:
        from_pandapower(radial(), power_flow_options={"trafo_model": "pi"})
    assert "trafo_model" in e.value.problems[0]


def test_a_power_flow_that_does_not_converge_names_its_time():
    plan = Schedule.from_values(["2026-01-01T00:00:00Z", "2026-01-01T01:00:00Z"],
                                {("load/0", "active_power_mw"): [80.0, 90000.0]})
    with pytest.raises(ConversionError) as e:
        run_time_slices(pn.case9(), plan, end_time_s=0.02)
    assert e.value.problems[0].startswith("2026-01-01T01:00:00Z: ")
    assert "did not converge" in e.value.problems[0]


def test_faults_need_a_bus_of_the_network_and_names_of_their_own():
    net = pn.case9()
    plan = Schedule.from_values(["2026-01-01T00:00:00Z"], {("load/0", "active_power_mw"): [80.0]})
    with pytest.raises(ConversionError) as e:
        time_slices(net, plan, [fault(99, 0.01, 0.02)])
    assert "99" in e.value.problems[0]
    with pytest.raises(ConversionError) as e:
        time_slices(net, plan, [fault(4, 0.005, 0.02), fault(4, 0.015, 0.025)])
    assert "fault_b4_abc" in e.value.problems[0] and "name" in e.value.problems[0]
    slices = time_slices(net, plan, [fault(np.int64(4), 0.005, 0.02), fault(4, 0.015, 0.025, name="late")])
    assert [s.name for s in slices] == ["t000.fault_b4_abc", "t000.late"]
    assert slices[0].model["components"][-1]["bus"] == "b4"


def test_a_scheduled_power_is_the_power_of_the_power_flow():
    net = radial()
    net.load.at[0, "scaling"] = 0.5
    pp.create_sgen(net, 3, p_mw=1.0, q_mvar=0.1, scaling=2.0)
    pp.create_storage(net, 3, p_mw=0.5, max_e_mwh=1.0, scaling=4.0)
    pp.create_gen(net, 2, p_mw=1.0, vm_pu=1.0, scaling=0.25)
    pp.create_shunt(net, 3, q_mvar=-0.5, step=2)
    time = "2026-01-01T00:00:00Z"
    plan = Schedule.from_values([time], {
        ("load/0", "active_power_mw"): [8.0], ("load/0", "reactive_power_mvar"): [2.0],
        ("sgen/0", "active_power_mw"): [3.0], ("sgen/0", "reactive_power_mvar"): [-1.0],
        ("storage/0", "active_power_mw"): [1.0], ("gen/0", "active_power_mw"): [2.0],
        ("shunt/0", "reactive_power_mvar"): [-1.0],
    })
    n = apply_schedule(net, plan, time)
    pp.runpp(n)
    assert (n.res_load.p_mw[0], n.res_load.q_mvar[0]) == pytest.approx((8.0, 2.0))
    assert (n.res_sgen.p_mw[0], n.res_sgen.q_mvar[0]) == pytest.approx((3.0, -1.0))
    assert n.res_storage.p_mw[0] == pytest.approx(1.0)
    assert n.res_gen.p_mw[0] == pytest.approx(2.0)
    # A shunt's power holds at its rated voltage.
    assert n.res_shunt.q_mvar[0] == pytest.approx(-1.0 * n.res_bus.vm_pu[3] ** 2)
    # The network itself is not changed.
    assert net.load.p_mw[0] == 10 and net.load.scaling[0] == 0.5

    net.load.at[0, "scaling"] = 0.0
    with pytest.raises(ConversionError) as e:
        apply_schedule(net, plan, time)
    assert "load/0" in e.value.problems[0] and "scaling" in e.value.problems[0]


def test_an_ac_opf_schedule_reproduces_the_opf():
    net = pn.case9()
    pp.create_sgen(net, 4, p_mw=20, q_mvar=0.0, controllable=True, min_p_mw=0, max_p_mw=50, min_q_mvar=-30,
                   max_q_mvar=30)
    pp.create_poly_cost(net, 0, "sgen", cp1_eur_per_mw=1.0)
    inputs = Schedule.from_values(["2026-01-01T00:00:00Z"], {("load/0", "active_power_mw"): [80.0]})
    plan, _ = schedule_from_opf(net, inputs)
    assert {("sgen/0", "active_power_mw"), ("sgen/0", "reactive_power_mvar")} <= \
        {(s["target"], s["quantity"]) for s in plan.series}
    opf = apply_schedule(net, inputs, inputs.times[0])
    pp.runopp(opf)
    assert abs(opf.res_sgen.q_mvar[0]) > 1.0
    pf = apply_schedule(net, plan, inputs.times[0])
    pp.runpp(pf)
    # The power flow of the slice is the OPF's, to the tolerance of its
    # interior-point solver.
    assert pf.res_sgen.q_mvar[0] == pytest.approx(opf.res_sgen.q_mvar[0], abs=1e-9)
    assert np.allclose(pf.res_bus.vm_pu, opf.res_bus.vm_pu, atol=1e-5)
    assert np.allclose(pf.res_gen.q_mvar, opf.res_gen.q_mvar, atol=1e-2)
    assert pf.res_ext_grid.q_mvar[0] == pytest.approx(opf.res_ext_grid.q_mvar[0], abs=1e-2)


def test_an_opf_schedule_with_elements_out_of_service():
    times = ["2026-01-01T00:00:00Z", "2026-01-01T01:00:00Z"]
    net = pn.case9()
    net.gen.at[1, "in_service"] = False
    plan, _ = schedule_from_opf(net, Schedule.from_values(times, {("load/0", "active_power_mw"): [80.0, 85.0]}))
    targets = {s["target"] for s in plan.series}
    assert "gen/0" in targets and "gen/1" not in targets
    # Out of service at one time only: the series keeps the set point there.
    net = pn.case9()
    plan, _ = schedule_from_opf(net, Schedule.from_values(times, {("gen/1", "in_service"): [1.0, 0.0]}))
    values = {(s["target"], s["quantity"]): s["values"] for s in plan.series}
    assert values[("gen/1", "voltage_pu")][1] == net.gen.vm_pu[1]
    assert values[("gen/1", "active_power_mw")][1] == net.gen.p_mw[1]
    run = run_time_slices(net, plan, end_time_s=0.02)
    assert all(r["status"] == "completed" and r["max_abs_dv_pu"] < TOL for r in run.rows)


def test_negative_active_power_load_splits_reactive_impedance():
    """P1: a load with P < 0 and Q != 0 keeps Q as a constant-impedance
    three_phase_load (P = 0) and puts only P into a three_phase_current_source."""
    net = pp.create_empty_network(f_hz=50.0)
    b0, b1 = pp.create_bus(net, 20.0), pp.create_bus(net, 20.0)
    pp.create_ext_grid(net, b0, vm_pu=1.0, s_sc_max_mva=1000.0, rx_max=0.1)
    pp.create_line_from_parameters(net, b0, b1, 5.0, r_ohm_per_km=0.1, x_ohm_per_km=0.3,
                                   c_nf_per_km=200.0, max_i_ka=1.0)
    pp.create_load(net, b1, p_mw=-2.0, q_mvar=-0.5)
    conv = from_pandapower(net, end_time_s=0.02)
    loads = [c for c in conv.model["components"] if c["name"] == "load0"]
    ps = [c for c in conv.model["components"] if c["name"] == "load0_p"]
    assert len(loads) == 1 and loads[0]["type"] == "three_phase_load"
    assert loads[0]["active_power_mw"] == 0.0 and loads[0]["reactive_power_mvar"] == pytest.approx(-0.5)
    assert len(ps) == 1 and ps[0]["type"] == "three_phase_current_source"
    c = conv.compare(openkeito.run(conv.model))
    assert c["max_abs_dv_pu"] < TOL and c["max_abs_dva_degree"] < TOL
