"""Phase-angle clocks in the PowSyBl converter (``phase_angle_clocks``).

IIDM's load flow does not turn the voltages by the clocks of the
transformers. With ``phase_angle_clocks="rotate"`` the model does: the ideal
ratio of every transformer turns by its clock (the positive sequence by
-clock * 30 degrees from side 1 to side 2, the negative sequence by as much
the other way), and the load-flow voltages of every zone between
transformers are turned by the clocks on the way to it.

Checked here:

- the turned load flow is the steady state of the model (1e-9 pu and degrees,
  as without clocks: turning every voltage and current of a zone by one
  angle leaves its nodal equations as they are);
- behind a Dyn1 and a YNd11 transformer, the phase quantities of a
  line-to-ground fault are those of a hand calculation with symmetrical
  components, in which the positive- and the negative-sequence part of every
  bus are turned by plus and minus the angle of its zone. The tolerance is
  that of the zero-sequence impedance of the fault (5e-4, see
  test_powsybl_short_circuit);
- clocks that contradict each other around a loop are refused.
"""

from __future__ import annotations

import cmath
import math

import numpy as np
import pytest

pn = pytest.importorskip("pypowsybl.network")

import openkeito  # noqa: E402
from openkeito.convert import ConversionError, phasors  # noqa: E402
from openkeito.powsybl import from_powsybl  # noqa: E402

from test_powsybl_short_circuit import (  # noqa: E402
    BLOCKED, BUSES, CASES, GENERATORS, LINES, RF, TOL, eurostag, sequence_matrices)

A = cmath.exp(2j * math.pi / 3)

#: The step-up transformer as Dyn1 (the 380 kV star lags the 24 kV delta by
#: 30 degrees) and the load transformer as YNd11 (the 150 kV delta leads the
#: 380 kV star by 30 degrees). The 380 kV buses keep the angles of the load
#: flow, so both the generators and the load are 30 degrees ahead of it.
CLOCKS = {"NGEN_NHV1": 1, "NHV2_NLOAD": 11}
SHIFT = {"VLGEN_0": 30.0, "VLHV1_0": 0.0, "VLHV2_0": 0.0, "VLLOAD_0": 30.0}


def data_with_clocks(clocks=CLOCKS):
    data = {**GENERATORS, **LINES, **{tid: dict(row) for tid, row in CASES["Dyn_YNd"].items()}}
    for tid, clock in clocks.items():
        data[tid]["phase_angle_clock"] = clock
    return data


def steady(conv, extra=()):
    """(phasors of the steady state, result) of the model with more
    components."""
    model = dict(conv.model)
    model["components"] = conv.model["components"] + list(extra)
    result = openkeito.run(model)
    assert result.status == "completed" and not result.warnings
    return phasors(result), result


def test_turned_by_the_clocks_the_load_flow_is_still_the_steady_state():
    net = eurostag()
    plain = from_powsybl(net, short_circuit_data={**GENERATORS, **LINES, **CASES["Dyn_YNd"]}, end_time_s=0.02)
    ignored = from_powsybl(net, short_circuit_data=data_with_clocks(), end_time_s=0.02)
    conv = from_powsybl(net, short_circuit_data=data_with_clocks(), phase_angle_clocks="rotate", end_time_s=0.02)
    # By default the clocks do nothing but a warning.
    assert ignored.model == plain.model and "bus_phase_shift_deg" not in ignored.mapping
    assert [w for w in ignored.warnings if "clock" in w] == [
        w for w in ignored.warnings if w.startswith("phase-angle clocks are not applied")
        and w.endswith(": NGEN_NHV1, NHV2_NLOAD")] != []
    assert not any("clock" in w for w in plain.warnings + conv.warnings)

    assert conv.mapping["bus_phase_shift_deg"] == SHIFT
    for bus in BUSES:
        vm, va, vn = plain.power_flow[bus]
        assert conv.power_flow[bus] == (vm, va + SHIFT[bus], vn)
    assert openkeito.validate(conv.model)["valid"]
    result = openkeito.run(conv.model)
    c = conv.compare(result)
    assert not result.warnings and c["max_abs_dv_pu"] < TOL and c["max_abs_dva_degree"] < TOL, c
    # The waveforms stay on the phasors.
    w_h = 2 * math.pi * 50.0 * conv.model["simulation"]["time_step_s"]
    assert result.summary["initialization"]["relative_voltage_deviation"] < w_h ** 2
    # Only the ideal ratios and the angles of sources differ from the model
    # without clocks (and, in its last digit, the voltage behind a turned
    # ratio at which a magnetizing branch is rated).
    by_name = {c["name"]: c for c in plain.model["components"]}
    changed = {c["name"] for c in conv.model["components"] if c != by_name[c["name"]]}
    assert {"gen0", "gen1", "tr0", "tr1"} <= changed <= {"gen0", "gen1", "tr0", "tr1", "tr1_y"}
    turned = {c["name"]: c for c in conv.model["components"]}
    assert turned["tr0"]["phase_shift_deg"] == pytest.approx(-30.0)
    assert turned["tr1"]["phase_shift_deg"] == pytest.approx(30.0)
    assert turned["gen0"]["angle_deg"] == pytest.approx(by_name["gen0"]["angle_deg"] + 30.0)


