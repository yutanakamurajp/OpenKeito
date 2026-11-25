"""Conversion of a PowSyBl (IIDM) network, such as one read from CGMES, and its
AC load flow into an OpenKeito model.

The model starts from the steady state of the load flow, as with
``openkeito.pandapower`` (see there and docs/numerical-method.md): sources
behind an impedance with E = V + Z I, constant impedances at the load-flow
voltage, current injections I = conj(S / 3V). The branches follow IIDM's
equivalent circuits:

- line: series R + jX with the shunt admittances g1 + jb1 and g2 + jb2 at its
  two ends (``coupled_rl`` and ``three_phase_load``);
- two-winding transformer: an ideal transformer with the complex ratio
  rho e^{j alpha} at side 1 (``three_phase_ideal_transformer``), the
  magnetizing admittance g + jb at its side-2 terminal and the series
  R + jX to side 2, all at the current tap;
- three-winding transformer: three such legs from the network buses to a
  star node at rated_u0;
- dangling line: the shunt g + jb at the network bus, the series R + jX to a
  boundary node, and the boundary's p0 + jq0 there.

IIDM's load flow does not turn the voltages by the phase-angle clocks of the
transformers (a clock is an extension, not a part of the equivalent circuit),
and by default neither does the model: behind a Yd transformer the phase
currents of an unbalanced fault are then those behind a Yy one. With
``phase_angle_clocks="rotate"`` the ideal transformers turn by the clocks and
the load-flow voltages of every zone between transformers are turned with
them (see ``from_powsybl``).

A branch that is open at one end stays energized from the other, as in the
load flow: its open end is a node of its own. A branch without a connected
end is left out, as are the buses without a load-flow voltage (a warning
names them).

The bus with IIDM id ``k`` (bus view) becomes ``b0``, ``b1``, ... in the order
of ``network.get_buses()``; ``Conversion.mapping`` gives both directions of
the names, and the element ids (the CGMES mRIDs) of every component.

    import pypowsybl as pp
    from openkeito.powsybl import from_powsybl
    conv = from_powsybl(pp.network.create_micro_grid_be_network())
    conv.compare(openkeito.run(conv.model))["max_abs_dv_pu"]
"""

from __future__ import annotations

import cmath
import math
from collections import defaultdict
from typing import Any

from .convert import (
    Conversion,
    ConversionError,
    TimeSlice,
    TimeSliceRun,
    check_generators,
    check_inverter_model,
    consumption_components,
    finite,
    grid_following_converter,
    machine_rating,
    model_document,
    run_slices,
    schedule_slices,
    source_component,
    split_machine_parameters,
    summarize,
    synchronous_machine,
)

__all__ = [
    "UNSUPPORTED", "KIND_QUANTITIES", "apply_schedule", "check_schedule", "from_cgmes", "from_powsybl", "imbalance",
    "run_time_slices", "tight_load_flow_parameters", "time_slices",
    "SHORT_CIRCUIT_FIELDS", "ZERO_SEQUENCE_BLOCK", "cgmes_short_circuit_data", "short_circuit_data",
    "PHASE_ANGLE_CLOCKS",
]

#: Element kinds the converter cannot represent yet.
UNSUPPORTED = ["hvdc_lines", "vsc_converter_stations", "lcc_converter_stations", "tie_lines"]


#: What ``phase_angle_clocks`` of ``from_powsybl`` can be.
PHASE_ANGLE_CLOCKS = ("ignore", "rotate")


def from_cgmes(path: str, *, short_circuit_profile: bool = True, **options: Any) -> Conversion:
    """Reads a CGMES file (or zip) with pypowsybl and converts it; see
    ``from_powsybl`` for the options.

    short_circuit_profile: also read the short-circuit attributes of the
    equipment profile from the file (``cgmes_short_circuit_data``), which
    pypowsybl's import leaves out; ``short_circuit_data`` in the options
    wins over them."""
    import pypowsybl.network as pn

    network = pn.load(str(path))
    if not short_circuit_profile:
        return from_powsybl(network, **options)
    given = options.get("short_circuit_data")
    options, read, notes = with_cgmes_short_circuit_data(path, network, options)
    conv = from_powsybl(network, **options)
    label_cgmes_short_circuit_data(conv, read, given)
    conv.warnings.extend(notes)
    return conv


def with_cgmes_short_circuit_data(path: Any, network: Any, options: dict[str, Any]
                                  ) -> tuple[dict[str, Any], dict[str, dict[str, Any]], list[str]]:
    """(the options of ``from_powsybl`` with the short-circuit attributes of
    the CGMES files at ``path`` under their ``short_circuit_data``, which
    wins; what was read; the warning about what was left out, if any). A
    path that holds no such attributes (another format, no file) leaves the
    options as they are."""
    read, skipped = cgmes_short_circuit_data(path, network)
    notes = ["short-circuit attributes of the CGMES file left out: " + "; ".join(skipped)] if skipped else []
    if not read:
        return options, read, notes
    given = options.get("short_circuit_data") or {}
    merged = {eid: {**read.get(eid, {}), **given.get(eid, {})} for eid in [*read, *(e for e in given if e not in read)]}
    return {**options, "short_circuit_data": merged}, read, notes


def label_cgmes_short_circuit_data(conv: Conversion, read: dict[str, dict[str, Any]],
                                   given: dict[str, Any] | None = None) -> None:
    """Names "CGMES" as the source, in ``conv.mapping["short_circuit"]``, of
    the elements whose data ``with_cgmes_short_circuit_data`` read (before
    "short_circuit_data" for those that the caller's ``given`` data also
    name)."""
    for key, entry in conv.mapping["short_circuit"].items():
        eid = key.split("/", 1)[1]
        if eid in read and "short_circuit_data" in entry["sources"]:
            at = entry["sources"].index("short_circuit_data")
            entry["sources"][at:at + 1] = ["CGMES"] + (["short_circuit_data"] if eid in (given or {}) else [])


def _clock_connections(tid: str, hours: list[int], known: list[str | None],
                       rated: list[float], assumed: dict[str, list[str]]) -> list[str | None]:
    """The connections of the windings of a transformer that the clocks
    turn: as they are known, or, without them and with an odd clock,
    those that such a clock needs (two windings: Dyn with the delta at
    the higher rated voltage; three: a delta at every leg with an odd
    clock against grounded stars). Warns where the zero sequence
    passes between windings that the clock turns against each other."""
    if not any(hours):
        return known
    if None in known and any(h % 2 for h in hours):
        delta_hour = next((h % 2 for h, c in zip(hours, known) if c == "DELTA"), None)
        if delta_hour is None:
            star_hour = next((h % 2 for h, c in zip(hours, known) if c in ("Y", "Y_GROUNDED")), None)
            if star_hour is not None:
                delta_hour = 1 - star_hour
            elif len(hours) == 2:
                delta_hour = hours[0 if rated[0] >= rated[1] else 1] % 2
            else:
                delta_hour = 1
        known = [c if c is not None else ("DELTA" if h % 2 == delta_hour else "Y_GROUNDED")
                 for h, c in zip(hours, known)]
        assumed["transformer connections unknown; with an odd phase-angle clock taken as a delta (two "
                "windings: at the higher rated voltage) and grounded stars, without grounding "
                "impedance"].append(tid)
    grounded_hours = {h for h, c in zip(hours, known) if c in (None, "Y_GROUNDED")}
    # YNyn6 (all connections known, grounded stars only at clocks 0 and 6)
    # reverses the zero sequence explicitly with zero_sequence_inverted.
    if len(grounded_hours) > 1 and not (None not in known and grounded_hours == {0, 6}):
        assumed["the zero sequence passes between grounded stars that the phase-angle clock turns against "
                "each other as through YNyn0 (a YNyn6 transformer reverses it; the positive and the "
                "negative sequence are turned)"].append(tid)
    return known


def _neutral_of(row: dict[str, Any]) -> tuple[bool, complex] | None:
    """(grounded, grounding impedance in ohm) of a generator with data
    on its star point or zero sequence; None without."""
    if not any(key in row for key in ("rz", "xz", "grounded", "grounding_r", "grounding_x")):
        return None
    return bool(row.get("grounded", True)), complex(row.get("grounding_r", 0.0), row.get("grounding_x", 0.0))


def _series(cname: str, a: str, b: str, z: complex, zero_factor: float, w: float,
            z0: complex | None = None) -> dict[str, Any]:
    """Series impedance z with the zero-sequence impedance z0 (default zero_factor z)."""
    r0, x0 = (zero_factor * z.real, zero_factor * z.imag) if z0 is None else (z0.real, z0.imag)
    return {"type": "coupled_rl", "name": cname, "buses": [a, b],
            "impedance": {"type": "sequence", "r1_ohm": z.real, "l1_h": z.imag / w,
                          "r0_ohm": r0, "l0_h": x0 / w}}


def _bank(cname: str, node: str, u_kv: float, z0: complex, f_hz: float,
          problems: list[str]) -> list[dict[str, Any]]:
    """A path to gnd for the zero sequence only, of impedance z0 (ohm at
    the voltage level of the node): a YNd bank without magnetizing branch."""
    if z0.imag <= 0:
        problems.append(f"{cname}: zero-sequence reactance {z0.imag} ohm is not > 0")
        return []
    return [{"type": "three_phase_transformer", "name": cname,
             "buses": [node, ["gnd", f"{cname}.b", f"{cname}.c"]], "vector_group": "YNd11",
             "rated_voltage_kv": [u_kv, u_kv], "rated_power_mva": u_kv ** 2 / abs(z0),
             "rated_frequency_hz": f_hz, "impedance_percent": 100.0,
             "resistance_percent": 100.0 * max(z0.real, 0.0) / abs(z0)}]


def _zero_of_leg(conn: str | None, other: list[str | None], z: complex, z0: complex | None, zg: complex,
                 k2: float, free: bool) -> dict[str, Any] | None:
    """How the zero sequence passes the leg of a transformer."""
    if conn is None or any(c is None for c in other):
        return None if z0 is None else {"series": z0, "y_grounded": True, "bank": None}
    own = (z if z0 is None else z0) + 3 * zg * k2
    grounded = conn == "Y_GROUNDED" and not free
    if conn == "Y_GROUNDED" and len(other) == 1 and other[0] == "DELTA":
        return {"series": ZERO_SEQUENCE_BLOCK * z, "y_grounded": grounded, "bank": own}
    if conn == "Y_GROUNDED" and (len(other) > 1 or other[0] == "Y_GROUNDED"):
        return {"series": own, "y_grounded": grounded, "bank": None}
    return {"series": ZERO_SEQUENCE_BLOCK * z, "y_grounded": grounded, "bank": None}


