"""Short-circuit data in the PowSyBl converter: subtransient reactances,
zero-sequence impedances, winding connections and grounding, read from the
IIDM extensions, from the short-circuit attributes of a CGMES file or from
``short_circuit_data``.

The reference is a hand calculation with symmetrical components on the
Eurostag example (two generators, a step-up transformer, two lines, a
transformer and a load): the nodal admittance matrices of the positive and
the zero sequence are written here from the IIDM data, and their inverses
give the impedances that a frequency scan of the model must show and the
current of a line-to-ground fault, I = 3 V / (Z1 + Z2 + Z0 + 3 Rf).

Tolerance of the comparison: where a winding lets no zero-sequence current
pass, the model has ZERO_SEQUENCE_BLOCK (1e6) times the positive-sequence
impedance z of the branch in place of an open circuit. That changes a
zero-sequence impedance Z0 at a bus by |Z0| / (1e6 |z|) of itself for every
such branch there. At the generators that is 1.6e-4: |Z0| = 6.3 ohm (the
grounding impedance of GEN, three times) beside the blocked step-up
transformer, whose 11.1 ohm at 400 kV are 0.04 ohm at 24 kV. At the 380 kV
buses with both transformers blocked it is 2.2e-4: the 1500 ohm of the line
capacitances beside 1e6 times 11 ohm and 26 ohm. The tolerance is 5e-4.
Everything else is exact to rounding, so the positive sequence is compared
to 1e-9.
"""

from __future__ import annotations

import cmath
import math
import re
import zipfile

import numpy as np
import pytest

pn = pytest.importorskip("pypowsybl.network")

import openkeito  # noqa: E402
from openkeito.convert import fault  # noqa: E402
from openkeito.powsybl import (  # noqa: E402
    ZERO_SEQUENCE_BLOCK,
    cgmes_short_circuit_data,
    from_cgmes,
    from_powsybl,
    short_circuit_data,
)

TOL = 1e-9
BLOCKED = 5e-4
BUSES = ["VLGEN_0", "VLHV1_0", "VLHV2_0", "VLLOAD_0"]
RF = 5.0

#: Generators: one grounded through an impedance, one not grounded.
GENERATORS = {
    "GEN": {"direct_sub_trans_x": 0.9, "rz": 0.01, "xz": 0.4, "grounded": True, "grounding_r": 2.0,
            "grounding_x": 0.5},
    "GEN2": {"direct_sub_trans_x": 1.1, "grounded": False},
}
LINES = {
    "NHV1_NHV2_1": {"rz": 12.0, "xz": 100.0, "bz": 2e-4},
    "NHV1_NHV2_2": {"rz": 10.0, "xz": 90.0},
}
#: Transformer connections (step-up NGEN_NHV1, then NHV2_NLOAD).
CASES = {
    "Dyn_YNd": {
        "NGEN_NHV1": {"connection_type1": "DELTA", "connection_type2": "Y_GROUNDED", "rz": 0.2, "xz": 10.0,
                      "grounding_x2": 3.0},
        "NHV2_NLOAD": {"connection_type1": "Y_GROUNDED", "connection_type2": "DELTA", "free_fluxes": False,
                       "grounding_r1": 4.0},
    },
    "YNyn_YNyn": {
        "NGEN_NHV1": {"connection_type1": "Y_GROUNDED", "connection_type2": "Y_GROUNDED", "rz": 0.2, "xz": 10.0,
                      "grounding_x1": 0.02, "grounding_x2": 3.0},
        "NHV2_NLOAD": {"connection_type1": "Y_GROUNDED", "connection_type2": "Y_GROUNDED", "free_fluxes": True,
                       "grounding_r2": 1.0},
    },
    "Yyn_YNy": {
        "NGEN_NHV1": {"connection_type1": "Y", "connection_type2": "Y_GROUNDED"},
        # A core that gives the zero-sequence flux no path of iron: a
        # zero-sequence magnetizing reactance of its own, far below the
        # 33 kilo-ohm of the positive sequence.
        "NHV2_NLOAD": {"connection_type1": "Y_GROUNDED", "connection_type2": "Y", "free_fluxes": False,
                       "xm": 180.0},
    },
}


def eurostag():
    net = pn.create_eurostag_tutorial_example1_network()
    # A magnetizing branch, to tell free fluxes from the others.
    net.update_2_windings_transformers(id="NHV2_NLOAD", g=2e-6, b=-3e-5)
    return net


def sequence_impedances(net, conv, data):
    """Thevenin impedances (Z1, Z0) at the four buses, in ohm, from the IIDM
    data, the load-flow voltage of the load and the short-circuit data."""
    y1, y0 = sequence_matrices(net, conv, data)
    return np.diag(np.linalg.inv(y1)), np.diag(np.linalg.inv(y0))