def test_machines_and_converters_start_from_the_turned_load_flow():
    net = eurostag()
    conv = from_powsybl(net, short_circuit_data={"NGEN_NHV1": {"phase_angle_clock": 5}},
                        phase_angle_clocks="rotate", generators="machine", converter_generators=["GEN2"],
                        end_time_s=0.02)
    assert conv.mapping["bus_phase_shift_deg"]["VLGEN_0"] == 150.0
    types = [c["type"] for c in conv.model["components"]]
    assert "synchronous_machine" in types and "grid_following_converter" in types
    result = openkeito.run(conv.model)
    c = conv.compare(result)
    assert c["max_abs_dv_pu"] < TOL and c["max_abs_dva_degree"] < TOL, c


def test_behind_the_delta_the_phases_of_a_line_to_ground_fault_are_those_of_the_hand_calculation():
    net = eurostag()
    data = data_with_clocks()
    conv = from_powsybl(net, short_circuit_data=data, phase_angle_clocks="rotate", outputs="all", end_time_s=0.02)
    ignored = from_powsybl(net, short_circuit_data=data, outputs="all", end_time_s=0.02)
    y1, y0 = sequence_matrices(net, conv, data)
    z1, z0 = np.linalg.inv(y1), np.linalg.inv(y0)
    at = {b: i for i, b in enumerate(BUSES)}
    f = at["VLHV1_0"]
    gen, load = at["VLGEN_0"], at["VLLOAD_0"]

    # Hand calculation. Phase a of the 380 kV bus VLHV1 to gnd through RF:
    # I1 = I2 = I0 = V / (Z1 + Z2 + Z0 + 3 RF) with the load-flow voltage V
    # of the bus, which is in the zone that keeps its angles.
    vm, va, vn = conv.power_flow["VLHV1_0"]
    i_seq = cmath.rect(vm * vn * 1e3 / math.sqrt(3), math.radians(va)) / (2 * z1[f, f] + z0[f, f] + 3 * RF)
    # The fault takes I1 out of the positive- and the negative-sequence
    # network (the same passive network: sources have Z2 = Z1) and I0 out
    # of the zero-sequence one. The voltages change by -Z[:, f] I.
    dv1 = -z1[:, f] * i_seq
    dv0 = -z0[:, f] * i_seq

    def phases(bus, positive, negative, zero):
        """Phase values a, b, c from the sequence values of a bus, which
        are those of a network without clocks: the positive sequence of
        the zone is turned by its angle, the negative by minus that."""
        turn = cmath.exp(1j * math.radians(SHIFT[bus]))
        p, n = positive * turn, negative / turn
        return np.array([zero + p + n, zero + A ** 2 * p + A * n, zero + A * p + A ** 2 * n])

    def measured(with_clocks, names):
        """Change of the phasors of the model by the fault."""
        before, _ = steady(with_clocks)
        after, result = steady(with_clocks, [{"type": "resistor", "name": "RF",
                                              "nodes": [f"{with_clocks.bus('VLHV1_0')}.a", "gnd"],
                                              "resistance_ohm": RF}])
        # The unbalanced state is the steady state of the run.
        w_h = 2 * math.pi * 50.0 * with_clocks.model["simulation"]["time_step_s"]
        assert result.summary["initialization"]["relative_voltage_deviation"] < w_h ** 2
        return [np.array([after[n.format(p)] - before[n.format(p)] for p in "abc"]) for n in names]

    names = [f"V({conv.bus('VLGEN_0')}.{{}})", f"V({conv.bus('VLLOAD_0')}.{{}})", "I(gen0.{})", "I(gen1.{})",
             f"V({conv.bus('VLHV2_0')}.{{}})"]
    v_gen, v_load, i_gen, i_gen2, v_hv2 = measured(conv, names)

    # Voltages behind the two transformers. The zero sequence does not pass
    # the deltas: at the generators it is only what the blocking factor
    # lets through, at the load nothing.
    expected_gen = phases("VLGEN_0", dv1[gen], dv1[gen], dv0[gen])
    expected_load = phases("VLLOAD_0", dv1[load], dv1[load], dv0[load])
    scale = abs(dv1[gen])
    assert np.max(np.abs(v_gen - expected_gen)) < BLOCKED * scale, (v_gen, expected_gen)
    assert np.max(np.abs(v_load - expected_load)) < BLOCKED * abs(dv1[load]), (v_load, expected_load)
    # In the zone of the fault nothing is turned.
    hv2 = at["VLHV2_0"]
    expected_hv2 = np.array([dv0[hv2] + 2 * dv1[hv2], dv0[hv2] - dv1[hv2], dv0[hv2] - dv1[hv2]])
    assert np.max(np.abs(v_hv2 - expected_hv2)) < BLOCKED * abs(dv1[hv2])

    # Currents of the generators (a source's current flows from the bus
    # into it: dI = dV / z in each sequence; no zero-sequence voltage to
    # speak of behind the delta).
    w = 2 * math.pi * 50.0
    by_name = {c["name"]: c for c in conv.model["components"]}
    for name, current in (("gen0", i_gen), ("gen1", i_gen2)):
        z = complex(by_name[name]["impedance"]["r1_ohm"], by_name[name]["impedance"]["l1_h"] * w)
        expected = phases("VLGEN_0", dv1[gen] / z, dv1[gen] / z, 0.0)
        assert np.max(np.abs(current - expected)) < BLOCKED * np.max(np.abs(expected)), (name, current, expected)
        # The distribution over the phases is that of a delta-star
        # transformer: the phase that lags the faulted one on the star side
        # carries nothing, the other two the same current in opposite
        # directions (30 degrees ahead, I1 e^{j30} + I2 e^{-j30} and its
        # turns by 120 degrees are sqrt(3) I (1, 0, -1)).
        assert abs(current[1]) < BLOCKED * abs(current[0])
        assert abs(current[0] + current[2]) < BLOCKED * abs(current[0])
        assert abs(abs(current[0]) - math.sqrt(3) * abs(dv1[gen] / z)) < BLOCKED * abs(current[0])

    # Without the clocks the same fault gives the distribution of a
    # star-star transformer (2, -1, -1), which is what the option corrects.
    (plain,) = measured(ignored, ["I(gen0.{})"])
    z = complex(by_name["gen0"]["impedance"]["r1_ohm"], by_name["gen0"]["impedance"]["l1_h"] * w)
    yy = dv1[gen] / z * np.array([2.0, -1.0, -1.0])
    assert np.max(np.abs(plain - yy)) < BLOCKED * abs(yy[0])
    assert np.max(np.abs(plain - i_gen)) > 0.5 * abs(yy[0])