def _winding(row: dict[str, Any], i: int, single: bool = False) -> tuple[str | None, complex, complex | None, bool]:
    """(connection, grounding impedance, zero-sequence impedance, free fluxes)
    of winding i of a transformer from its short-circuit data."""
    conn = row.get(f"connection_type{i}")
    zg = complex(row.get(f"grounding_r{i}", 0.0), row.get(f"grounding_x{i}", 0.0))
    s = "" if single else i
    rz, xz = row.get(f"rz{s}"), row.get(f"xz{s}")
    z0 = complex(rz, xz) if rz is not None and xz is not None and xz > 0 else None
    return conn, zg, z0, bool(row.get(f"free_fluxes{s}", True))


def from_powsybl(
    network: Any,
    *,
    frequency_hz: float = 50.0,
    time_step_s: float = 5e-5,
    end_time_s: float = 0.1,
    method: str = "2s-dirk",
    outputs: list[dict[str, Any]] | str = "buses",
    output_interval_steps: int = 1,
    run_load_flow: bool = True,
    load_flow_parameters: Any = None,
    generator_x_pu: float = 0.2,
    generator_r_pu: float = 0.005,
    generator_rated_mva: float = 100.0,
    generators: str = "source",
    machine_parameters: dict[str, Any] | None = None,
    name: str | None = None,
    inverter_model: str = "current_source",
    converter_generators: Any = (),
    converter_parameters: dict[str, Any] | None = None,
    short_circuit_data: dict[str, dict[str, Any]] | None = None,
    phase_angle_clocks: str = "ignore",
) -> Conversion:
    """Converts ``network`` and its AC load flow into an OpenKeito model.

    run_load_flow: run ``pypowsybl.loadflow.run_ac`` first (default) with
    ``load_flow_parameters`` (default: ``tight_load_flow_parameters()``,
    which keeps the generators' set points and puts the slack at the largest
    generator; when the load flow does not converge with them, pass others,
    such as ``pypowsybl.loadflow.Parameters()``). With False, the network
    must hold a solved load flow (bus voltages and terminal flows). Whatever the load flow leaves unbalanced
    at a bus goes to a generator there (or to an injection ``balance<k>``),
    so that the load-flow voltages solve the model exactly; the warnings
    give the amounts.
    outputs: "buses" records every bus voltage, "all" also the currents of
    sources, lines and transformers; or a list of model outputs.
    generator_x_pu, generator_r_pu: internal impedance of generators without
    short-circuit data, per unit of their rated_s (generator_rated_mva when
    missing) and the nominal voltage.
    generators: what the generators become: "source" (default), a
    three-phase source behind the subtransient impedance, or "machine", a
    synchronous_machine with its rotor windings and inertia, which starts
    from the load flow with a constant field voltage and torque.
    machine_parameters: parameters of the machines (fields of
    synchronous_machine, such as {"inertia_h_s": 6.0}); an entry
    {"generator/<id>": {...}} or {"<id>": {...}} holds those of one generator.
    What neither they nor the network give (``rated_s``, and from the extension
    generatorShortCircuit the subtransient and transient reactances) is
    taken from ``openkeito.convert.TYPICAL_MACHINE`` and named in the
    warnings; a generator without ``rated_s`` gets the rating that its
    output (or ``max_p``, unless that is more than ten times the output)
    loads to 85 %.
    inverter_model: what batteries become. "current_source" (default): a
    constant impedance when they charge, a current injection when they
    discharge. "converter": the averaged grid-following converter
    (``grid_following_converter``), rated at the larger of |min_p| and
    max_p (when missing, or 9999 and more: its apparent power / 0.9, with a
    warning) and the
    nominal voltage of its bus; the model then has format version 0.5.
    converter_generators: ids of generators that are inverters (wind, solar)
    and become such converters in place of a voltage source behind an
    impedance, rated at rated_s (when missing: their apparent power / 0.9,
    with a warning). They do not hold the voltage:
    another generator must take what the load flow leaves unbalanced.
    converter_parameters: more fields of every such converter, for example
    {"current_limit_pu": 1.2, "priority": "active"}.
    short_circuit_data: {element id: {field: value}}, the data that a load
    flow does not need (subtransient reactances, zero-sequence impedances,
    winding connections and grounding), for networks whose extensions do not
    hold them. The fields are those of ``SHORT_CIRCUIT_FIELDS``, with the
    names and units of the IIDM extensions (ohm and siemens); a value given
    here wins over the extension. See ``short_circuit_data`` for what is
    read from the network itself, and ``from_cgmes`` for CGMES files.
    ``Conversion.mapping["short_circuit"]`` lists what was used.
    phase_angle_clocks: what the phase-angle clocks of the transformers (the
    extensions twoWindingsTransformerPhaseAngleClock and
    threeWindingsTransformerPhaseAngleClock, PowerTransformerEnd.phaseAngleClock
    of a CGMES file, or ``phase_angle_clock`` in ``short_circuit_data``) do.
    "ignore" (default): nothing, as in IIDM's load flow; every transformer
    has the clock 0, and a warning names those with another. "rotate": side
    2 of a two-winding transformer lags side 1 by clock * 30 degrees in the
    positive sequence and leads it by as much in the negative sequence (leg
    2 and leg 3 of a three-winding transformer against leg 1), as in a
    transformer with that vector group and as ``openkeito.pandapower`` has
    it. The network falls into zones that the transformers separate; the
    zone of the bus with the highest nominal voltage keeps the angles of
    the load flow, and the others are turned by the clocks on the way to
    them, so that the turned load flow is still the steady state of the
    model. ``Conversion.power_flow`` then holds the turned angles and
    ``Conversion.mapping["bus_phase_shift_deg"]`` what was added at each
    bus. Clocks that do not add up to the same angle on every way to a bus
    (parallel transformers with different clocks) are refused. A phase tap
    changer adds its angle to the clock. Where the connections are not
    known, an odd clock is taken as Dyn (the delta at the higher rated
    voltage) and an even one as YNyn, as ``openkeito.pandapower`` does.
    """
    import pypowsybl.loadflow as lf

    check_generators(generators)
    if phase_angle_clocks not in PHASE_ANGLE_CLOCKS:
        raise ValueError(f"phase_angle_clocks must be one of {', '.join(PHASE_ANGLE_CLOCKS)}, not "
                         f"{phase_angle_clocks!r}")
    rotate = phase_angle_clocks == "rotate"
    machine_common, machine_own = split_machine_parameters(machine_parameters)
    check_inverter_model(inverter_model, converter_parameters)
    converter_generators = set(converter_generators)
    assumed: dict[str, list[str]] = defaultdict(list)
    assumed_ratings: dict[str, dict[str, Any]] = {}
    sc_rows, sc_sources = _short_circuit_rows(network, short_circuit_data)
    used: dict[str, dict[str, Any]] = {}

    def data(kind: str, eid: Any) -> dict[str, Any]:
        """The short-circuit data of an element; noted as used."""
        row = sc_rows.get(str(eid), {})
        if row:
            used[f"{kind}/{eid}"] = {"sources": sc_sources[str(eid)], "data": dict(row)}
        return row

    if run_load_flow:
        results = lf.run_ac(network, load_flow_parameters or tight_load_flow_parameters())
        failed = [r for r in results if "CONVERGED" not in str(r.status)]
        if failed and len(failed) == len(results):
            how = ("pass other load_flow_parameters" if load_flow_parameters is not None else
                   "the default load_flow_parameters are tight_load_flow_parameters(), without a distributed "
                   "slack; pass others, such as load_flow_parameters=pypowsybl.loadflow.Parameters()")
            raise ConversionError([f"the AC load flow did not converge "
                                   f"({', '.join(_status(r) for r in failed)}); {how}"])
        for r in failed:
            assumed["the AC load flow did not converge for a part of the network, which is left out"].append(
                f"connected component {r.connected_component_num} ({_status(r)})")

    problems: list[str] = []
    for kind in UNSUPPORTED:
        frame = getattr(network, f"get_{kind}")()
        if len(frame):
            problems.append(f"{kind}: {len(frame)} element(s); not supported yet")
    if problems:
        raise ConversionError(problems)

    f_hz = float(frequency_hz)
    w = 2 * math.pi * f_hz

    nominal = network.get_voltage_levels()["nominal_v"].to_dict()
    buses = network.get_buses()
    live = [b for b, r in buses.iterrows() if finite(r["v_mag"]) and finite(r["v_angle"])]
    if not live:
        raise ConversionError([
            "no bus has a load-flow voltage: " +
            ("the load flow solved no part of the network (it needs a generator that regulates the voltage)"
             if run_load_flow else "run the load flow first, or convert with run_load_flow=True")])
    dead = sorted(set(map(str, buses.index)) - set(live))
    bus_name = {b: f"b{k}" for k, b in enumerate(live)}
    vn = {b: float(nominal[buses.at[b, "voltage_level_id"]]) for b in live}
    # The phase-angle clocks, and the angle that they add to the load-flow
    # voltage of every bus (0 unless the clocks turn the model).
    clocks, clock_problems = _clocks(network, sc_rows)
    if rotate and clock_problems:
        raise ConversionError(clock_problems)
    if rotate:
        shift, clock_problems = _clock_shifts(network, live, vn, clocks)
        if clock_problems:
            raise ConversionError(clock_problems)
    else:
        shift = dict.fromkeys(live, 0.0)
        named = sorted(tid for tid, hours in clocks.items() if any(hours))
        if named:
            assumed["phase-angle clocks are not applied, as in the load flow: behind these transformers the "
                    "phase currents of an unbalanced fault are those of the clock 0 (convert with "
                    "phase_angle_clocks='rotate' to turn the model by them)"] += named
        clocks = {}
    pf = {b: (float(buses.at[b, "v_mag"]) / vn[b], float(buses.at[b, "v_angle"]) + shift[b], vn[b]) for b in live}

    def v_ll(b: str) -> complex:
        """Line-to-line voltage phasor of a bus, kV (turned by the clocks
        on the way to its zone)."""
        return cmath.rect(float(buses.at[b, "v_mag"]), math.radians(float(buses.at[b, "v_angle"]) + shift[b]))

    def clock_connections(tid: str, hours: list[int], known: list[str | None],
                          rated: list[float]) -> list[str | None]:
        return _clock_connections(tid, hours, known, rated, assumed)

    components: list[dict[str, Any]] = []
    elements: list[dict[str, Any]] = []

    def add(kind: str, eid: str, label: Any, comps: list[dict[str, Any]], at: list[str]) -> None:
        components.extend(comps)
        entry: dict[str, Any] = {
            "element": f"{kind}/{eid}",
            "name": label if isinstance(label, str) else "",
            "component": comps[0]["name"],
            "type": comps[0]["type"],
            "buses": [bus_name[b] for b in at if b in bus_name],
        }
        if len(comps) > 1:
            entry["components"] = [c["name"] for c in comps]
        elements.append(entry)

    def source(cname: str, bus: str, s_mva: complex, z_ohm: complex, z0_ohm: complex | None = None,
               neutral: str | None = None) -> dict[str, Any]:
        """A source behind z_ohm that delivers s_mva at the load flow of its bus."""
        return source_component(cname, bus_name[bus], v_ll(bus) * 1e3 / math.sqrt(3), s_mva, z_ohm, f_hz,
                                z0_ohm=z0_ohm, neutral=neutral)

    def machine(cname: str, gid: str, g: Any, bus: str, s_mva: complex, row: dict[str, Any]) -> dict[str, Any]:
        """A generator as a synchronous machine at the load flow of its bus."""
        own = {**machine_own.get(str(gid), {}), **machine_own.get(f"generator/{gid}", {})}
        chosen = {**machine_common, **own}
        rated, assumed_rating = machine_rating(chosen.pop("rated_power_mva", g.get("rated_s")), s_mva,
                                               g.get("max_p"))
        if assumed_rating:
            assumed["no rated_s; assumed that the output of the load flow (or max_p) is 85 % of "
                    "it"].append(f"{gid} ({rated:.4g} MVA)")
        z_base = vn[bus] ** 2 / rated
        # The data are in ohm. A reactance that contradicts the order
        # xd > xd' > xd'' of a machine is left out: the subtransient one,
        # which decides the first cycles, is kept.
        read = {}
        lowest = 0.0
        for key, field in (("direct_sub_trans_x", "xd_subtransient_pu"), ("direct_trans_x", "xd_transient_pu"),
                           ("direct_sync_x", "xd_pu")):
            x = row.get(key)
            if x is None or not x > 0:
                continue
            if x / z_base <= lowest:
                assumed[f"{key} of the data is not above the reactance of the faster time range (xd > xd' > "
                        "xd'' must hold); left out"].append(str(gid))
                continue
            read[field] = lowest = x / z_base
        if "xd_subtransient_pu" in read:
            read["xq_subtransient_pu"] = read["xd_subtransient_pu"]
        if assumed_rating:
            assumed_ratings[str(gid)] = {"component": cname, "xd_from_ohm": "xd_pu" in read}
        if row.get("stator_r", -1.0) >= 0:
            read["ra_pu"] = row["stator_r"] / z_base
        if row.get("xz", 0.0) > 0:
            read["x0_pu"] = row["xz"] / z_base
        star = _neutral_of(row)
        if star is not None:
            grounded, zg = star
            chosen = {"neutral": {"type": "grounded", "resistance_ohm": zg.real, "inductance_h": zg.imag / w}
                      if grounded else {"type": "ungrounded"}, **chosen}
        if row.get("step_up_transformer_x", 0.0) > 0:
            assumed["step_up_transformer_x is not represented with generators='machine' (the machine is at "
                    "the bus of the network)"].append(str(gid))
        v = v_ll(bus)
        component, notes = synchronous_machine(
            cname, bus_name[bus], frequency_hz=f_hz, rated_power_mva=rated, rated_voltage_kv=vn[bus],
            s_mva=s_mva, voltage_pu=abs(v) / vn[bus], angle_deg=math.degrees(cmath.phase(v)), data=read,
            parameters=chosen)
        for note in notes:
            assumed[note].append(str(gid))
        return component

    def consumption(cname: str, node: str, v_kv: complex, s_mva: complex,
                    neutral: str | None = None) -> list[dict[str, Any]]:
        return consumption_components(
            cname, node, abs(v_kv), v_kv * 1e3 / math.sqrt(3), s_mva, f_hz, assumed, cname,
            "negative active power (or conductance), represented as a current injection", neutral=neutral)

    def shunt(cname: str, node: str, v_kv: complex, y_s: complex, y0_s: complex | None = None,
              grounded: bool = True) -> list[dict[str, Any]]:
        """Admittance y_s (siemens, per phase to ground in the line-to-line
        convention S = |V|^2 conj(Y)) at a node of voltage v_kv. y0_s: its
        zero-sequence admittance (default y_s): the star y0_s to gnd and the
        star y_s - y0_s to a star point of its own, which add up to y_s in
        the positive sequence. grounded=False: no zero-sequence admittance."""
        if not grounded:
            y0_s = 0j
        if y0_s is None or y0_s == y_s:
            return consumption(cname, node, v_kv, abs(v_kv) ** 2 * y_s.conjugate())
        return (consumption(cname, node, v_kv, abs(v_kv) ** 2 * y0_s.conjugate()) +
                consumption(f"{cname}_u" if y0_s != 0 else cname, node, v_kv,
                            abs(v_kv) ** 2 * (y_s - y0_s).conjugate(), neutral=f"{cname}_n"))

    def series(cname: str, a: str, b: str, z: complex, zero_factor: float,
               z0: complex | None = None) -> dict[str, Any]:
        return _series(cname, a, b, z, zero_factor, w, z0)

    def bank(cname: str, node: str, u_kv: float, z0: complex) -> list[dict[str, Any]]:
        return _bank(cname, node, u_kv, z0, f_hz, problems)

    def leg(cname: str, bus: str, end: str | None, k: complex, u1: float, z: complex,
            y: complex, zero: dict[str, Any] | None = None, *, zero_inverted: bool = False) -> list[dict[str, Any]]:
        """IIDM transformer (leg): ideal ratio k at the bus, the admittance y
        at its other terminal, the series z to `end` (None: that end is
        open, and without y nothing is left). zero: how the zero sequence
        passes (``_zero_of_leg``; default: as the positive sequence)."""
        if z.imag <= 0:
            problems.append(f"{cname}: series reactance {z.imag} ohm is not > 0")
            return []
        m = f"{cname}_m"
        zero = zero or {"series": None, "y_grounded": True, "bank": None}
        ideal: dict[str, Any] = {
            "type": "three_phase_ideal_transformer", "name": cname, "buses": [bus_name[bus], m],
            "rated_voltage_kv": [u1, abs(k) * u1], "phase_shift_deg": math.degrees(cmath.phase(k)),
        }
        if zero_inverted:
            ideal["zero_sequence_inverted"] = True
        out = [ideal]
        out += shunt(f"{cname}_y", m, k * v_ll(bus), y, zero.get("y0"), grounded=zero["y_grounded"])
        if end is not None:
            out.append(series(f"{cname}_z", m, end, z, 1.0, zero["series"]))
        if zero["bank"] is not None:
            out += bank(f"{cname}_z0", m, abs(k) * u1, zero["bank"])
        return out if len(out) > 1 else []

    def open_leg(cname: str, end: str, v_end: complex, z: complex, y: complex,
                 zero: dict[str, Any] | None = None) -> list[dict[str, Any]]:
        """The same with the bus side open: from `end` (at the voltage
        v_end), the series z to the admittance y. The ideal transformer
        carries nothing and is left out, as is all of it without y."""
        if z.imag <= 0:
            problems.append(f"{cname}: series reactance {z.imag} ohm is not > 0")
            return []
        if y == 0:
            return []
        m = f"{cname}_m"
        zero = zero or {"series": None, "y_grounded": True, "bank": None}
        return ([series(f"{cname}_z", m, end, z, 1.0, zero["series"])] +
                shunt(f"{cname}_y", m, v_end / (1 + z * y), y, zero.get("y0"), grounded=zero["y_grounded"]))

    winding = _winding
    zero_of_leg = _zero_of_leg
    neutral_of = _neutral_of

    def connected(*ids: Any) -> bool:
        return all(isinstance(i, str) and i in bus_name for i in ids)

    # What the load flow left unbalanced at each bus goes to a generator
    # there, so that the load-flow voltages solve the model exactly.
    residual = imbalance(network)
    balanced: set[str] = set()

    # Generators: the load flow's terminal p, q are consumed (load sign).
    gens = network.get_generators(all_attributes=True)
    if machine_own:
        known_gens = {str(gid) for gid in gens.index} | {f"generator/{gid}" for gid in gens.index}
        unknown_gens = sorted(set(machine_own) - known_gens)
        if unknown_gens:
            raise ValueError(f"machine_parameters names no generator in the network: {', '.join(unknown_gens)}")
    for k, (gid, g) in enumerate(gens.iterrows()):
        if not connected(g["bus_id"]) or not finite(g["p"]):
            continue
        b = g["bus_id"]
        s = -complex(g["p"], g["q"] if finite(g["q"]) else 0.0)
        if gid in converter_generators:
            converter_generators.discard(gid)
            add("generator", gid, g.get("name"), [grid_following_converter(
                f"gen{k}", bus_name[b], s, v_ll(b), f_hz, _rating(g, "rated_s"), vn[b],
                converter_parameters, assumed, gid)], [b])
            continue
        if b not in balanced:
            balanced.add(b)
            r = residual.get(b, 0j)
            if abs(r) > 1e-6:
                assumed["the generator takes what the load flow left unbalanced at its bus (as the slack: "
                        "losses and set-point imbalance)"].append(
                    f"{gid} ({r.real:+.2e} MW, {r.imag:+.2e} Mvar)")
            s += r
        row = data("generator", gid)
        if generators == "machine":
            add("generator", gid, g.get("name"), [machine(f"gen{k}", gid, g, b, s, row)], [b])
            continue
        rated = float(g["rated_s"]) if finite(g.get("rated_s")) and g["rated_s"] > 0 else generator_rated_mva
        z_base = vn[b] ** 2 / rated
        r = row["stator_r"] if row.get("stator_r", -1.0) >= 0 else generator_r_pu * z_base
        if row.get("direct_sub_trans_x", 0.0) > 0:
            z = complex(r, row["direct_sub_trans_x"])
        elif row.get("direct_trans_x", 0.0) > 0:
            z = complex(r, row["direct_trans_x"])
            assumed["no subtransient reactance; the transient reactance of the data is used in its "
                    "place"].append(gid)
        else:
            z = complex(generator_r_pu, generator_x_pu) * z_base
            assumed[f"no subtransient reactance; assumed r = {generator_r_pu:g}, x = {generator_x_pu:g} pu "
                    "on rated_s"].append(gid)
        if not (finite(g.get("rated_s")) and g["rated_s"] > 0):
            assumed[f"no rated_s; assumed {generator_rated_mva:g} MVA"].append(gid)
        if row.get("step_up_transformer_x", 0.0) > 0:
            # The generator stands for the machine and its transformer.
            z += 1j * row["step_up_transformer_x"]
        z0, star_point = None, None
        star = neutral_of(row)
        if star is not None:
            grounded, zg = star
            z0 = complex(row.get("rz", z.real), row["xz"] if row.get("xz", 0.0) > 0 else z.imag)
            if grounded:
                z0 += 3 * zg
            else:
                star_point = f"gen{k}_n"
        if "rn" in row or "xn" in row:
            assumed["the negative-sequence impedance of the data (rn, xn) is not represented: a source has "
                    "Z2 = Z1"].append(gid)
        add("generator", gid, g.get("name"), [source(f"gen{k}", b, s, z, z0, star_point)], [b])
    if converter_generators:
        raise ConversionError([
            "converter_generators names no generator with a load-flow result at a bus with a voltage: " +
            ", ".join(sorted(map(str, converter_generators)))])
    if not components:
        # Without a source the model has no steady state.
        raise ConversionError([
            "no generator has a load-flow result (terminal p) at a bus with a voltage: " +
            ("the model would have no source" if run_load_flow else
             "the network holds no solved load flow; run it first, or convert with run_load_flow=True")])

    # Loads, batteries, shunt compensators, static var compensators.
    for kind, prefix, frame in [
        ("load", "load", network.get_loads(all_attributes=True)),
        ("battery", "battery", network.get_batteries(all_attributes=True)),
        ("shunt_compensator", "shunt", network.get_shunt_compensators(all_attributes=True)),
        ("static_var_compensator", "svc", network.get_static_var_compensators(all_attributes=True)),
    ]:
        for k, (eid, e) in enumerate(frame.iterrows()):
            if not connected(e["bus_id"]) or not finite(e["p"]):
                continue
            b = e["bus_id"]
            s = complex(e["p"], e["q"] if finite(e["q"]) else 0.0)
            if kind == "battery" and inverter_model == "converter" and s != 0:
                add(kind, eid, e.get("name"), [grid_following_converter(
                    f"{prefix}{k}", bus_name[b], -s, v_ll(b), f_hz, _rating(e, "max_p", "min_p"), vn[b],
                    converter_parameters, assumed, eid)], [b])
                continue
            if kind == "static_var_compensator":
                assumed["static var compensator held at its load-flow susceptance (no regulation)"].append(eid)
            comps = consumption(f"{prefix}{k}", bus_name[b], v_ll(b), s)
            if comps:
                add(kind, eid, e.get("name"), comps, [b])

    # Lines.
    for k, (lid, ln) in enumerate(network.get_lines(all_attributes=True).iterrows()):
        b1, b2 = ln["bus1_id"], ln["bus2_id"]
        if not (connected(b1) or connected(b2)):
            continue
        z = complex(ln["r"], ln["x"])
        if z.imag <= 0:
            problems.append(f"line {lid}: x = {ln['x']} ohm is not > 0")
            continue
        ys = [complex(ln["g1"], ln["b1"]), complex(ln["g2"], ln["b2"])]
        # An open end is a node of its own, at the voltage that the other
        # end gives it through the series impedance and its admittance.
        nodes, volts = [], []
        for b, other, y in ((b1, b2, ys[0]), (b2, b1, ys[1])):
            nodes.append(bus_name[b] if connected(b) else f"line{k}_open")
            volts.append(v_ll(b) if connected(b) else v_ll(other) / (1 + z * y))
        comps = []
        # Without an admittance at the open end, the series part carries
        # nothing and is left out.
        row = data("line", lid)
        z0 = complex(row["rz"], row["xz"]) if "rz" in row and row.get("xz", 0.0) > 0 else None
        # The zero-sequence shunt of the data is split between the ends as
        # the positive-sequence one.
        ys0: list[complex | None] = [None, None]
        if z0 is not None and ("gz" in row or "bz" in row) and sum(ys) != 0:
            total = complex(row.get("gz", sum(ys).real), row.get("bz", sum(ys).imag))
            ys0 = [total * (abs(y) / (abs(ys[0]) + abs(ys[1]))) for y in ys]
        if connected(b1, b2) or ys[0 if connected(b2) else 1] != 0:
            comps.append(series(f"line{k}", nodes[0], nodes[1], z, 3.0, z0))
            if z0 is None:
                assumed["no zero-sequence data; assumed R0 = 3 R1, X0 = 3 X1 (shunts as in the positive "
                        "sequence)"].append(lid)
            elif ys0[0] is None and sum(ys) != 0:
                assumed["no zero-sequence shunt admittance; as in the positive sequence"].append(lid)
            for i in (0, 1):
                comps += shunt(f"line{k}_y{i + 1}", nodes[i], volts[i], ys[i], ys0[i])
        else:
            i = 0 if connected(b1) else 1
            comps += shunt(f"line{k}_y{i + 1}", nodes[i], volts[i], ys[i], ys0[i])
        if comps:
            add("line", lid, ln.get("name"), comps, [b1, b2])

    # Two-winding transformers, at the current tap.
    for k, (tid, t) in enumerate(network.get_2_windings_transformers(all_attributes=True).iterrows()):
        b1, b2 = t["bus1_id"], t["bus2_id"]
        if not (connected(b1) or connected(b2)):
            continue
        # Side 2 lags side 1 by the clock; the ideal ratio is at side 1.
        hours = clocks.get(str(tid), [0, 0])
        kk = float(t["rho"]) * cmath.exp(1j * math.radians(float(t["alpha"]) - 30.0 * hours[1]))
        z = complex(t["r_at_current_tap"], t["x_at_current_tap"])
        y = complex(t["g_at_current_tap"], t["b_at_current_tap"])
        row = data("two_windings_transformer", tid)
        (c1, zg1, z0, free), (c2, zg2, _, _) = winding(row, 1, True), winding(row, 2, True)
        c1, c2 = clock_connections(tid, hours, [c1, c2], [float(t["rated_u1"]), float(t["rated_u2"])])
        zero = zero_of_leg(c1, [c2], z, z0, zg1, abs(kk) ** 2, free)
        if zero is not None and not free and row.get("xm", 0.0) > 0:
            zero["y0"] = 1 / (1j * row["xm"])
        if zero is not None and c1 == c2 == "Y_GROUNDED":
            zero["series"] += 3 * zg2
        zero_inv = (c1 == "Y_GROUNDED" and c2 == "Y_GROUNDED" and hours[1] == 6)
        if connected(b1):
            comps = leg(f"tr{k}", b1, bus_name[b2] if connected(b2) else None, kk, float(t["rated_u1"]), z, y,
                        zero, zero_inverted=zero_inv)
        else:
            comps = open_leg(f"tr{k}", bus_name[b2], v_ll(b2), z, y, zero)
        if c1 == "DELTA" and c2 == "Y_GROUNDED" and connected(b2):
            # Dyn: to gnd at side 2, behind the series impedance.
            comps += bank(f"tr{k}_z0", bus_name[b2], vn[b2], (z if z0 is None else z0) + 3 * zg2)
        if comps:
            if c1 is None or c2 is None:
                assumed["transformer connections unknown; the zero sequence passes as through "
                        "YNyn0"].append(tid)
            add("two_windings_transformer", tid, t.get("name"), comps, [b1, b2])

    # Three-winding transformers: legs to a star node at rated_u0.
    steps = {"ratio": network.get_ratio_tap_changer_steps(), "phase": network.get_phase_tap_changer_steps()}
    for k, (tid, t) in enumerate(network.get_3_windings_transformers(all_attributes=True).iterrows()):
        ids = [t[f"bus{i}_id"] for i in (1, 2, 3)]
        if not any(connected(b) for b in ids):
            continue
        star = f"tr3_{k}_star"
        legs = []
        # Leg 2 and leg 3 lag leg 1, and with it the star node, by their
        # clocks; the ideal ratio of a leg is at its bus.
        hours = clocks.get(str(tid), [0, 0, 0])
        for i, b in zip((1, 2, 3), ids):
            u0, ui = float(t["rated_u0"]), float(t[f"rated_u{i}"])
            kk = complex(u0 / ui)
            if hours[i - 1]:
                kk *= cmath.exp(1j * math.radians(30.0 * hours[i - 1]))
            z = complex(t[f"r{i}"], t[f"x{i}"])
            y = complex(t[f"g{i}"], t[f"b{i}"])
            for which in ("ratio", "phase"):
                pos = t[f"{which}_tap_position{i}"]
                if not finite(pos) or int(pos) == -99999:
                    continue
                st = steps[which]
                rows = st.loc[[ix for ix in st.index if ix[0] == tid and ix[1] == int(pos)]]
                rows = rows[rows["side"] == ["ONE", "TWO", "THREE"][i - 1]] if "side" in rows else rows
                if rows.empty:
                    problems.append(f"three-winding transformer {tid}: no {which} tap step {pos} on leg {i}")
                    continue
                r = rows.iloc[0]
                kk *= float(r["rho"]) * cmath.exp(1j * math.radians(float(r["alpha"]) if "alpha" in r else 0.0))
                z = complex(z.real * (1 + r["r"] / 100), z.imag * (1 + r["x"] / 100))
                y = complex(y.real * (1 + r["g"] / 100), y.imag * (1 + r["b"] / 100))
            legs.append((i, b, kk, ui, z, y))
        row = data("three_windings_transformer", tid)
        windings = [winding(row, i) for i in (1, 2, 3)]
        turned = clock_connections(tid, hours, [w[0] for w in windings], [float(t[f"rated_u{i}"]) for i in (1, 2, 3)])
        windings = [(c, *w[1:]) for c, w in zip(turned, windings)]
        if any(z == 0 for _, _, _, _, z, _ in legs):
            problems.append(f"three-winding transformer {tid}: a leg without series impedance")
            continue
        # The star voltage, which the open legs need: the connected legs
        # feed the star from k V through z, the open ones load it with
        # their z and y in series.
        feed = sum(kk * v_ll(b) / z for _, b, kk, _, z, _ in legs if connected(b))
        total = sum(1 / z if connected(b) else y / (1 + z * y) for _, b, _, _, z, y in legs)
        comps: list[dict[str, Any]] = []
        known_all = all(c is not None for c, _, _, _ in windings)
        grounded_hrs = {h for h, (c, _, _, _) in zip(hours, windings) if c == "Y_GROUNDED"}
        inv_leg6 = known_all and grounded_hrs == {0, 6}
        for (i, b, kk, ui, z, y), (conn, zg, z0, free) in zip(legs, windings):
            # A leg carries the zero sequence between its bus and the star
            # node when its winding is a grounded star; a delta winding
            # connects the star node to gnd through the impedance of its leg.
            zero = zero_of_leg(conn, [c for j, (c, _, _, _) in enumerate(windings) if j != i - 1], z, z0, zg,
                               abs(kk) ** 2, free)
            if connected(b):
                comps += leg(f"tr3_{k}_{i}", b, star, kk, ui, z, y, zero,
                             zero_inverted=inv_leg6 and conn == "Y_GROUNDED" and hours[i - 1] == 6)
            else:
                comps += open_leg(f"tr3_{k}_{i}", star, feed / total, z, y, zero)
            if zero is not None and conn == "DELTA":
                comps += bank(f"tr3_{k}_{i}_z0", star, float(t["rated_u0"]), z if z0 is None else z0)
        if any(conn is None for conn, _, _, _ in windings):
            assumed["transformer connections unknown; the zero sequence passes as through YNyn0"].append(tid)
        if comps:
            add("three_windings_transformer", tid, t.get("name"), comps, ids)

    # Dangling lines: shunt at the network side, series to a boundary node
    # that consumes p0 + j q0 at the load-flow boundary voltage.
    for k, (did, d) in enumerate(boundary_lines(network, all_attributes=True).iterrows()):
        b = d["bus_id"]
        if not connected(b) or not finite(d.get("boundary_v_mag")):
            continue
        x_node = f"dl{k}_x"
        vb = cmath.rect(float(d["boundary_v_mag"]), math.radians(float(d["boundary_v_angle"]) + shift[b]))
        row = data("dangling_line", did)
        comps = [series(f"dl{k}", bus_name[b], x_node, complex(d["r"], d["x"]), 3.0,
                        complex(row["rz"], row["xz"]) if "rz" in row and row.get("xz", 0.0) > 0 else None)]
        comps += shunt(f"dl{k}_y", bus_name[b], v_ll(b), complex(d["g"], d["b"]))
        comps += consumption(f"dl{k}_boundary", x_node, vb, complex(d["p0"], d["q0"]))
        add("dangling_line", did, d.get("name"), comps, [b])

    for k, b in enumerate(sorted(b for b in residual if b in bus_name and b not in balanced)):
        r = residual[b]
        if abs(r) <= 1e-9:
            continue
        # No generator at the bus: a current injection takes the residual.
        i = (r * 1e6 / (3 * v_ll(b) * 1e3 / math.sqrt(3))).conjugate()
        add("balance", b, None, [{
            "type": "three_phase_current_source", "name": f"balance{k}", "bus": bus_name[b],
            "current_rms_a": abs(i), "frequency_hz": f_hz, "angle_deg": math.degrees(cmath.phase(i)),
        }], [b])
        if abs(r) > 1e-6:
            assumed["the load flow left power unbalanced at a bus without a generator; a current injection "
                    "(balance<k>) takes it"].append(f"{b} ({r.real:+.2e} MW, {r.imag:+.2e} Mvar)")
    if problems:
        raise ConversionError(problems)

    model_buses = [bus_name[b] for b in live]
    if outputs in ("buses", "all"):
        outs: list[dict[str, Any]] = [{"type": "bus_voltage", "bus": b} for b in model_buses]
        if outputs == "all":
            outs += [{"type": "branch_current", "component": c["name"]} for c in components
                     if c["type"] in ("three_phase_source", "three_phase_current_source",
                                      "grid_following_converter", "coupled_rl",
                                      "three_phase_ideal_transformer")]
    else:
        outs = list(outputs)

    model = model_document(
        name or str(getattr(network, "name", "") or network.id or "PowSyBl network"),
        "Converted from PowSyBl (IIDM) by openkeito.powsybl.from_powsybl; starts from its AC load flow.",
        components,
        outs,
        time_step_s=time_step_s,
        end_time_s=end_time_s,
        method=method,
        output_interval_steps=output_interval_steps,
    )
    mapping = {
        "source": "powsybl",
        "frequency_hz": f_hz,
        "buses": {b: bus_name[b] for b in live},
        "elements": elements,
        "short_circuit": used,
    }
    if rotate:
        mapping["bus_phase_shift_deg"] = {b: shift[b] for b in live}
    warnings = summarize(assumed)
    if dead:
        # Every bus that is not in the model is named.
        warnings.append(f"{len(dead)} bus(es) without a load-flow voltage (not connected to a generator, or in "
                        "a part of the network that the load flow did not solve) left out with what is "
                        f"connected to them: {', '.join(dead)}")
    return Conversion(model=model, mapping=mapping, warnings=warnings, power_flow={b: pf[b] for b in live},
                      assumed_machine_ratings=assumed_ratings)