def sequence_matrices(net, conv, data):
    """Nodal admittance matrices (positive, zero sequence) of the four
    buses, in siemens, from the same data; the sources are short-circuited
    behind their impedances."""
    at = {b: i for i, b in enumerate(BUSES)}
    y1 = np.zeros((4, 4), dtype=complex)
    y0 = np.zeros((4, 4), dtype=complex)

    def branch(y, i, j, k, z, ym):
        """Ideal ratio k at bus i, then ym to ground and z to bus j."""
        y[i, i] += k * k * (1 / z + ym)
        y[j, j] += 1 / z
        y[i, j] -= k / z
        y[j, i] -= k / z

    gens = net.get_generators()
    z_base = 24.0 ** 2 / 100.0  # no rated_s: 100 MVA
    for gid, g in gens.iterrows():
        d = data[gid]
        z = complex(0.005 * z_base, d["direct_sub_trans_x"])
        y1[0, 0] += 1 / z
        if d["grounded"]:
            zg = complex(d.get("grounding_r", 0.0), d.get("grounding_x", 0.0))
            y0[0, 0] += 1 / (complex(d.get("rz", z.real), d.get("xz", z.imag)) + 3 * zg)
    v_load = conv.power_flow["VLLOAD_0"][0] * conv.power_flow["VLLOAD_0"][2]
    load = net.get_loads().iloc[0]
    y_load = complex(load["p0"], -load["q0"]) / v_load ** 2
    y1[3, 3] += y_load
    y0[3, 3] += y_load
    for lid, ln in net.get_lines(all_attributes=True).iterrows():
        d = data[lid]
        i, j = at[ln["bus1_id"]], at[ln["bus2_id"]]
        z = complex(ln["r"], ln["x"])
        ends = [complex(ln["g1"], ln["b1"]), complex(ln["g2"], ln["b2"])]
        branch(y1, i, j, 1.0, z, 0.0)
        branch(y0, i, j, 1.0, complex(d["rz"], d["xz"]), 0.0)
        for bus, y in zip((i, j), ends):
            y1[bus, bus] += y
            y0[bus, bus] += 1j * d["bz"] / 2 if "bz" in d else y
    for tid, t in net.get_2_windings_transformers(all_attributes=True).iterrows():
        d = data[tid]
        i, j = at[t["bus1_id"]], at[t["bus2_id"]]
        k = float(t["rho"])
        z = complex(t["r_at_current_tap"], t["x_at_current_tap"])
        ym = complex(t["g_at_current_tap"], t["b_at_current_tap"])
        branch(y1, i, j, k, z, ym)
        z0 = complex(d.get("rz", z.real), d.get("xz", z.imag))
        zg1 = complex(d.get("grounding_r1", 0.0), d.get("grounding_x1", 0.0)) * k * k  # at side 2
        zg2 = complex(d.get("grounding_r2", 0.0), d.get("grounding_x2", 0.0))
        c1, c2 = d["connection_type1"], d["connection_type2"]
        ym0 = ym if c1 == "Y_GROUNDED" and not d.get("free_fluxes", True) else 0.0
        if ym0 and "xm" in d:
            # The zero-sequence magnetizing reactance, at side 2.
            ym0 = 1 / (1j * d["xm"])
        if c1 == c2 == "Y_GROUNDED":
            branch(y0, i, j, k, z0 + 3 * zg1 + 3 * zg2, ym0)
        elif c1 == "Y_GROUNDED" and c2 == "DELTA":
            y0[i, i] += k * k * (ym0 + 1 / (z0 + 3 * zg1))
        elif c1 == "DELTA" and c2 == "Y_GROUNDED":
            y0[j, j] += 1 / (z0 + 3 * zg2)
        else:
            y0[i, i] += k * k * ym0
    # A bus without any zero-sequence path has an infinite impedance.
    y0 += 1e-15 * np.eye(4)
    return y1, y0


def scanned(conv):
    """(Z1, Z2, Z0) at every bus of the model at its frequency, from the
    frequency scan."""
    model = dict(conv.model)
    names = [conv.bus(b) for b in BUSES]
    model["frequency_scan"] = {"frequencies_hz": [conv.mapping["frequency_hz"]], "buses": names}
    scan = openkeito.scan(model)
    return [np.array([cmath.rect(scan[f"Z{s}mag({b})"][0], math.radians(scan[f"Z{s}deg({b})"][0])) for b in names])
            for s in (1, 2, 0)]