def test_three_winding_transformers_phase_shifters_and_boundaries_are_turned():
    net = pn.create_micro_grid_be_network()
    plain = from_powsybl(net, end_time_s=0.02)
    three = net.get_3_windings_transformers(all_attributes=True)
    two = net.get_2_windings_transformers(all_attributes=True)
    tid = three.index[0]
    shifter = next(t for t in two.index if abs(two.at[t, "alpha"]) > 1.0)
    low = next(t for t in two.index if two.at[t, "bus2_id"] in set(net.get_generators()["bus_id"]))
    # The generator of leg 3 behind a clock of 11, the machine transformer
    # with 5, and the phase shifter and the transformer beside it, which
    # end in the same 110 kV zone, with 6.
    clocks = {tid: {"phase_angle_clock_leg2": 0, "phase_angle_clock_leg3": 11}, low: {"phase_angle_clock": 5}}
    beside = [t for t in two.index if t != low]
    assert shifter in beside and len(beside) == 2
    clocks.update({t: {"phase_angle_clock": 6} for t in beside})
    conv = from_powsybl(net, short_circuit_data=clocks, phase_angle_clocks="rotate", outputs="all", end_time_s=0.02)
    shift = conv.mapping["bus_phase_shift_deg"]
    assert shift[three.at[tid, "bus1_id"]] == 0.0 and shift[three.at[tid, "bus2_id"]] == 0.0
    assert shift[three.at[tid, "bus3_id"]] == 30.0
    assert shift[two.at[shifter, "bus2_id"]] == -180.0 and shift[two.at[low, "bus2_id"]] == -180.0 - 150.0 + 360.0
    assert {b: (v[0], v[2]) for b, v in conv.power_flow.items()} == {
        b: (v[0], v[2]) for b, v in plain.power_flow.items()}
    result = openkeito.run(conv.model)
    c = conv.compare(result)
    assert not result.warnings and c["max_abs_dv_pu"] < TOL and c["max_abs_dva_degree"] < TOL, c
    # The phase tap changer adds its angle to the clock.
    by_name = {c["name"]: c for c in conv.model["components"]}
    k = list(two.index).index(shifter)
    assert by_name[f"tr{k}"]["phase_shift_deg"] == pytest.approx(float(two.at[shifter, "alpha"]) - 180.0)
    # Sources deliver what they deliver without clocks.
    before = plain.source_powers(openkeito.run(dict(plain.model, outputs=conv.model["outputs"])))
    after = conv.source_powers(result)
    assert before.keys() == after.keys() and len(after) > 2
    assert max(abs(after[n] - before[n]) for n in after) < 1e-6
    # The zero sequence passes the transformers whose connections are not
    # known as through YNyn0, also where the clock is 6: a warning. An odd
    # clock makes the windings a delta and grounded stars.
    assert any(w.startswith("the zero sequence passes between grounded stars") and shifter in w
               for w in conv.warnings)
    assumed = next(w for w in conv.warnings if w.startswith("transformer connections unknown; with an odd"))
    assert tid in assumed and low in assumed and shifter not in assumed
    assert by_name["tr3_0_3_z0"]["buses"][0] == "tr3_0_star"
    k = list(two.index).index(low)
    high_voltage_side_is_1 = two.at[low, "rated_u1"] >= two.at[low, "rated_u2"]
    assert by_name[f"tr{k}_z0"]["buses"][0] == (conv.bus(two.at[low, "bus2_id"]) if high_voltage_side_is_1
                                                else f"tr{k}_m")