def _clocks(network: Any, rows: dict[str, dict[str, Any]]) -> tuple[dict[str, list[int]], list[str]]:
    """(the phase-angle clocks of the windings of every transformer that has
    data: [0, side 2] or [0, leg 2, leg 3], in hours; what is wrong with
    them). A clock is a whole number from 0 to 11 (IEC 60076-1,
    PowerTransformerEnd.phaseAngleClock of CGMES): the winding lags winding
    1 by that many times 30 degrees."""
    out: dict[str, list[int]] = {}
    problems = []
    for getter, keys in (("get_2_windings_transformers", ("phase_angle_clock",)),
                         ("get_3_windings_transformers", ("phase_angle_clock_leg2", "phase_angle_clock_leg3"))):
        for tid in map(str, getattr(network, getter)().index):
            row = rows.get(tid, {})
            if not any(key in row for key in keys):
                continue
            hours = [0]
            for key in keys:
                value = row.get(key, 0.0)
                if isinstance(value, bool) or not finite(value) or float(value) != int(float(value)) or \
                        not 0 <= float(value) <= 11:
                    problems.append(f"transformer {tid}: {key} = {value!r} is not a whole number from 0 to 11")
                    value = 0.0
                hours.append(int(float(value)))
            out[tid] = hours
    return out, problems