@pytest.mark.parametrize("case", list(CASES))
def test_sequence_impedances_and_the_line_to_ground_fault_are_those_of_the_hand_calculation(case):
    net = eurostag()
    data = {**GENERATORS, **LINES, **CASES[case]}
    conv = from_powsybl(net, short_circuit_data=data, outputs="all", end_time_s=0.02)
    assert openkeito.validate(conv.model)["valid"]
    # The data are used, and nothing about the zero sequence is assumed.
    assert set(conv.mapping["short_circuit"]) == {
        "generator/GEN", "generator/GEN2", "line/NHV1_NHV2_1", "line/NHV1_NHV2_2",
        "two_windings_transformer/NGEN_NHV1", "two_windings_transformer/NHV2_NLOAD"}
    assert conv.mapping["short_circuit"]["line/NHV1_NHV2_1"] == {"sources": ["short_circuit_data"],
                                                                 "data": LINES["NHV1_NHV2_1"]}
    assert not any("unknown" in w or "no zero-sequence data" in w or "no subtransient" in w
                   for w in conv.warnings), conv.warnings
    # The load flow is still the steady state.
    result = openkeito.run(conv.model)
    c = conv.compare(result)
    assert not result.warnings and c["max_abs_dv_pu"] < TOL and c["max_abs_dva_degree"] < TOL, c

    z1, z0 = sequence_impedances(net, conv, data)
    s1, s2, s0 = scanned(conv)
    assert np.max(np.abs(s1 / z1 - 1)) < TOL and np.max(np.abs(s2 / z1 - 1)) < TOL
    open_circuit = np.abs(z0) > 1e9
    assert np.max(np.abs(s0 / z0 - 1)[~open_circuit]) < BLOCKED, (s0, z0)
    # No path at all: only the blocked branches are left.
    assert np.all(np.abs(s0[open_circuit]) > 0.1 * ZERO_SEQUENCE_BLOCK)

    # A line-to-ground fault through RF that is there from the start: the
    # steady state of the run is the fault, I = 3 V / (Z1 + Z2 + Z0 + 3 RF)
    # with the load-flow voltage V of the bus.
    for k, bus in enumerate(BUSES):
        if open_circuit[k]:
            continue
        model = dict(conv.model)
        model["components"] = conv.model["components"] + [
            {"type": "resistor", "name": "RF", "nodes": [f"{conv.bus(bus)}.a", "gnd"], "resistance_ohm": RF}]
        model["outputs"] = [{"type": "branch_current", "component": "RF"}]
        faulted = openkeito.run(model)
        phasors = {p["name"]: cmath.rect(p["rms"], math.radians(p["angle_deg"]))
                   for p in faulted.summary["initialization"]["phasors"]}
        # The waveforms stay on these phasors: the unbalanced state is also
        # the steady state of the time-domain elements (the limit is the
        # error of the integration over a step, as for the load flow).
        w_h = 2 * math.pi * 50.0 * model["simulation"]["time_step_s"]
        assert not faulted.warnings and faulted.summary["initialization"]["relative_voltage_deviation"] < w_h ** 2
        vm, va, vn = conv.power_flow[bus]
        v = cmath.rect(vm * vn * 1e3 / math.sqrt(3), math.radians(va))
        expected = 3 * v / (2 * z1[k] + z0[k] + 3 * RF)
        assert abs(phasors["I(RF)"] / expected - 1) < BLOCKED, (bus, phasors["I(RF)"], expected)


def test_a_delta_winding_keeps_the_zero_sequence_out():
    net = eurostag()
    data = {**GENERATORS, **LINES, **CASES["Dyn_YNd"]}
    conv = from_powsybl(net, short_circuit_data=data)
    _, _, s0 = scanned(conv)
    # At the generators only the grounded generator is seen (the step-up
    # transformer has its delta there), at the load only the load.
    d = GENERATORS["GEN"]
    z_gen = complex(d["rz"], d["xz"]) + 3 * complex(d["grounding_r"], d["grounding_x"])
    v_load = conv.power_flow["VLLOAD_0"][0] * conv.power_flow["VLLOAD_0"][2]
    z_load = v_load ** 2 / complex(600.0, -200.0)
    assert abs(s0[0] / z_gen - 1) < BLOCKED and abs(s0[3] / z_load - 1) < BLOCKED
    # As YNyn0 (no data), the same buses see the network behind the transformers.
    _, _, through = scanned(from_powsybl(net, short_circuit_data={**GENERATORS, **LINES}))
    assert abs(through[0] / z_gen - 1) > 0.05 and abs(through[3] / z_load - 1) > 0.05
    # The delta is a bank that takes no positive-sequence current.
    banks = [c for c in conv.model["components"] if c["type"] == "three_phase_transformer"]
    assert [c["name"] for c in banks] == ["tr0_z0", "tr1_z0"]
    # A fault that starts and is cleared during the run: phase a of the bus
    # is at gnd while it lasts, and the voltages at the load, behind the
    # delta, keep their sum at zero (no zero-sequence voltage gets there
    # but that of the load currents, which have none).
    model = dict(conv.model)
    model["simulation"] = {**model["simulation"], "end_time_s": 0.1}
    model["components"] = model["components"] + [fault("VLHV1_0", 0.02, 0.06, phases="a")["components"][0]
                                                 | {"bus": conv.bus("VLHV1_0")}]
    result = openkeito.run(model)
    assert result.status == "completed" and not result.warnings
    time = result.time
    during = (time > 0.03) & (time < 0.06)
    hv, load = conv.bus("VLHV1_0"), conv.bus("VLLOAD_0")
    peak = np.max(np.abs(result[f"V({hv}.a)"][time < 0.02]))
    assert np.max(np.abs(result[f"V({hv}.a)"][during])) < 1e-6 * peak
    assert np.max(np.abs(result[f"V({hv}.b)"][during])) > 0.5 * peak
    zero = sum(result[f"V({load}.{p})"] for p in "abc") / 3
    assert np.max(np.abs(zero[during])) < 1e-3 * np.max(np.abs(result[f"V({load}.a)"]))


def test_without_data_the_assumptions_and_their_warnings_stay():
    net = eurostag()
    conv = from_powsybl(net)
    assert conv.mapping["short_circuit"] == {} and short_circuit_data(net) == {}
    assert from_powsybl(net, short_circuit_data={}).model == conv.model
    types = {c["type"] for c in conv.model["components"]}
    assert "three_phase_transformer" not in types
    assert not any("neutral" in c for c in conv.model["components"])
    for c in conv.model["components"]:
        if c["type"] == "coupled_rl":
            factor = 3.0 if c["name"].startswith("line") else 1.0
            assert c["impedance"]["r0_ohm"] == factor * c["impedance"]["r1_ohm"]
    for text in ("no subtransient reactance; assumed r = 0.005, x = 0.2 pu",
                 "no zero-sequence data; assumed R0 = 3 R1, X0 = 3 X1",
                 "transformer connections unknown; the zero sequence passes as through YNyn0"):
        assert any(w.startswith(text) for w in conv.warnings), text