def test_clocks_that_do_not_add_up_around_a_loop_are_refused():
    # The three transformers of IEEE 14 end in one 12 kV network.
    net = pn.create_ieee14()
    for clocks, one in (({"T4-7-1": 11}, "T4-7-1"), ({"T4-7-1": 11, "T4-9-1": 11, "T5-6-1": 1}, "T5-6-1")):
        data = {t: {"phase_angle_clock": c} for t, c in clocks.items()}
        with pytest.raises(ConversionError) as e:
            from_powsybl(net, short_circuit_data=data, phase_angle_clocks="rotate")
        text = str(e.value)
        assert "the phase-angle clocks do not add up around the loop through" in text
        assert "phase_angle_clocks='ignore'" in text
        # The loop is named by its transformers with a clock.
        named = text.partition("loop through ")[2].partition(":")[0]
        assert f"transformer {one}" in named and "line" not in named
        # Ignored, they convert.
        assert from_powsybl(net, short_circuit_data=data).model == from_powsybl(net).model
    data = {t: {"phase_angle_clock": 11} for t in ("T4-7-1", "T4-9-1", "T5-6-1")}
    conv = from_powsybl(net, short_circuit_data=data, phase_angle_clocks="rotate", end_time_s=0.02)
    assert sorted(set(conv.mapping["bus_phase_shift_deg"].values())) == [0.0, 30.0]
    assert conv.mapping["bus_phase_shift_deg"]["VL1_0"] == 0.0
    c = conv.compare(openkeito.run(conv.model))
    assert c["max_abs_dv_pu"] < TOL and c["max_abs_dva_degree"] < TOL, c


def test_wrong_clocks_and_options_are_refused():
    net = eurostag()
    with pytest.raises(ValueError, match="phase_angle_clocks must be one of ignore, rotate"):
        from_powsybl(net, phase_angle_clocks="yes")
    for value in (12, -1, 1.5):
        with pytest.raises(ConversionError, match="is not a whole number from 0 to 11"):
            from_powsybl(net, short_circuit_data={"NGEN_NHV1": {"phase_angle_clock": value}},
                         phase_angle_clocks="rotate")
    # Without clocks the option changes nothing.
    assert from_powsybl(net, phase_angle_clocks="rotate").model == from_powsybl(net).model


def test_the_clock_of_the_extension_is_used():
    net = eurostag()
    net.create_extensions("twoWindingsTransformerPhaseAngleClock", id="NHV2_NLOAD", phase_angle_clock=11)
    conv = from_powsybl(net, phase_angle_clocks="rotate", end_time_s=0.02)
    assert conv.mapping["bus_phase_shift_deg"]["VLLOAD_0"] == 30.0
    assert conv.mapping["short_circuit"]["two_windings_transformer/NHV2_NLOAD"]["sources"] == [
        "twoWindingsTransformerPhaseAngleClock"]
    c = conv.compare(openkeito.run(conv.model))
    assert c["max_abs_dv_pu"] < TOL and c["max_abs_dva_degree"] < TOL, c
    # Connections unknown and an odd clock: the delta is at the higher
    # rated voltage (side 1 here), so side 2 has the grounded star.
    by_name = {c["name"]: c for c in conv.model["components"]}
    assert by_name["tr1_z0"]["buses"][0] == conv.bus("VLLOAD_0")
    assert by_name["tr1_z"]["impedance"]["r0_ohm"] > 1e5 * by_name["tr1_z"]["impedance"]["r1_ohm"]