def _clock_shifts(network: Any, live: list[str], vn: dict[str, float], clocks: dict[str, list[int]],
                  ) -> tuple[dict[str, float], list[str]]:
    """(the angle in degrees that the phase-angle clocks add to the
    load-flow voltage of every bus; the loops whose clocks do not add up).

    The buses and the star nodes of the three-winding transformers are the
    nodes of a graph whose edges are the branches that are connected at
    both ends: a line adds nothing, a two-winding transformer makes side 2
    lag side 1 by its clock, a leg of a three-winding transformer makes its
    bus lag the star node by its clock. In every connected part, the bus
    with the highest nominal voltage (the first of them) keeps its angle,
    and a search from it gives every other node its lag. An edge between
    two nodes whose lags it contradicts closes a loop that no set of
    voltages satisfies: the transformers of the loop are named."""
    inside = set(live)
    edges: dict[Any, list[tuple[Any, int, str]]] = defaultdict(list)

    def edge(a: Any, b: Any, hours: int, label: str) -> None:
        """b lags a by hours * 30 degrees."""
        edges[a].append((b, hours, label))
        edges[b].append((a, -hours, label))

    for lid, r in network.get_lines().iterrows():
        if r["bus1_id"] in inside and r["bus2_id"] in inside:
            edge(r["bus1_id"], r["bus2_id"], 0, f"line {lid}")
    for tid, r in network.get_2_windings_transformers().iterrows():
        if r["bus1_id"] in inside and r["bus2_id"] in inside:
            edge(r["bus1_id"], r["bus2_id"], clocks.get(str(tid), [0, 0])[1], f"transformer {tid}")
    for tid, r in network.get_3_windings_transformers().iterrows():
        for i, hours in zip((1, 2, 3), clocks.get(str(tid), [0, 0, 0])):
            if r[f"bus{i}_id"] in inside:
                edge(("star", tid), r[f"bus{i}_id"], hours, f"transformer {tid} (leg {i})")

    lag: dict[Any, int] = {}
    parent: dict[Any, tuple[Any, int, str]] = {}
    problems: list[str] = []
    loops = set()

    def turning(node: Any) -> set[str]:
        """The edges with a clock on the way from a node to its root."""
        out = set()
        while node in parent:
            node, hours, label = parent[node]
            if hours % 12:
                out.add(label)
        return out

    order = {b: k for k, b in enumerate(live)}
    for root in sorted(live, key=lambda b: (-vn[b], order[b])):
        if root in lag:
            continue
        lag[root] = 0
        queue = [root]
        for a in queue:
            for b, hours, label in edges[a]:
                if b not in lag:
                    lag[b] = (lag[a] + hours) % 12
                    parent[b] = (a, hours, label)
                    queue.append(b)
                elif (lag[a] + hours - lag[b]) % 12:
                    loop = tuple(sorted(turning(a) ^ turning(b) | ({label} if hours % 12 else set())))
                    if loop in loops:
                        continue
                    loops.add(loop)
                    where = b if isinstance(b, str) else f"the star node of transformer {b[1]}"
                    problems.append(
                        f"the phase-angle clocks do not add up around the loop through {', '.join(loop)}: "
                        f"{where} would lag {root} by both {30 * lag[b]} and {30 * ((lag[a] + hours) % 12)} "
                        "degrees; correct the clocks (short_circuit_data) or convert with "
                        "phase_angle_clocks='ignore'")
    # A lag of more than half a turn is a lead.
    return {b: -30.0 * lag[b] + (360.0 if lag[b] > 6 else 0.0) for b in live}, problems