def test_the_zero_sequence_magnetizing_reactance_is_used_where_the_fluxes_are_not_free():
    net = eurostag()
    data = {**GENERATORS, **LINES, **CASES["Yyn_YNy"]}
    without = {**data, "NHV2_NLOAD": {k: v for k, v in data["NHV2_NLOAD"].items() if k != "xm"}}
    free = {**data, "NHV2_NLOAD": {**data["NHV2_NLOAD"], "free_fluxes": True}}
    z = {}
    for name, d in (("xm", data), ("without", without), ("free", free)):
        conv = from_powsybl(net, short_circuit_data=d)
        c = conv.compare(openkeito.run(conv.model))
        assert c["max_abs_dv_pu"] < TOL and c["max_abs_dva_degree"] < TOL
        z[name] = scanned(conv)[2][BUSES.index("VLHV2_0")]
        hand = sequence_impedances(net, conv, d)[1][BUSES.index("VLHV2_0")]
        assert abs(z[name] / hand - 1) < BLOCKED, (name, z[name], hand)
    # At the 380 kV bus of the transformer the reactance is behind the ideal
    # ratio rho: it adds rho^2 / (j xm) to the admittance that the bus has
    # with free fluxes (the line capacitances).
    rho = float(net.get_2_windings_transformers(all_attributes=True).at["NHV2_NLOAD", "rho"])
    added = 1 / z["xm"] - 1 / z["free"]
    assert abs(added / (rho ** 2 / 180.0j) - 1) < BLOCKED, added
    # Without xm it is the magnetizing admittance of the positive sequence.
    assert abs((1 / z["without"] - 1 / z["free"]) / (rho ** 2 * complex(2e-6, -3e-5)) - 1) < BLOCKED
    # With free fluxes the reactance does nothing.
    conv = from_powsybl(net, short_circuit_data=free)
    assert conv.model == from_powsybl(net, short_circuit_data={
        **free, "NHV2_NLOAD": {k: v for k, v in free["NHV2_NLOAD"].items() if k != "xm"}}).model


def test_wrong_data_are_refused():
    net = eurostag()
    with pytest.raises(ValueError) as e:
        from_powsybl(net, short_circuit_data={"nope": {"rz": 1.0}, "GEN": {"x0": 1.0},
                                              "NGEN_NHV1": {"connection_type1": "Dyn"}})
    for text in ("'nope' is not the id", "x0 is no field of generators", "connection type Dyn"):
        assert text in str(e.value)


# IIDM extensions ---------------------------------------------------------------

EXTENSIONS = {
    "generator_fortescue/1_0": ("GEN", '<f:generatorFortescue rz="0.01" xz="0.4" grounded="true" '
                                       'groundingR="2.0" groundingX="0.5"/>'),
    "line_fortescue/1_1": ("NHV1_NHV2_1", '<f:lineFortescue rz="12.0" xz="100.0"/>'),
    "two_windings_transformer_fortescue/1_1": (
        "NGEN_NHV1", '<f:twoWindingsTransformerFortescue rz="0.2" xz="10.0" freeFluxes="true" '
                     'connectionType1="DELTA" connectionType2="Y_GROUNDED" groundingX2="3.0"/>'),
}


def with_extensions(net, extensions, tmp_path):
    """The network with Fortescue extensions, which pypowsybl can only take
    from a file: written as XIIDM, the extensions added, read again."""
    text = net.save_to_string("XIIDM")
    for k, (namespace, (eid, body)) in enumerate(extensions.items()):
        text = text.replace("<iidm:network ", f'<iidm:network xmlns:f{k}="http://www.powsybl.org/schema/iidm/ext/'
                                              f'{namespace}" ', 1)
        body = body.replace("<f:", f"<f{k}:").replace("</f:", f"</f{k}:")
        text = text.replace("</iidm:network>", f'<iidm:extension id="{eid}">{body}</iidm:extension></iidm:network>')
    path = tmp_path / "network.xiidm"
    path.write_text(text, encoding="utf-8", newline="\n")
    return pn.load(str(path))