def test_ynyn6_inverts_zero_sequence_without_warning_and_matches_hand_calculation():
    # Two-winding Eurostag transformers given known Y_GROUNDED-Y_GROUNDED
    # windings with NHV2_NLOAD at clock 6 (YNyn6) and NGEN_NHV1 at clock 0.
    net = pn.create_eurostag_tutorial_example1_network()
    data = {
        "NGEN_NHV1": {"connection_type1": "Y_GROUNDED", "connection_type2": "Y_GROUNDED", "phase_angle_clock": 0},
        "NHV2_NLOAD": {"connection_type1": "Y_GROUNDED", "connection_type2": "Y_GROUNDED", "phase_angle_clock": 6},
    }
    conv6 = from_powsybl(net, short_circuit_data=data, phase_angle_clocks="rotate", outputs="all", end_time_s=0.02)
    assert conv6.model["version"] == "0.6"
    assert not any("the zero sequence passes between grounded stars" in w for w in conv6.warnings), conv6.warnings
    by_name = {c["name"]: c for c in conv6.model["components"]}
    assert by_name["tr1"]["zero_sequence_inverted"] is True
    assert "zero_sequence_inverted" not in by_name["tr0"]

    # Power flow matches before turning; steady state preserves load flow.
    res6 = openkeito.run(conv6.model)
    c = conv6.compare(res6)
    assert c["max_abs_dv_pu"] < TOL and c["max_abs_dva_degree"] < TOL, c

    # Single-line-to-ground fault at VLLOAD_0 (phase a): compare zero-sequence
    # voltage change on the primary bus VLHV2_0 against YNyn0 (clock 0).
    # Note: phase_angle_clocks="rotate" keeps zone VLLOAD_0 shifted by -180 deg
    # relative to VLHV2_0, while on phase components in VLHV2_0 the zero
    # sequence of YNyn6 has the OPPOSITE sign to YNyn0!
    data0 = {
        "NGEN_NHV1": {"connection_type1": "Y_GROUNDED", "connection_type2": "Y_GROUNDED", "phase_angle_clock": 0},
        "NHV2_NLOAD": {"connection_type1": "Y_GROUNDED", "connection_type2": "Y_GROUNDED", "phase_angle_clock": 0},
    }
    conv0 = from_powsybl(net, short_circuit_data=data0, phase_angle_clocks="rotate", outputs="all", end_time_s=0.02)

    def slg_hv2_zero_transfer(conv):
        before, _ = steady(conv)
        fault = [{"type": "resistor", "name": "RF", "nodes": [f"{conv.bus('VLLOAD_0')}.a", "gnd"], "resistance_ohm": 200.0}]
        after, _ = steady(conv, fault)
        dv = np.array([after[f"V({conv.bus('VLHV2_0')}.{p})"] - before[f"V({conv.bus('VLHV2_0')}.{p})"] for p in "abc"])
        v_load_pre = before[f"V({conv.bus('VLLOAD_0')}.a)"]
        # Normalized by pre-fault voltage at the faulted zone (which rotates by
        # -180 deg for clock 6): isolates the zero-sequence turns ratio sign (-1 vs +1).
        return np.mean(dv) / v_load_pre

    tr0 = slg_hv2_zero_transfer(conv0)
    tr6 = slg_hv2_zero_transfer(conv6)
    assert abs(tr6 + tr0) < BLOCKED * abs(tr0), (tr0, tr6)

    # Clocks other than 0 and 6 (e.g. even clock 2 between known Y_GROUNDED) still warn.
    data2 = {
        "NGEN_NHV1": {"connection_type1": "Y_GROUNDED", "connection_type2": "Y_GROUNDED", "phase_angle_clock": 0},
        "NHV2_NLOAD": {"connection_type1": "Y_GROUNDED", "connection_type2": "Y_GROUNDED", "phase_angle_clock": 2},
    }
    conv2 = from_powsybl(net, short_circuit_data=data2, phase_angle_clocks="rotate", end_time_s=0.02)
    assert any("the zero sequence passes between grounded stars" in w and "NHV2_NLOAD" in w for w in conv2.warnings)