def _rating(row: Any, *columns: str) -> float | None:
    """The largest magnitude among the columns of an element that hold a
    usable rating in MVA or MW; None when there is none. An unknown limit
    is written as a huge number, or as 9999 in the IEEE cases: from 9999 on
    a value counts as unknown."""
    values = [abs(float(row[c])) for c in columns if c in row.index and finite(row[c])]
    values = [v for v in values if 0 < v < 9999]
    return max(values) if values else None


def _status(result: Any) -> str:
    """Status of a component of a load-flow result, with the solver's text."""
    text = getattr(result, "status_text", "")
    return str(result.status).rpartition(".")[2] + (f": {text}" if text else "")


def tight_load_flow_parameters() -> Any:
    """Load-flow parameters for the converter: generators keep their set
    points (no distributed slack); the slack is the bus of the largest
    generator, which takes the losses and any imbalance; OpenLoadFlow's
    Newton tolerance is 1e-10 per equation (default 1e-4) and its slack-bus
    mismatch 1e-6 MW (default 1 MW).

    Without the distributed slack OpenLoadFlow fails on some networks that
    its default parameters solve; ``from_powsybl`` takes others as
    ``load_flow_parameters``."""
    import pypowsybl.loadflow as lf

    return lf.Parameters(distributed_slack=False,
                         provider_parameters={"newtonRaphsonConvEpsPerEq": "1e-10",
                                              "slackBusPMaxMismatch": "1e-6",
                                              "slackBusSelectionMode": "LARGEST_GENERATOR"})