def test_the_extensions_of_a_network_are_read(tmp_path):
    net = with_extensions(eurostag(), EXTENSIONS, tmp_path)
    net.create_extensions("generatorShortCircuit", id="GEN", direct_sub_trans_x=0.9, direct_trans_x=1.4,
                          step_up_transformer_x=0.3)
    net.create_extensions("twoWindingsTransformerPhaseAngleClock", id="NGEN_NHV1", phase_angle_clock=11)
    assert short_circuit_data(net) == {
        "GEN": {"direct_sub_trans_x": 0.9, "direct_trans_x": 1.4, "step_up_transformer_x": 0.3, "rz": 0.01,
                "xz": 0.4, "grounded": True, "grounding_r": 2.0, "grounding_x": 0.5},
        "NHV1_NHV2_1": {"rz": 12.0, "xz": 100.0},
        "NGEN_NHV1": {"phase_angle_clock": 11.0, "rz": 0.2, "xz": 10.0, "free_fluxes": True,
                      "connection_type1": "DELTA", "connection_type2": "Y_GROUNDED", "grounding_x2": 3.0},
    }
    conv = from_powsybl(net, outputs="all", end_time_s=0.02)
    c = conv.compare(openkeito.run(conv.model))
    assert c["max_abs_dv_pu"] < TOL and c["max_abs_dva_degree"] < TOL
    used = conv.mapping["short_circuit"]
    assert used["generator/GEN"]["sources"] == ["generatorShortCircuit", "fortescue"]
    assert used["two_windings_transformer/NGEN_NHV1"]["sources"] == ["twoWindingsTransformerPhaseAngleClock",
                                                                     "fortescue"]
    by_name = {c["name"]: c for c in conv.model["components"]}
    w = 2 * math.pi * 50.0
    # The generator stands for the machine and its step-up transformer;
    # the grounding impedance is three times in the zero sequence.
    z = by_name["gen0"]["impedance"]
    assert z["l1_h"] * w == pytest.approx(0.9 + 0.3) and z["l0_h"] * w == pytest.approx(0.4 + 3 * 0.5)
    assert z["r0_ohm"] == pytest.approx(0.01 + 3 * 2.0)
    assert by_name["line0"]["impedance"]["r0_ohm"] == 12.0
    assert by_name["line0"]["impedance"]["l0_h"] * w == pytest.approx(100.0)
    assert by_name["tr0_z"]["impedance"]["r0_ohm"] == ZERO_SEQUENCE_BLOCK * by_name["tr0_z"]["impedance"]["r1_ohm"]
    bank = by_name["tr0_z0"]
    z0 = bank["rated_voltage_kv"][0] ** 2 / bank["rated_power_mva"]
    assert z0 == pytest.approx(abs(complex(0.2, 10.0 + 3 * 3.0)))
    # The other generator, line and transformer have no data: warnings.
    assert any(w.startswith("no subtransient reactance") and "GEN2" in w and "GEN," not in w
               for w in conv.warnings)
    assert any(w.startswith("no zero-sequence data") and w.endswith("NHV1_NHV2_2") for w in conv.warnings)
    assert any(w.startswith("transformer connections unknown") and w.endswith("NHV2_NLOAD")
               for w in conv.warnings)
    # What the caller gives wins over the extension.
    conv = from_powsybl(net, short_circuit_data={"NHV1_NHV2_1": {"rz": 20.0}})
    assert conv.mapping["short_circuit"]["line/NHV1_NHV2_1"] == {
        "sources": ["fortescue", "short_circuit_data"], "data": {"rz": 20.0, "xz": 100.0}}


def test_a_delta_leg_of_a_three_winding_transformer_grounds_the_star_node():
    net = pn.create_micro_grid_be_network()
    t = net.get_3_windings_transformers(all_attributes=True).iloc[0]
    tid = t.name

    def zero_sequence_admittance(connections, **more):
        data = {tid: {**{f"connection_type{i + 1}": c for i, c in enumerate(connections)}, **more}}
        conv = from_powsybl(net, short_circuit_data=data, outputs="all", end_time_s=0.02)
        result = openkeito.run(conv.model)
        c = conv.compare(result)
        assert not result.warnings and c["max_abs_dv_pu"] < TOL and c["max_abs_dva_degree"] < TOL
        assert not any(w.startswith("transformer connections unknown") and tid in w for w in conv.warnings)
        bus = conv.bus(t["bus1_id"])
        model = dict(conv.model)
        model["frequency_scan"] = {"frequencies_hz": [50.0], "buses": [bus]}
        scan = openkeito.scan(model)
        return 1 / cmath.rect(scan[f"Z0mag({bus})"][0], math.radians(scan[f"Z0deg({bus})"][0])), conv

    # YNyd against Yyy, which is open: the difference of the zero-sequence
    # admittances at the bus of leg 1 is the path through leg 1 to the star
    # node and through the delta of leg 3 to gnd, (z0_1 + z0_3) / k1^2 at
    # the bus (k1: the ratio of leg 1 to the star node), with three times
    # the grounding impedance of winding 1.
    grounded, conv = zero_sequence_admittance(["Y_GROUNDED", "Y", "DELTA"], xz1=30.0, rz1=1.0, grounding_x1=2.0)
    isolated, _ = zero_sequence_admittance(["Y", "Y", "Y"])
    by_name = {c["name"]: c for c in conv.model["components"]}
    k1 = by_name["tr3_0_1"]["rated_voltage_kv"][1] / by_name["tr3_0_1"]["rated_voltage_kv"][0]
    z3 = by_name["tr3_0_3_z"]["impedance"]
    z03 = complex(z3["r1_ohm"], z3["l1_h"] * 2 * math.pi * 50.0)
    expected = 1 / ((complex(1.0, 30.0) + z03) / k1 ** 2 + 3 * 2.0j)
    assert abs((grounded - isolated) / expected - 1) < BLOCKED, (grounded - isolated, expected)
    assert by_name["tr3_0_3_z0"]["buses"][0] == "tr3_0_star"


def test_machines_take_their_reactances_and_star_point_from_the_data():
    net = eurostag()
    z_base = 24.0 ** 2 / 1000.0
    data = {
        "GEN": {"direct_sub_trans_x": 0.2 * z_base, "direct_trans_x": 0.32 * z_base, "direct_sync_x": 2.0 * z_base,
                "stator_r": 0.004 * z_base, "xz": 0.1 * z_base, "grounded": True, "grounding_r": 100.0},
        # A transient reactance below the subtransient one is left out.
        "GEN2": {"direct_sub_trans_x": 0.2 * z_base, "direct_trans_x": 0.15 * z_base, "grounded": False},
    }
    conv = from_powsybl(net, generators="machine", machine_parameters={"rated_power_mva": 1000.0},
                        short_circuit_data=data, end_time_s=0.02)
    gen, gen2 = (c for c in conv.model["components"] if c["type"] == "synchronous_machine")
    for field, value in (("xd_subtransient_pu", 0.2), ("xq_subtransient_pu", 0.2), ("xd_transient_pu", 0.32),
                         ("xd_pu", 2.0), ("ra_pu", 0.004), ("x0_pu", 0.1)):
        assert gen[field] == pytest.approx(value), field
    assert gen["neutral"] == {"type": "grounded", "resistance_ohm": 100.0, "inductance_h": 0.0}
    assert gen2["neutral"] == {"type": "ungrounded"} and gen2["xd_subtransient_pu"] == pytest.approx(0.2)
    assert gen2["xd_transient_pu"] > gen2["xd_subtransient_pu"]
    assert any(w.startswith("direct_trans_x of the data is not above") and w.endswith("GEN2")
               for w in conv.warnings)
    assert not any(w.startswith("typical machine parameters") and "xd_subtransient_pu" in w
                   for w in conv.warnings)
    assert openkeito.validate(conv.model)["valid"]
    c = conv.compare(openkeito.run(conv.model))
    assert c["max_abs_dv_pu"] < TOL and c["max_abs_dva_degree"] < TOL


# CGMES ---------------------------------------------------------------------------

CIM = "http://iec.ch/TC57/2013/CIM-schema-cim16#"


def cgmes_with_short_circuit_attributes(tmp_path):
    """The MicroGrid BE configuration that comes with pypowsybl, written as
    CGMES, with short-circuit attributes added to its equipment file (the
    values are made up) and with the base voltages that the export leaves to
    the boundary files."""
    net = pn.create_micro_grid_be_network()
    plain = tmp_path / "plain.zip"
    net.save(str(plain), format="CGMES")
    with zipfile.ZipFile(plain) as archive:
        files = {name: archive.read(name).decode("utf-8") for name in archive.namelist()}
    name = next(n for n in files if n.endswith("_EQ.xml"))
    eq = files[name]

    def add(kind, lines, text, only=lambda block: True):
        def extend(match):
            if not only(match.group(0)):
                return match.group(0)
            return match.group(1) + "".join(f"<cim:{line}" for line in lines) + match.group(2)
        return re.sub(rf"(<cim:{kind} rdf:ID.*?)(</cim:{kind}>)", extend, text, flags=re.S)

    def values(prefix, **fields):
        return [f"{prefix}.{k}>{v}</cim:{prefix}.{k}>" for k, v in fields.items()]

    eq = add("SynchronousMachine", values(
        "SynchronousMachine", satDirectSubtransX=0.22, satDirectTransX=0.31, satDirectSyncX=1.9, r0=0.004, x0=0.11,
        r2=0.006, x2=0.21, r=0.002, earthing="true", earthingStarPointR=0.5, earthingStarPointX=1.5), eq)
    eq = add("ACLineSegment", values("ACLineSegment", r0=6.1, x0=99.9, b0ch=0.00002, g0ch=0), eq)
    for number, kind, clock, grounded in ((1, "Yn", 0, "true"), (2, "D", 11, "false"), (3, "Yn", 0, "true")):
        eq = add("PowerTransformerEnd", [
            f'PowerTransformerEnd.connectionKind rdf:resource="{CIM}WindingConnection.{kind}"/>',
            *values("PowerTransformerEnd", phaseAngleClock=clock, r0=1.23 if number == 1 else 0,
                    x0=12.3 if number == 1 else 0),
            *values("TransformerEnd", grounded=grounded, rground=0.7, xground=2.1),
        ], eq, only=lambda block, n=number: f"<cim:TransformerEnd.endNumber>{n}<" in block)
    # Base voltages of the boundary files, from the voltage levels.
    nominal = net.get_voltage_levels()["nominal_v"]
    lines = net.get_lines(all_attributes=True)
    defined = set(re.findall(r'<cim:BaseVoltage rdf:ID="_([^"]+)"', eq))
    missing = {}
    for kind, frame in (("VoltageLevel", None), ("ACLineSegment", lines)):
        for block in re.findall(rf'<cim:{kind} rdf:ID.*?</cim:{kind}>', eq, flags=re.S):
            eid = re.search(r'rdf:ID="_([^"]+)"', block).group(1)
            ref = re.search(r'BaseVoltage rdf:resource="#_([^"]+)"', block).group(1)
            if ref in defined:
                continue
            if frame is None and eid in nominal.index:
                missing[ref] = float(nominal[eid])
            elif frame is not None and eid in frame.index:
                missing.setdefault(ref, float(nominal[frame.at[eid, "voltage_level1_id"]]))
    eq = eq.replace("</rdf:RDF>", "".join(
        f'<cim:BaseVoltage rdf:ID="_{ref}"><cim:BaseVoltage.nominalVoltage>{kv}</cim:BaseVoltage.nominalVoltage>'
        "</cim:BaseVoltage>" for ref, kv in missing.items()) + "</rdf:RDF>")
    files[name] = eq
    path = tmp_path / "be_short_circuit.zip"
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, text in files.items():
            archive.writestr(name, text)
    return path