def imbalance(network: Any) -> dict[str, complex]:
    """Per bus, the sum of the load flow's terminal flows (MW, Mvar) out of
    the bus into every element: zero for an exact solution."""
    out: dict[str, complex] = defaultdict(complex)

    def take(bus: Any, p: Any, q: Any) -> None:
        if isinstance(bus, str) and bus and finite(p):
            out[bus] += complex(p, q if finite(q) else 0.0)

    frames = [getattr(network, getter)() for getter in ("get_generators", "get_loads", "get_shunt_compensators",
                                                        "get_batteries", "get_static_var_compensators")]
    for frame in frames + [boundary_lines(network)]:
        for _, r in frame.iterrows():
            take(r["bus_id"], r["p"], r["q"])
    for getter in ("get_lines", "get_2_windings_transformers"):
        for _, r in getattr(network, getter)().iterrows():
            take(r["bus1_id"], r["p1"], r["q1"])
            take(r["bus2_id"], r["p2"], r["q2"])
    for _, r in network.get_3_windings_transformers().iterrows():
        for i in (1, 2, 3):
            take(r[f"bus{i}_id"], r[f"p{i}"], r[f"q{i}"])
    return dict(out)


# Schedules and time slices ---------------------------------------------------

#: Scheduled quantities of each IIDM element kind and the attributes they
#: set (with IIDM's signs: target_p of generators and batteries is
#: generated, p0 of loads consumed; voltage_pu times the nominal voltage of
#: the regulated bus is target_v).
KIND_QUANTITIES: dict[str, dict[str, Any]] = {
    "generators": {"active_power_mw": "target_p", "voltage_pu": "target_v", "in_service": "connected"},
    "loads": {"active_power_mw": "p0", "reactive_power_mvar": "q0", "in_service": "connected"},
    "batteries": {"active_power_mw": "target_p", "reactive_power_mvar": "target_q", "in_service": "connected"},
    "shunt_compensators": {"in_service": "connected"},
    "lines": {"in_service": ("connected1", "connected2")},
    "2_windings_transformers": {"tap_position": "tap", "in_service": ("connected1", "connected2")},
}


def _kinds(network: Any) -> dict[str, str]:
    """Element id -> kind, for the kinds a schedule can set."""
    out = {}
    for kind in KIND_QUANTITIES:
        for eid in getattr(network, f"get_{kind}")().index:
            out[eid] = kind
    return out


def check_schedule(network: Any, schedule: Any) -> None:
    """Raises ConversionError unless every series names an element of the
    network (by its id, the CGMES mRID) and a quantity of its kind."""
    kinds = _kinds(network)
    problems = []
    changers = None
    for k, s in enumerate(schedule.series):
        kind = kinds.get(s["target"])
        if kind is None:
            problems.append(f"series[{k}]: the network has no element {s['target']!r} that a schedule can set "
                            f"({', '.join(KIND_QUANTITIES)})")
        elif s["quantity"] not in KIND_QUANTITIES[kind]:
            problems.append(f"series[{k}]: {s['quantity']} cannot be scheduled for {kind} "
                            f"(quantities: {', '.join(KIND_QUANTITIES[kind])})")
        elif s["quantity"] == "tap_position":
            if changers is None:
                changers = network.get_ratio_tap_changers()
            if s["target"] not in changers.index:
                problems.append(f"series[{k}]: tap_position cannot be scheduled for {s['target']!r}, which has "
                                "no ratio tap changer")
                continue
            low, high = int(changers.at[s["target"], "low_tap"]), int(changers.at[s["target"], "high_tap"])
            outside = sorted({int(round(v)) for v in s["values"] if not low <= round(v) <= high})
            if outside:
                problems.append(f"series[{k}]: tap_position {', '.join(map(str, outside))} of {s['target']!r} "
                                f"is outside the positions {low} to {high} of its ratio tap changer")
    if problems:
        raise ConversionError(problems)


def apply_schedule(network: Any, schedule: Any, time: Any) -> Any:
    """A copy of ``network`` with the values of the schedule at ``time``."""
    import copy

    check_schedule(network, schedule)
    kinds = _kinds(network)
    out = copy.deepcopy(network)
    gens = out.get_generators(all_attributes=True)
    buses = out.get_buses()
    nominal = out.get_voltage_levels()["nominal_v"]
    for (target, quantity), value in schedule.values_at(time).items():
        kind = kinds[target]
        attribute = KIND_QUANTITIES[kind][quantity]
        if quantity == "in_service":
            value = bool(value)
        elif quantity == "tap_position":
            out.update_ratio_tap_changers(id=target, tap=int(round(value)))
            continue
        elif quantity == "voltage_pu":
            regulated = gens.at[target, "regulated_bus_id"] if "regulated_bus_id" in gens else ""
            level = (buses.at[regulated, "voltage_level_id"] if isinstance(regulated, str) and regulated in buses.index
                     else gens.at[target, "voltage_level_id"])
            value = float(value) * float(nominal[level])
        attributes = attribute if isinstance(attribute, tuple) else (attribute,)
        getattr(out, f"update_{kind}")(id=target, **{a: value for a in attributes})
    return out


def time_slices(
    network: Any,
    schedule: Any,
    disturbances: list[dict[str, Any]] | None = None,
    *,
    times: list[Any] | None = None,
    **convert_options: Any,
) -> list[TimeSlice]:
    """For every time (default: every time of the schedule) and every
    disturbance (default: none), the network with the schedule's values,
    its AC load flow, and the converted model with the disturbance added. A
    component's "bus" that is an IIDM bus id becomes its model bus;
    disturbances need names of their own. convert_options go to
    ``from_powsybl``. A time whose load flow or conversion fails raises
    ConversionError with the time."""
    return schedule_slices(network, schedule, disturbances, times, check=check_schedule, apply=apply_schedule,
                           convert=from_powsybl, convert_options=convert_options)


def run_time_slices(
    network: Any,
    schedule: Any,
    disturbances: list[dict[str, Any]] | None = None,
    *,
    times: list[Any] | None = None,
    jobs: int | None = None,
    out: Any = None,
    force: bool = False,
    keep_rows: bool = False,
    **convert_options: Any,
) -> TimeSliceRun:
    """Makes the time slices (see ``time_slices``) and runs them in parallel;
    see ``openkeito.convert.run_slices`` for the rows of the result."""
    slices = time_slices(network, schedule, disturbances, times=times, **convert_options)
    return run_slices(slices, jobs=jobs, out=out, force=force, keep_rows=keep_rows)


def boundary_lines(network: Any, **options: Any) -> Any:
    """The dangling (boundary) lines; pypowsybl renamed get_dangling_lines to
    get_boundary_lines."""
    getter = getattr(network, "get_boundary_lines", None) or network.get_dangling_lines
    return getter(**options)


# Short-circuit data ------------------------------------------------------------

#: Factor on the positive-sequence impedance that stands for no zero-sequence
#: path through a series branch (a winding that is a delta or a star without
#: a grounded star point).
ZERO_SEQUENCE_BLOCK = 1e6

#: Winding connections (IIDM's WindingConnectionType).
CONNECTIONS = ("Y", "Y_GROUNDED", "DELTA")