def test_the_short_circuit_attributes_of_a_cgmes_file_are_read(tmp_path):
    path = cgmes_with_short_circuit_attributes(tmp_path)
    net = pn.load(str(path))
    # pypowsybl's import keeps none of them.
    assert short_circuit_data(net) == {}
    read, skipped = cgmes_short_circuit_data(path, net)
    assert not skipped
    gens = net.get_generators(all_attributes=True)
    # (Read again, the boundary injections of the file are generators too.)
    gid = gens.index[gens["name"] == "BE-G1"][0]
    z_base = float(net.get_voltage_levels().at[gens.at[gid, "voltage_level_id"], "nominal_v"]) ** 2 / float(
        gens.at[gid, "rated_s"])
    assert read[gid] == pytest.approx({
        "direct_sub_trans_x": 0.22 * z_base, "direct_trans_x": 0.31 * z_base, "direct_sync_x": 1.9 * z_base,
        "rz": 0.004 * z_base, "xz": 0.11 * z_base, "rn": 0.006 * z_base, "xn": 0.21 * z_base, "stator_r": 0.002,
        "grounded": True, "grounding_r": 0.5, "grounding_x": 1.5})
    lid = net.get_lines().index[0]
    assert read[lid] == {"rz": 6.1, "xz": 99.9, "gz": 0.0, "bz": 2e-5}
    two = net.get_2_windings_transformers(all_attributes=True)
    tid = two.index[0]
    ratio = (float(two.at[tid, "rated_u2"]) / float(two.at[tid, "rated_u1"])) ** 2
    assert read[tid] == pytest.approx({
        "rz": 1.23 * ratio, "xz": 12.3 * ratio, "free_fluxes": True, "phase_angle_clock": 11.0,
        "connection_type1": "Y_GROUNDED", "connection_type2": "DELTA", "grounding_r1": 0.7, "grounding_x1": 2.1})
    three = net.get_3_windings_transformers().index[0]
    assert [read[three][f"connection_type{i}"] for i in (1, 2, 3)] == ["Y_GROUNDED", "DELTA", "Y_GROUNDED"]
    assert read[three]["phase_angle_clock_leg2"] == 11.0 and read[three]["xz1"] == pytest.approx(12.3)

    conv = from_cgmes(path, outputs="all", end_time_s=0.02)
    result = openkeito.run(conv.model)
    c = conv.compare(result)
    assert not result.warnings and c["max_abs_dv_pu"] < TOL and c["max_abs_dva_degree"] < TOL
    used = conv.mapping["short_circuit"]
    assert used[f"generator/{gid}"]["sources"] == ["CGMES"] and used[f"line/{lid}"]["sources"] == ["CGMES"]
    assert not any(w.startswith(("no zero-sequence data", "transformer connections unknown"))
                   for w in conv.warnings), conv.warnings
    assert not any(w.startswith("no subtransient") and (gid in w or "more" in w) for w in conv.warnings)
    # Without them, the file converts as before.
    plain = from_cgmes(path, short_circuit_profile=False)
    assert plain.mapping["short_circuit"] == {}
    assert any(w.startswith("transformer connections unknown") for w in plain.warnings)
    assert plain.model == from_powsybl(pn.load(str(path))).model
    # What the caller gives wins.
    conv = from_cgmes(path, short_circuit_data={lid: {"rz": 7.0}})
    assert conv.mapping["short_circuit"][f"line/{lid}"] == {
        "sources": ["CGMES", "short_circuit_data"], "data": {"rz": 7.0, "xz": 99.9, "gz": 0.0, "bz": 2e-5}}


EQ = """<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="{namespace}" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <cim:SynchronousMachine rdf:ID="_GEN">
    <cim:RotatingMachine.ratedU>24</cim:RotatingMachine.ratedU>
    <cim:RotatingMachine.ratedS>1200</cim:RotatingMachine.ratedS>
    <cim:SynchronousMachine.satDirectSubtransX>0.2</cim:SynchronousMachine.satDirectSubtransX>
    <cim:SynchronousMachine.r>0.003</cim:SynchronousMachine.r>
    <cim:SynchronousMachine.r0>0.01</cim:SynchronousMachine.r0>
    <cim:SynchronousMachine.x0>0.05</cim:SynchronousMachine.x0>
    <cim:SynchronousMachine.r2>0.02</cim:SynchronousMachine.r2>
    <cim:SynchronousMachine.x2>0.09</cim:SynchronousMachine.x2>
    <cim:SynchronousMachine.earthing>true</cim:SynchronousMachine.earthing>
    <cim:SynchronousMachine.earthingStarPointR>0.4</cim:SynchronousMachine.earthingStarPointR>
  </cim:SynchronousMachine>
</rdf:RDF>
"""