#: Fields of ``short_circuit_data`` by element kind, with the names and units
#: of the IIDM extensions generatorShortCircuit, generatorFortescue,
#: lineFortescue, twoWindingsTransformerFortescue (impedances at side 2, as
#: r and x), threeWindingsTransformerFortescue (per leg 1 to 3, at rated_u0)
#: and the phase-angle clocks: reactances and resistances in ohm, rz + j xz
#: the zero-sequence and rn + j xn the negative-sequence impedance, grounding
#: impedances in ohm at the winding, connection types "Y", "Y_GROUNDED" or
#: "DELTA". Beyond the extensions: ``stator_r`` and ``direct_sync_x`` of a
#: generator (ohm), ``gz`` and ``bz`` of a line (its whole zero-sequence
#: shunt conductance and susceptance in siemens; used with ``rz``, ``xz``).
#:
#: How the extensions are read, and where that is written (PowSyBl,
#: powsybl-core, docs/grid_model/extensions.md and network_subnetwork.md;
#: https://powsybl.readthedocs.io/projects/powsybl-core/en/stable/grid_model/extensions.html):
#:
#: - ``rz``, ``xz`` of a two-winding transformer are at side 2. The table of
#:   the extension gives only the unit; IIDM has the impedances of a
#:   two-winding transformer at its side 2 (network_subnetwork.md,
#:   "Two-winding transformer"), and PowSyBl's own readers of such data do
#:   the same: the CGMES post-processor of powsybl-incubator
#:   (simulator/short-circuit-cgmes) adds the r0, x0 of end 1 times
#:   (rated_u2 / rated_u1)^2 to those of end 2, and its short-circuit
#:   engine divides them by the impedance base of voltage level 2. Those
#:   of a leg of a three-winding transformer are at rated_u0 there.
#: - ``free_fluxes``: "If set to true, then the magnetizing impedance is
#:   considered as infinite" (default true): no zero-sequence magnetizing
#:   admittance. With false it is that of ``xm`` (the magnetizing reactance
#:   in ohm, taken at side 2 as the other impedances; two-winding
#:   transformers only), or, without ``xm``, the magnetizing admittance of
#:   the positive sequence.
#: - ``grounding_r``, ``grounding_x`` of a winding: the table says "the
#:   resistance (reactance) part of the impedance to the ground" in ohm and
#:   not at which voltage. They are read as the impedance that is connected
#:   to the star point, in ohm at the voltage of that winding, as
#:   TransformerEnd.rground of CGMES.
#: - ``grounded`` of a generator is false by default in the extension. Here
#:   a generator with zero-sequence data and no ``grounded`` is grounded
#:   (the extension always holds the flag; the default matters only for
#:   ``short_circuit_data`` of the caller).
SHORT_CIRCUIT_FIELDS: dict[str, tuple[str, ...]] = {
    "generators": ("direct_sub_trans_x", "direct_trans_x", "direct_sync_x", "step_up_transformer_x", "stator_r",
                   "rz", "xz", "rn", "xn", "grounded", "grounding_r", "grounding_x"),
    "lines": ("rz", "xz", "gz", "bz"),
    "dangling_lines": ("rz", "xz"),
    "2_windings_transformers": ("rz", "xz", "free_fluxes", "xm", "connection_type1", "connection_type2",
                                "grounding_r1", "grounding_x1", "grounding_r2", "grounding_x2",
                                "phase_angle_clock"),
    "3_windings_transformers": tuple(f"{field}{i}" for i in (1, 2, 3) for field in (
        "rz", "xz", "free_fluxes", "connection_type", "grounding_r", "grounding_x")) + (
        "phase_angle_clock_leg2", "phase_angle_clock_leg3"),
}

_TABLE_EXTENSIONS = ("generatorShortCircuit", "twoWindingsTransformerPhaseAngleClock",
                     "threeWindingsTransformerPhaseAngleClock")


def _clean(row: dict[str, Any]) -> dict[str, Any]:
    """The fields of a row that hold a value: numbers as floats, flags as
    bools, connection types as they are."""
    out: dict[str, Any] = {}
    for key, value in row.items():
        if isinstance(value, str) and value.lower() in ("true", "false"):
            value = value.lower() == "true"
        if isinstance(value, bool) or key.startswith("connection_type"):
            if value is not None:
                out[key] = value
        elif finite(value):
            out[key] = float(value)
    return out


def _fortescue_extensions(network: Any) -> dict[str, dict[str, Any]]:
    """The Fortescue extensions of a network (generatorFortescue,
    lineFortescue, twoWindingsTransformerFortescue,
    threeWindingsTransformerFortescue), which pypowsybl keeps but does not
    give as tables: read from the network written as XIIDM."""
    import re
    import xml.etree.ElementTree as ET

    save = getattr(network, "save_to_string", None)
    if save is None:
        return {}
    try:
        text = save("XIIDM")
    except Exception:  # a network that cannot be written has no such data to read
        return {}
    if "Fortescue" not in text:
        return {}

    def snake(name: str) -> str:
        return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()

    rows: dict[str, dict[str, Any]] = {}
    for extension in ET.fromstring(text).iter():
        if extension.tag.rpartition("}")[2] != "extension":
            continue
        for child in extension:
            if not child.tag.endswith("Fortescue"):
                continue
            row = {snake(k): v for k, v in child.attrib.items()}
            for part in child:
                # The legs of a three-winding transformer.
                number = part.tag.rpartition("}")[2].removeprefix("leg")
                row.update({f"{snake(k)}{number}": v for k, v in part.attrib.items()})
            rows[extension.get("id")] = _clean(row)
    return rows


def short_circuit_data(network: Any) -> dict[str, dict[str, Any]]:
    """What the extensions of a network hold of the data that a load flow
    does not need, by element id, with the fields of
    ``SHORT_CIRCUIT_FIELDS``: generatorShortCircuit and the phase-angle
    clocks from pypowsybl's tables, the Fortescue extensions from the network
    written as XIIDM. ``from_powsybl`` uses them."""
    return _extension_rows(network)[0]


def _extension_rows(network: Any) -> tuple[dict[str, dict[str, Any]], dict[str, list[str]]]:
    """(rows by element id, the extensions each row came from)."""
    rows: dict[str, dict[str, Any]] = defaultdict(dict)
    sources: dict[str, list[str]] = defaultdict(list)
    for name in _TABLE_EXTENSIONS:
        try:
            frame = network.get_extensions(name)
        except Exception:  # a pypowsybl without this extension
            continue
        for eid, r in frame.iterrows():
            row = _clean(r.to_dict())
            if row:
                rows[str(eid)].update(row)
                sources[str(eid)].append(name)
    for eid, row in _fortescue_extensions(network).items():
        if row:
            rows[eid].update(row)
            sources[eid].append("fortescue")
    return dict(rows), dict(sources)


def _short_circuit_rows(network: Any, given: dict[str, dict[str, Any]] | None,
                        ) -> tuple[dict[str, dict[str, Any]], dict[str, list[str]]]:
    """The rows of the extensions with those given by the caller on top.
    Raises ValueError for an id that is no generator, line, dangling line or
    transformer, for a field that its kind does not have and for a
    connection type that does not exist."""
    rows, sources = _extension_rows(network)
    if not given:
        return rows, sources
    kinds: dict[str, str] = {}
    for kind in SHORT_CIRCUIT_FIELDS:
        frame = boundary_lines(network) if kind == "dangling_lines" else getattr(network, f"get_{kind}")()
        kinds.update({str(eid): kind for eid in frame.index})
    problems = []
    for eid, row in given.items():
        kind = kinds.get(str(eid))
        if kind is None:
            problems.append(f"{eid!r} is not the id of a generator, line, dangling line or transformer")
            continue
        unknown = sorted(set(row) - set(SHORT_CIRCUIT_FIELDS[kind]))
        if unknown:
            problems.append(f"{eid!r}: {', '.join(unknown)} is no field of {kind} "
                            f"(fields: {', '.join(SHORT_CIRCUIT_FIELDS[kind])})")
        wrong = sorted({str(v) for k, v in row.items() if k.startswith("connection_type") and v not in CONNECTIONS})
        if wrong:
            problems.append(f"{eid!r}: connection type {', '.join(wrong)}; use one of {', '.join(CONNECTIONS)}")
        row = _clean(row)
        if row:
            rows.setdefault(str(eid), {}).update(row)
            sources.setdefault(str(eid), []).append("short_circuit_data")
    if problems:
        raise ValueError("short_circuit_data: " + "; ".join(problems))
    return rows, sources


def _cgmes_id(raw: str) -> str:
    return raw.removeprefix("#").removeprefix("urn:uuid:").removeprefix("_")


class _SafeXmlStream:
    """Streaming byte wrapper that rejects DTD/ENTITY declarations before
    ET.iterparse parses a chunk."""

    def __init__(self, raw: Any) -> None:
        self._raw = raw
        self._tail = b""

    def read(self, size: int = -1) -> bytes:
        chunk = self._raw.read(size)
        if chunk:
            window = (self._tail + chunk).upper()
            if b"<!DOCTYPE" in window or b"<!ENTITY" in window:
                raise ValueError("CGMES XML files containing <!DOCTYPE or <!ENTITY declarations are not allowed")
            self._tail = chunk[-16:]
        return chunk


def _cgmes_objects(path: Any, classes: set[str]) -> dict[str, dict[str, dict[str, str]]]:
    """The objects of the given classes in the CGMES files at ``path`` (an
    XML file, a zip of them or a directory): class -> id -> attribute ->
    text (or the resource it refers to). The id is the rdf:ID without its
    leading underscore, which is pypowsybl's id of the element. The entry
    "@namespace" of an object is the namespace of its class, which tells
    the CIM version (CIM16 for CGMES 2.4.15, CIM100 for CGMES 3.0)."""
    import os
    import xml.etree.ElementTree as ET
    import zipfile

    rdf = "{http://www.w3.org/1999/02/22-rdf-syntax-ns#}"
    objects: dict[str, dict[str, dict[str, str]]] = {c: {} for c in classes}

    def read(stream: Any) -> None:
        depth = 0
        root: Any = None
        try:
            for event, element in ET.iterparse(_SafeXmlStream(stream), events=("start", "end")):
                if event == "start":
                    depth += 1
                    if depth == 1:
                        if element.tag != f"{rdf}RDF":
                            return
                        root = element
                    continue
                depth -= 1
                if depth != 1:
                    continue
                kind = element.tag.rpartition("}")[2]
                if kind in classes:
                    oid = _cgmes_id(element.get(f"{rdf}ID") or element.get(f"{rdf}about") or "")
                    attributes = objects[kind].setdefault(oid, {})
                    attributes["@namespace"] = element.tag.rpartition("}")[0].lstrip("{")
                    for child in element:
                        resource = child.get(f"{rdf}resource")
                        attributes[child.tag.rpartition("}")[2].partition(".")[2]] = (
                            resource if resource is not None else (child.text or "").strip())
                element.clear()
                if root is not None:
                    root.clear()
        except ET.ParseError:
            return

    path = str(path)
    if os.path.isdir(path):
        for name in sorted(os.listdir(path)):
            if name.lower().endswith(".xml"):
                with open(os.path.join(path, name), "rb") as stream:
                    read(stream)
    elif zipfile.is_zipfile(path):
        with zipfile.ZipFile(path) as archive:
            for name in sorted(archive.namelist()):
                if name.lower().endswith(".xml"):
                    with archive.open(name) as stream:
                        read(stream)
    elif path.lower().endswith(".xml"):
        with open(path, "rb") as stream:
            read(stream)
    return objects


def cgmes_short_circuit_data(path: Any, network: Any) -> tuple[dict[str, dict[str, Any]], list[str]]:
    """(short-circuit data by element id, what was left out and why) from the
    CGMES files at ``path`` (an XML file, a zip or a directory; anything else
    gives nothing), for ``network``, which pypowsybl read from them.

    pypowsybl's CGMES import keeps none of the short-circuit attributes of
    the equipment profile except the phase-angle clock (with the
    post-processor PhaseAngleClock). This reads, with the standard library:

    - SynchronousMachine: satDirectSubtransX, satDirectTransX,
      satDirectSyncX (per unit of ratedU^2 / ratedS; the nominal voltage
      and rated_s of the network when the file has none), r0, x0, r2, x2
      (per unit in CGMES 2.4.15, ohm in CGMES 3.0), r, earthing,
      earthingStarPointR, earthingStarPointX (ohm);
    - ACLineSegment: r0, x0, g0ch, b0ch;
    - PowerTransformerEnd: connectionKind, grounded, rground, xground,
      phaseAngleClock, r0, x0 (ohm at the end; referred to side 2 of a
      two-winding transformer and to rated_u0 of a three-winding one, as
      IIDM's r and x), b0 and g0 (a transformer without them has free
      fluxes: no zero-sequence magnetizing admittance).

    The units are the data types of the attributes in the RDF schemas of
    the profiles that ENTSO-E publishes (CGMES 2.4.15:
    EquipmentProfileCoreShortCircuitOperationRDFSAugmented-v2_4_15-4Jul2016.rdf,
    namespace CIM16; CGMES 3.0, IEC 61970-600-2:
    61970-600-2_ShortCircuit-AP-Voc-RDFS2020_v3-0-0.rdf, namespace CIM100).
    SynchronousMachine.r is a Resistance in both (the equivalent
    resistance R_G of IEC 60909) and the saturated reactances are PU in
    both, but r0, x0, r2 and x2 are PU in 2.4.15 and Resistance and
    Reactance in 3.0. PowerTransformerEnd.r0 and x0 are the Resistance
    and Reactance of the end ("star-model"; of two ends the high-voltage
    one holds the values), TransformerEnd.rground and xground those of the
    neutral impedance, and phaseAngleClock is the lag of the end against
    end 1 in hours of 30 degrees.

    The result has the fields of ``SHORT_CIRCUIT_FIELDS`` and can be given to
    ``from_powsybl`` as ``short_circuit_data``; ``from_cgmes`` does that."""
    objects = _cgmes_objects(path, {"SynchronousMachine", "ACLineSegment", "PowerTransformerEnd"})
    rows: dict[str, dict[str, Any]] = {}
    skipped: list[str] = []

    def number(attributes: dict[str, str], name: str) -> float | None:
        try:
            value = float(attributes[name])
        except (KeyError, ValueError):
            return None
        return value if math.isfinite(value) else None

    def put(eid: str, row: dict[str, Any]) -> None:
        row = {k: v for k, v in row.items() if v is not None}
        if row:
            rows[eid] = row

    gens = network.get_generators(all_attributes=True)
    nominal = network.get_voltage_levels()["nominal_v"]
    for eid, a in objects["SynchronousMachine"].items():
        if eid not in gens.index:
            continue
        u = number(a, "ratedU") or float(nominal[gens.at[eid, "voltage_level_id"]])
        s = number(a, "ratedS") or (float(gens.at[eid, "rated_s"]) if finite(gens.at[eid, "rated_s"]) else None)
        per_unit = {"direct_sub_trans_x": "satDirectSubtransX", "direct_trans_x": "satDirectTransX",
                    "direct_sync_x": "satDirectSyncX"}
        sequence = {"rz": "r0", "xz": "x0", "rn": "r2", "xn": "x2"}
        # CGMES 3.0 (CIM100) gives the zero- and negative-sequence
        # impedances in ohm, CGMES 2.4.15 (CIM16) per unit.
        in_ohm = "CIM100" in a.get("@namespace", "")
        if not in_ohm:
            per_unit.update(sequence)
        if s is None or not s > 0:
            if any(name in a for name in per_unit.values()):
                skipped.append(f"generator {eid}: per-unit values without a rated power")
            per_unit = {}
        row: dict[str, Any] = {field: number(a, name) * u * u / s for field, name in per_unit.items()
                               if number(a, name) is not None}
        if in_ohm:
            row.update({field: number(a, name) for field, name in sequence.items()})
        # SynchronousMachine.r is a Resistance (ohm) in both versions.
        row["stator_r"] = number(a, "r")
        if "earthing" in a:
            row["grounded"] = a["earthing"].lower() == "true"
            row["grounding_r"], row["grounding_x"] = number(a, "earthingStarPointR"), number(a, "earthingStarPointX")
        put(eid, row)

    line_ids = set(map(str, network.get_lines().index))
    dangling_ids = set(map(str, boundary_lines(network).index))
    for eid, a in objects["ACLineSegment"].items():
        if (number(a, "x0") or 0.0) <= 0:
            continue
        if eid in line_ids:
            put(eid, {"rz": number(a, "r0") or 0.0, "xz": number(a, "x0"), "gz": number(a, "g0ch"),
                      "bz": number(a, "b0ch")})
        elif eid in dangling_ids:
            put(eid, {"rz": number(a, "r0") or 0.0, "xz": number(a, "x0")})
            ignored = [attr for attr in ("g0ch", "b0ch") if number(a, attr) is not None]
            if ignored:
                skipped.append(f"dangling line {eid}: zero-sequence shunt admittance ({', '.join(ignored)}) "
                               "is not represented")

    ends: dict[str, list[dict[str, str]]] = defaultdict(list)
    for a in objects["PowerTransformerEnd"].values():
        ends[_cgmes_id(a.get("PowerTransformer", ""))].append(a)
    two = network.get_2_windings_transformers(all_attributes=True)
    three = network.get_3_windings_transformers(all_attributes=True)

    def connection(tid: str, a: dict[str, str]) -> str | None:
        kind = a.get("connectionKind", "").rpartition(".")[2]
        if kind == "D":
            return "DELTA"
        if kind in ("Y", "Z"):
            return "Y"
        if kind == "Yn":
            return "Y" if a.get("grounded", "true").lower() == "false" else "Y_GROUNDED"
        if kind:
            skipped.append(f"transformer {tid}: winding connection {kind} is not represented")
        return None

    for tid, parts in ends.items():
        frame = two if tid in two.index else three if tid in three.index else None
        parts = sorted(parts, key=lambda a: number(a, "endNumber") or 0.0)
        if frame is None or len(parts) != (2 if frame is two else 3):
            continue
        rated = [float(frame.at[tid, f"rated_u{i + 1}"]) for i in range(len(parts))]
        if any(number(a, "ratedU") is None or abs(number(a, "ratedU") - u) > 1e-6 * u
               for a, u in zip(parts, rated)):
            skipped.append(f"transformer {tid}: the rated voltages of its ends are not those of the network's "
                           "windings")
            continue
        connections = [connection(tid, a) for a in parts]
        free = not any((number(a, name) or 0.0) != 0 for a in parts for name in ("b0", "g0"))
        row = {}
        # IIDM has the impedances of a two-winding transformer at side 2 and
        # those of a three-winding one at rated_u0.
        u0 = rated[1] if frame is two else float(frame.at[tid, "rated_u0"])
        z0 = [complex(number(a, "r0") or 0.0, number(a, "x0") or 0.0) * (u0 / u) ** 2 for a, u in zip(parts, rated)]
        if frame is two:
            if sum(z0).imag > 0:
                row.update(rz=sum(z0).real, xz=sum(z0).imag)
            if None not in connections:
                row["free_fluxes"] = free
            row["phase_angle_clock"] = number(parts[1], "phaseAngleClock")
        else:
            for i, z in enumerate(z0, start=1):
                if z.imag > 0:
                    row.update({f"rz{i}": z.real, f"xz{i}": z.imag})
                if None not in connections:
                    row[f"free_fluxes{i}"] = free
            row["phase_angle_clock_leg2"] = number(parts[1], "phaseAngleClock")
            row["phase_angle_clock_leg3"] = number(parts[2], "phaseAngleClock")
        if None not in connections:
            for i, (a, c) in enumerate(zip(parts, connections), start=1):
                row[f"connection_type{i}"] = c
                if c == "Y_GROUNDED":
                    row[f"grounding_r{i}"], row[f"grounding_x{i}"] = number(a, "rground"), number(a, "xground")
        put(tid, row)
    return rows, skipped
# Transitions -----------------------------------------------------------------


def transition(network: Any, schedule: Any, from_time: Any = None, to_time: Any = None, *, times: Any = None,
               name: str | None = None, **options: Any) -> Any:
    """The model that starts from the load flow of ``from_time`` and moves
    the set points to the schedule's values of ``to_time`` (default: the
    first two times of the schedule): an ``openkeito.transition.Transition``.
    times: instead, two or more times of the schedule (or "all") that one
    model goes through (see ``openkeito.pandapower.transition``).
    options: those of ``openkeito.transition.build`` and of
    ``from_powsybl``; unless given, generators="machine",
    inverter_model="converter" and output_interval_steps=20. Where an option
    takes elements, they are named as in the mapping ("generator/<id>")."""
    from .transition import make_transition

    return make_transition(network, schedule, from_time, to_time, times=times, check=check_schedule,
                           apply=apply_schedule, convert=from_powsybl, name=name, rename_buses=True,
                           options=options)


def run_transitions(network: Any, schedule: Any, intervals: Any = None, *, times: Any = None, chain: bool = False,
                    jobs: int | None = None, out: Any = None, force: bool = False, window_s: float | None = None,
                    **options: Any) -> Any:
    """Makes the transition of every pair of neighbouring times of the
    schedule (or of the ``intervals`` given) and runs them in parallel, or,
    with chain=True or ``times``, one transition through all of them; see
    ``openkeito.pandapower.run_transitions`` for the result."""
    from .transition import make_and_run

    return make_and_run(network, schedule, intervals, make=transition, jobs=jobs, out=out, force=force,
                        window_s=window_s, options=options, times=times, chain=chain,
                        check=check_schedule, apply=apply_schedule, convert=from_powsybl, rename_buses=True)


__all__ += ["transition", "run_transitions"]