@pytest.mark.parametrize("namespace, per_unit", [
    ("http://iec.ch/TC57/2013/CIM-schema-cim16#", True),
    ("http://iec.ch/TC57/CIM100#", False),
])
def test_the_units_of_a_synchronous_machine_are_those_of_the_cgmes_version(tmp_path, namespace, per_unit):
    """The data types of the attributes in the RDF schemas of the profiles
    (ENTSO-E): SynchronousMachine.r is a Resistance and the saturated
    reactances are PU in CGMES 2.4.15 (CIM16) and in CGMES 3.0 (CIM100);
    r0, x0, r2 and x2 are PU in 2.4.15, Resistance and Reactance in 3.0."""
    path = tmp_path / "machine_EQ.xml"
    path.write_text(EQ.format(namespace=namespace), encoding="utf-8", newline="\n")
    read, skipped = cgmes_short_circuit_data(path, eurostag())
    assert not skipped
    z_base = 24.0 ** 2 / 1200.0
    sequence = z_base if per_unit else 1.0
    assert read == {"GEN": pytest.approx({
        "direct_sub_trans_x": 0.2 * z_base, "stator_r": 0.003, "rz": 0.01 * sequence, "xz": 0.05 * sequence,
        "rn": 0.02 * sequence, "xn": 0.09 * sequence, "grounded": True, "grounding_r": 0.4})}


def test_cgmes_dangling_line_shunt_admittance_is_skipped_and_uuid_id_parsed(tmp_path):
    """A4 & B9: an ACLineSegment corresponding to a boundary (dangling) line
    ignores b0ch/g0ch with a warning instead of failing, urn:uuid:_ ids keep
    only one underscore stripped, and partial winding connections keep the
    known side under an odd clock."""
    net = pn.create_micro_grid_be_network()
    dl_id = str(net.get_dangling_lines().index[0])
    xml = f"""<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:cim="http://iec.ch/TC57/2013/CIM-schema-cim16#" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <cim:ACLineSegment rdf:about="urn:uuid:_{dl_id}">
    <cim:ACLineSegment.r0>3.0</cim:ACLineSegment.r0>
    <cim:ACLineSegment.x0>30.0</cim:ACLineSegment.x0>
    <cim:ACLineSegment.b0ch>0.0001</cim:ACLineSegment.b0ch>
  </cim:ACLineSegment>
</rdf:RDF>
"""
    xml_path = tmp_path / "dangling_EQ.xml"
    xml_path.write_text(xml, encoding="utf-8", newline="\n")
    read, skipped = cgmes_short_circuit_data(xml_path, net)
    assert read[dl_id] == {"rz": 3.0, "xz": 30.0}
    assert any(dl_id in s and "b0ch" in s for s in skipped)

    # Full conversion with those CGMES attributes succeeds (no ValueError on bz).
    from openkeito.powsybl import with_cgmes_short_circuit_data

    opts, _, notes = with_cgmes_short_circuit_data(xml_path, net, {})
    conv = from_powsybl(net, **opts)
    assert conv.mapping["short_circuit"][f"dangling_line/{dl_id}"]["data"] == {"rz": 3.0, "xz": 30.0}
    assert any("b0ch" in n for n in notes)

    # B9: urn:uuid: prefix and only a single leading underscore are stripped.
    from openkeito.powsybl import _cgmes_objects

    u_path = tmp_path / "uuid_EQ.xml"
    u_path.write_text(
        '<rdf:RDF xmlns:cim="http://iec.ch/TC57/CIM100#" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">'
        '<cim:SynchronousMachine rdf:about="urn:uuid:__KEEP_ONE"><cim:SynchronousMachine.r>0.1</cim:SynchronousMachine.r>'
        "</cim:SynchronousMachine></rdf:RDF>",
        encoding="utf-8",
    )
    objs = _cgmes_objects(u_path, {"SynchronousMachine"})
    assert "_KEEP_ONE" in objs["SynchronousMachine"]

    # D4: one known winding connection is preserved under an odd phase-angle clock.
    eu = eurostag()
    tid = "NGEN_NHV1"
    conv_odd = from_powsybl(eu, phase_angle_clocks="rotate", short_circuit_data={
        tid: {"connection_type1": "Y_GROUNDED", "phase_angle_clock": 11},
    })
    assert any("transformer connections unknown" in w and tid in w for w in conv_odd.warnings)
    # Side 1 was Y_GROUNDED (not overwritten to DELTA), so side 2 was inferred as DELTA (bank on side 1).
    assert any(c["name"] == "tr0_z0" for c in conv_odd.model["components"])


def test_cgmes_xml_with_doctype_or_entity_is_rejected(tmp_path):
    """S2: CGMES XML files containing <!DOCTYPE or <!ENTITY are rejected before parsing."""
    from openkeito.powsybl import _cgmes_objects

    for decl in ('<!DOCTYPE rdf:RDF [<!ENTITY xxe "boom">]>', '<!ENTITY xxe "boom">'):
        p = tmp_path / "dtd.xml"
        p.write_text(f'<?xml version="1.0"?>{decl}<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/>',
                     encoding="utf-8")
        with pytest.raises(ValueError, match="DOCTYPE|ENTITY"):
            _cgmes_objects(p, {"SynchronousMachine"})
