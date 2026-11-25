"""Conversion of a pandapower network and its power flow into an OpenKeito model.

The model starts from the steady state of the power flow: the converter
chooses the values of the components so that the phasor solution of the EMT
model is the power-flow solution, and the model asks for
``"initialization": "steady_state"`` (docs/numerical-method.md).

- External grids and generators: a three-phase source behind an impedance
  Z, with the internal voltage E = V + Z I, I = conj(S / 3V) (V, S from the
  power flow).
- Loads and shunts: constant impedances that take the power-flow P and Q at
  the power-flow voltage.
- Static generators and storage: balanced current injections I = conj(S / 3V).
- Lines: nominal pi lines (lines without capacitance: coupled RL); the shunt
  conductance g_us_per_km is a resistance to ground at each end.
- Two-winding transformers: banks of single-phase units; the tap changes the
  rated voltage of its side where pandapower's power flow applies it
  (tap_changer_type "Ratio" or "Symmetrical" with tap_side "hv" or "lv");
  the phase shift sets the clock number.
- Bus-bus switches: closed ones join the buses (or, with z_ohm > 0, become an
  impedance as in pandapower); open line and transformer switches leave the
  end of the branch on a node of its own, as does an out-of-service bus at
  one end of a line. A branch without a closed end is left out, as is a
  transformer at an out-of-service bus (pandapower takes it out).

The bus of pandapower index k becomes the bus ``b<k>`` (buses joined by a
closed switch take the smallest index), the element of row k of table t the
component ``<t><k>``. ``Conversion.mapping`` lists both.

Example::

    import openkeito, pandapower.networks as pn
    from openkeito.pandapower import from_pandapower
    conv = from_pandapower(pn.case39(), end_time_s=0.2)
    result = openkeito.run(conv.model)
    conv.compare(result)["max_abs_dv_pu"]
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
    fault,
    finite,
    get,
    grid_following_converter,
    machine_rating,
    model_document,
    phasors,
    run_slices,
    schedule_slices,
    source_component,
    split_machine_parameters,
    summarize,
    synchronous_machine,
)

__all__ = [
    "UNSUPPORTED_TABLES", "TABLE_QUANTITIES", "Conversion", "ConversionError", "TimeSlice", "TimeSliceRun",
    "apply_schedule", "branch_powers", "check_schedule", "fault", "from_pandapower", "run_time_slices",
    "schedule_from_opf", "time_slices", "transition", "run_transitions",
]

#: Tables whose in-service elements the converter cannot represent yet.
UNSUPPORTED_TABLES = [
    "trafo3w", "impedance", "ward", "xward", "dcline", "svc", "tcsc", "ssc", "vsc", "vsc_stacked", "vsc_bipolar",
    "bus_dc", "line_dc", "source_dc", "load_dc",
    "asymmetric_load", "asymmetric_sgen", "motor",
]

#: Vector groups of three_phase_transformer, without the N of the star points.
_VECTOR_GROUPS = {"Yy0", "Yy6", "Yd1", "Yd5", "Yd7", "Yd11", "Dy1", "Dy5", "Dy7", "Dy11", "Dd0", "Dd6"}


def _in_service(row: Any) -> bool:
    return bool(row["in_service"]) if "in_service" in row.index else True


def _flag(value: Any) -> bool:
    """A cell of a boolean column that may hold None, NaN or pandas.NA."""
    try:
        return bool(value) and finite(value)
    except (TypeError, ValueError):
        return False


def _vector_group(letters: Any, shift_degree: float) -> tuple[str | None, str | None]:
    """(vector group, problem) from pandapower's letters (such as "Dyn",
    optional) and phase shift (the low-voltage side lags by shift_degree =
    clock * 30 degrees)."""
    clock_f = (shift_degree / 30.0) % 12.0
    clock = int(round(clock_f)) % 12
    if abs(clock_f - round(clock_f)) > 1e-9:
        return None, (f"phase shift {shift_degree} degrees is not a multiple of 30 "
                      "(phase-shifting transformers are not supported yet)")
    if isinstance(letters, str) and letters:
        hv = "YN" if letters.startswith("YN") else letters[:1]
        lv = letters[len(hv):]
        if lv in ("y", "yn", "d") and f"{hv[0]}{lv[0]}{clock}" in _VECTOR_GROUPS:
            return f"{hv}{lv}{clock}", None
    # Without letters (or with ones we cannot build): grounded stars for an
    # even clock, a delta on the high-voltage side otherwise, so that the
    # low-voltage network keeps a grounded star point.
    return (f"YNyn{clock}" if clock % 2 == 0 else f"Dyn{clock}"), None


def _taps(row: Any) -> tuple[dict[str, float], float, str | None, bool]:
    """What the tap changers of a transformer (tap_* and tap2_*) do in the
    power flow of pandapower 3: (factors on the rated voltages of "hv" and
    "lv", added phase shift in degrees, problem, ignored).

    pandapower applies a tap only with tap_changer_type "Ratio",
    "Symmetrical" or "Ideal" and tap_side "hv" or "lv"; `ignored` says that a
    tap away from its neutral position was left out for that reason."""
    factor = {"hv": 1.0, "lv": 1.0}
    shift = 0.0
    ignored = False
    if "tap_dependency_table" in row.index and _flag(row["tap_dependency_table"]):
        return factor, shift, "tap_dependency_table (tap changer characteristics) is not supported yet", ignored
    for t in ("", "2"):
        if f"tap{t}_pos" not in row.index:
            continue
        steps = get(row, f"tap{t}_pos", math.nan) - get(row, f"tap{t}_neutral", math.nan)
        percent, degree = get(row, f"tap{t}_step_percent", 0.0), get(row, f"tap{t}_step_degree", 0.0)
        if not finite(steps) or steps == 0 or (percent == 0 and degree == 0):
            continue
        kind = row[f"tap{t}_changer_type"] if f"tap{t}_changer_type" in row.index else None
        side = row[f"tap{t}_side"] if f"tap{t}_side" in row.index else None
        if kind not in ("Ratio", "Symmetrical", "Ideal") or side not in ("hv", "lv"):
            ignored = True
            continue
        direction = 1.0 if side == "hv" else -1.0
        if kind == "Ideal":
            # A phase shifter: by tap_step_degree, or by the angle that
            # tap_step_percent gives.
            shift += direction * (steps * degree if degree != 0
                                  else 2 * math.degrees(math.asin(steps * percent / 200)))
        else:
            # The added voltage du at the angle tap_step_degree.
            du = steps * percent / 100
            re, im = 1 + du * math.cos(math.radians(degree)), du * math.sin(math.radians(degree))
            factor[side] *= math.hypot(re, im)
            shift += math.degrees(math.atan(direction * im / re))
    return factor, shift, None, ignored


def from_pandapower(
    net: Any,
    *,
    time_step_s: float = 5e-5,
    end_time_s: float = 0.1,
    method: str = "2s-dirk",
    outputs: list[dict[str, Any]] | str = "buses",
    output_interval_steps: int = 1,
    run_power_flow: bool = True,
    power_flow_options: dict[str, Any] | None = None,
    generator_x_pu: float = 0.2,
    generator_r_pu: float = 0.005,
    ext_grid_sc_mva: float = 10000.0,
    ext_grid_rx: float = 0.1,
    generators: str = "source",
    machine_parameters: dict[str, Any] | None = None,
    name: str | None = None,
    inverter_model: str = "current_source",
    converter_parameters: dict[str, Any] | None = None,
) -> Conversion:
    """Converts ``net`` and its AC power flow into an OpenKeito model.

    run_power_flow: run ``pandapower.runpp`` first (default), which
    replaces ``net.res_*``. With False, ``net.res_*`` must hold a converged
    power flow, which is then left as it is.
    power_flow_options: keyword arguments of ``pandapower.runpp``, such as
    {"init": "results", "tolerance_mva": 1e-10}. The model needs the power
    flow's defaults calculate_voltage_angles=True and trafo_model="t".
    outputs: model outputs; "buses" records every bus voltage, "all" also the
    currents of lines and transformers; or a list of model outputs.
    generator_x_pu, generator_r_pu: internal impedance of generators without
    ``xdss_pu``, per unit of their rating (net.sn_mva when unknown).
    ext_grid_sc_mva, ext_grid_rx: short-circuit power and R/X of external
    grids without ``s_sc_max_mva``.
    generators: what the rows of ``net.gen`` become: "source" (default), a
    three-phase source behind the subtransient impedance, or "machine", a
    synchronous_machine with its rotor windings and inertia, which starts
    from the power flow with a constant field voltage and torque. External
    grids stay sources: they stand for a network, not for one machine.
    machine_parameters: parameters of the machines (fields of
    synchronous_machine, such as {"inertia_h_s": 6.0}); an entry
    {"gen/<index>": {...}} holds those of one generator. What neither they
    nor the network give (``sn_mva``, ``xdss_pu``, ``rdss_ohm``) is taken
    from ``openkeito.convert.TYPICAL_MACHINE`` and named in the warnings; a
    generator without ``sn_mva`` gets the rating that its output (or
    ``max_p_mw``, unless that is more than ten times the output) loads to
    85 %.
    inverter_model: what static generators and storage become.
    "current_source" (default): a balanced current injection, which keeps
    its current whatever the voltage does. "converter": the averaged
    grid-following converter (``grid_following_converter``, with its
    phase-locked loop, current control and current limit), rated at
    ``sn_mva`` (when missing: its apparent power / 0.9, with a warning) and
    the rated voltage of its bus; the model then has format version 0.5.
    converter_parameters: more fields of every such converter, for example
    {"current_limit_pu": 1.2, "priority": "active"}.
    """
    import pandapower as pp

    check_generators(generators)
    machine_common, machine_own = split_machine_parameters(machine_parameters)
    if machine_own:
        known_gens = {f"gen/{idx}" for idx in net.gen.index}
        unknown_gens = sorted(set(machine_own) - known_gens)
        if unknown_gens:
            raise ValueError(f"machine_parameters names no generator in the network: {', '.join(unknown_gens)}")
    check_inverter_model(inverter_model, converter_parameters)
    problems: list[str] = []
    for table in UNSUPPORTED_TABLES:
        frame = net[table] if table in net else None
        if frame is None or not len(frame):
            continue
        count = int(frame["in_service"].sum()) if "in_service" in frame else len(frame)
        if count:
            problems.append(f"{table}: {count} element(s) in service; this table is not supported yet")
    if problems:
        raise ConversionError(problems)

    if run_power_flow:
        try:
            pp.runpp(net, **(power_flow_options or {}))
        except pp.LoadflowNotConverged as e:
            raise ConversionError([f"the power flow did not converge ({e}); fix the network or pass "
                                   "power_flow_options"]) from e
    elif power_flow_options:
        raise ValueError("power_flow_options are for run_power_flow=True")
    if not getattr(net, "converged", True) or net.res_bus.empty:
        raise ConversionError(["the power flow did not converge; fix the network first"])
    # The options of the power flow that is in net.res_* (none: a network
    # loaded with its results).
    options = getattr(net, "_options", None) or {}

    f_hz = float(getattr(net, "f_hz", 50.0) or 50.0)
    w = 2 * math.pi * f_hz
    sn_system = float(getattr(net, "sn_mva", 1.0) or 1.0)
    # What was assumed, per kind: kind -> elements.
    assumed: dict[str, list[str]] = defaultdict(list)
    assumed_ratings: dict[str, dict[str, Any]] = {}

    # Closed bus-bus switches without impedance join their buses.
    parent = {int(b): int(b) for b in net.bus.index}

    def root(b: int) -> int:
        while parent[b] != b:
            parent[b] = parent[parent[b]]
            b = parent[b]
        return b

    switch_branches = []
    open_ends: dict[tuple[str, int, int], str] = {}
    for idx, sw in net.switch.iterrows():
        closed = bool(sw["closed"])
        if sw["et"] == "b":
            if not closed:
                continue
            if get(sw, "z_ohm", 0.0) > 0.0:
                switch_branches.append((idx, sw))
            else:
                a, b = root(int(sw["bus"])), root(int(sw["element"]))
                parent[max(a, b)] = min(a, b)
        elif not closed and sw["et"] in ("l", "t"):
            open_ends[(str(sw["et"]), int(sw["element"]), int(sw["bus"]))] = f"sw{idx}"

    res_bus = net.res_bus
    live = sorted(
        int(b) for b in net.bus.index
        if bool(net.bus.at[b, "in_service"]) and finite(res_bus.at[b, "vm_pu"])
    )
    live_set = set(live)
    bus_name = {b: f"b{root(b)}" for b in live}
    vn = {int(b): float(net.bus.at[b, "vn_kv"]) for b in net.bus.index}
    pf = {b: (float(res_bus.at[b, "vm_pu"]), float(res_bus.at[b, "va_degree"]), vn[b]) for b in live}

    def v_phase(b: int) -> complex:
        """Phase-to-neutral voltage phasor of a bus (rms, V)."""
        vm, va, kv = pf[b]
        return cmath.rect(vm * kv * 1e3 / math.sqrt(3), math.radians(va))

    components: list[dict[str, Any]] = []
    elements: list[dict[str, Any]] = []

    def add(table: str, idx: Any, row: Any, component: dict[str, Any] | list[dict[str, Any]],
            buses: list[int]) -> None:
        comps = component if isinstance(component, list) else [component]
        components.extend(comps)
        label = row["name"] if "name" in row.index else None
        entry: dict[str, Any] = {
            "element": f"{table}/{idx}",
            "name": label if isinstance(label, str) else "",
            "component": comps[0]["name"],
            "type": comps[0]["type"],
            "buses": [bus_name[b] for b in buses if b in bus_name],
        }
        if len(comps) > 1:
            entry["components"] = [c["name"] for c in comps]
        elements.append(entry)

    def source(table: str, idx: Any, row: Any, bus: int, s_mva: complex, z_ohm: complex) -> None:
        add(table, idx, row, source_component(
            f"{table}{idx}", bus_name[bus], v_phase(bus), s_mva, z_ohm, f_hz), [bus])

    def machine(table: str, idx: Any, row: Any, bus: int, s_mva: complex) -> None:
        """A generator as a synchronous machine at the power flow of its bus."""
        own = machine_own.get(f"{table}/{idx}", {})
        chosen = {**machine_common, **own}
        sn, assumed_rating = machine_rating(chosen.pop("rated_power_mva", get(row, "sn_mva", math.nan)), s_mva,
                                            get(row, "max_p_mw", math.nan))
        if assumed_rating:
            assumed["no rating; assumed that the output of the power flow (or max_p_mw) is 85 % of "
                    "it"].append(f"{table}/{idx} ({sn:.4g} MVA)")
            assumed_ratings[f"{table}/{idx}"] = {"component": f"{table}{idx}", "xd_from_ohm": False}
        z_base = vn[bus] ** 2 / sn
        data = {}
        xdss = get(row, "xdss_pu", math.nan)
        if finite(xdss) and xdss > 0:
            data["xd_subtransient_pu"] = data["xq_subtransient_pu"] = xdss
        rdss = get(row, "rdss_ohm", math.nan)
        if finite(rdss) and rdss >= 0:
            data["ra_pu"] = rdss / z_base
        component, notes = synchronous_machine(
            f"{table}{idx}", bus_name[bus], frequency_hz=f_hz, rated_power_mva=sn, rated_voltage_kv=vn[bus],
            s_mva=s_mva, voltage_pu=pf[bus][0], angle_deg=pf[bus][1], data=data, parameters=chosen)
        for note in notes:
            assumed[note].append(f"{table}/{idx}")
        add(table, idx, row, component, [bus])

    def injection(table: str, idx: Any, row: Any, bus: int, s_injected_mva: complex) -> None:
        if s_injected_mva == 0:
            return
        if inverter_model == "converter" and table in ("sgen", "storage"):
            sn = get(row, "sn_mva", float("nan"))
            v_ll = cmath.rect(pf[bus][0] * vn[bus], math.radians(pf[bus][1]))
            add(table, idx, row, grid_following_converter(
                f"{table}{idx}", bus_name[bus], s_injected_mva, v_ll, f_hz, sn if finite(sn) else None, vn[bus],
                converter_parameters, assumed, f"{table}/{idx}"), [bus])
            return
        i = (s_injected_mva * 1e6 / (3 * v_phase(bus))).conjugate()
        add(table, idx, row, {
            "type": "three_phase_current_source", "name": f"{table}{idx}", "bus": bus_name[bus],
            "current_rms_a": abs(i), "frequency_hz": f_hz, "angle_deg": math.degrees(cmath.phase(i)),
        }, [bus])

    def load(table: str, idx: Any, row: Any, bus: int, s_mva: complex) -> None:
        comps = consumption_components(
            f"{table}{idx}", bus_name[bus], pf[bus][0] * vn[bus], v_phase(bus), s_mva, f_hz,
            assumed, f"{table}/{idx}", "negative active power, represented as a current injection")
        if comps:
            add(table, idx, row, comps, [bus])

    # With neglect_open_switch_branches the power flow takes a branch with
    # an open switch out; otherwise it leaves the open end on a bus of its own.
    neglect_open = bool(options.get("neglect_open_switch_branches", False))

    def ends(et: str, idx: Any, buses: tuple[int, int]) -> tuple[list[str], list[bool]]:
        """(node names of the two ends, whether each is closed): an end is
        open at an open switch or at an out-of-service bus and then has a
        node of its own."""
        names, closed = [], []
        for b in buses:
            switch = open_ends.get((et, int(idx), b))
            closed.append(switch is None and b in live_set)
            names.append(bus_name[b] if closed[-1] else switch or f"{'line' if et == 'l' else 'trafo'}{idx}_open")
        if neglect_open and any((et, int(idx), b) in open_ends for b in buses):
            closed = [False, False]
        return names, closed

    # External grids and generators.
    for idx, row in net.ext_grid.iterrows():
        b = int(row["bus"])
        if not _in_service(row) or b not in live_set:
            continue
        r = net.res_ext_grid.loc[idx]
        sc, rx = get(row, "s_sc_max_mva", math.nan), get(row, "rx_max", math.nan)
        if not finite(sc) or sc <= 0:
            sc, rx = ext_grid_sc_mva, ext_grid_rx
            assumed[f"no short-circuit data; assumed {sc:g} MVA with R/X {rx:g}"].append(f"ext_grid/{idx}")
        elif not finite(rx):
            rx = ext_grid_rx
        x = (vn[b] * 1e3) ** 2 / (sc * 1e6) / math.sqrt(1 + rx * rx)
        source("ext_grid", idx, row, b, complex(r["p_mw"], r["q_mvar"]), complex(rx * x, x))
    for idx, row in net.gen.iterrows():
        b = int(row["bus"])
        if not _in_service(row) or b not in live_set:
            continue
        r = net.res_gen.loc[idx]
        if generators == "machine":
            machine("gen", idx, row, b, complex(r["p_mw"], r["q_mvar"]))
            continue
        sn = get(row, "sn_mva", math.nan)
        if not finite(sn) or sn <= 0:
            sn = sn_system
            assumed[f"no rating; assumed sn_mva = net.sn_mva = {sn:g}"].append(f"gen/{idx}")
        z_base = (vn[b] * 1e3) ** 2 / (sn * 1e6)
        xdss = get(row, "xdss_pu", math.nan)
        if finite(xdss) and xdss > 0:
            z = complex(get(row, "rdss_ohm", 0.0), xdss * z_base)
        else:
            z = complex(generator_r_pu, generator_x_pu) * z_base
            assumed[f"no subtransient reactance; assumed r = {generator_r_pu:g}, x = {generator_x_pu:g} pu "
                    "on the rating"].append(f"gen/{idx}")
        source("gen", idx, row, b, complex(r["p_mw"], r["q_mvar"]), z)

    # Loads, shunts, static generators and storage.
    for idx, row in net.load.iterrows():
        b = int(row["bus"])
        if _in_service(row) and b in live_set:
            r = net.res_load.loc[idx]
            load("load", idx, row, b, complex(r["p_mw"], r["q_mvar"]))
    for idx, row in net.shunt.iterrows():
        b = int(row["bus"])
        if _in_service(row) and b in live_set:
            r = net.res_shunt.loc[idx]
            load("shunt", idx, row, b, complex(r["p_mw"], r["q_mvar"]))
    for idx, row in net.sgen.iterrows():
        b = int(row["bus"])
        if _in_service(row) and b in live_set:
            r = net.res_sgen.loc[idx]
            injection("sgen", idx, row, b, complex(r["p_mw"], r["q_mvar"]))
    for idx, row in net.storage.iterrows():
        b = int(row["bus"])
        if _in_service(row) and b in live_set:
            r = net.res_storage.loc[idx]
            # p_mw > 0 charges (consumes); the injection is its negative.
            injection("storage", idx, row, b, -complex(r["p_mw"], r["q_mvar"]))

    # Lines: per-circuit values, then `parallel` circuits in parallel.
    for idx, row in net.line.iterrows():
        fb, tb = int(row["from_bus"]), int(row["to_bus"])
        names, closed = ends("l", idx, (fb, tb))
        if not _in_service(row) or not any(closed):
            continue
        if names[0] == names[1]:
            assumed["both ends on the same bus; left out"].append(f"line/{idx}")
            continue
        n = get(row, "parallel", 1.0)
        length = float(row["length_km"])
        r1, x1 = float(row["r_ohm_per_km"]), float(row["x_ohm_per_km"])
        c1 = get(row, "c_nf_per_km", 0.0) * 1e-9
        if x1 <= 0 or length <= 0:
            problems.append(f"line/{idx}: x_ohm_per_km and length_km must be > 0")
            continue
        g1 = get(row, "g_us_per_km", 0.0) * 1e-6
        if g1 < 0:
            problems.append(f"line/{idx}: g_us_per_km must be >= 0")
            continue
        r0, x0 = get(row, "r0_ohm_per_km", math.nan), get(row, "x0_ohm_per_km", math.nan)
        c0 = get(row, "c0_nf_per_km", math.nan) * 1e-9
        if not (finite(r0) and finite(x0) and x0 > 0):
            r0, x0, c0 = 3 * r1, 3 * x1, 0.6 * c1
            assumed["no zero-sequence data; assumed R0 = 3 R1, X0 = 3 X1, C0 = 0.6 C1"].append(f"line/{idx}")
        elif not (finite(c0) and c0 > 0):
            c0 = 0.6 * c1
        if c1 > 0:
            comp = {
                "type": "pi_line", "name": f"line{idx}", "buses": names, "length_km": length,
                "per_km": {"type": "sequence",
                           "r1_ohm_per_km": r1 / n, "l1_h_per_km": x1 / n / w, "c1_f_per_km": c1 * n,
                           "r0_ohm_per_km": r0 / n, "l0_h_per_km": x0 / n / w, "c0_f_per_km": c0 * n},
            }
        else:
            comp = {
                "type": "coupled_rl", "name": f"line{idx}", "buses": names,
                "impedance": {"type": "sequence",
                              "r1_ohm": r1 * length / n, "l1_h": x1 * length / n / w,
                              "r0_ohm": r0 * length / n, "l0_h": x0 * length / n / w},
            }
        line_comps = [comp]
        if g1 > 0:
            # Half the shunt conductance at each end, as the capacitance of
            # the pi line: a resistance to ground, written as the load that
            # takes G V^2 at the rated voltage.
            for k, b in enumerate((fb, tb)):
                line_comps.append({
                    "type": "three_phase_load", "name": f"line{idx}_g{k}", "bus": names[k],
                    "rated_voltage_kv": vn[b], "rated_frequency_hz": f_hz,
                    "active_power_mw": g1 * length * n / 2 * vn[b] ** 2, "reactive_power_mvar": 0.0,
                })
        add("line", idx, row, line_comps, [fb, tb])

    # Bus-bus switches with an impedance, as pandapower: |Z| = z_ohm with
    # R/X = switch_rx_ratio (2 by default).
    rx_switch = float((getattr(net, "_options", None) or {}).get("switch_rx_ratio", 2.0) or 2.0)
    for idx, sw in switch_branches:
        a, b = int(sw["bus"]), int(sw["element"])
        if a not in live_set or b not in live_set:
            continue
        x = float(sw["z_ohm"]) / math.sqrt(1 + rx_switch ** 2)
        add("switch", idx, sw, {
            "type": "coupled_rl", "name": f"switch{idx}", "buses": [bus_name[a], bus_name[b]],
            "impedance": {"type": "sequence", "r1_ohm": rx_switch * x, "l1_h": x / w,
                          "r0_ohm": rx_switch * x, "l0_h": x / w},
        }, [a, b])

    # Two-winding transformers.
    angles = bool(options.get("calculate_voltage_angles", True))
    trafo_model = options.get("trafo_model", "t")
    for idx, row in net.trafo.iterrows():
        hb, lb = int(row["hv_bus"]), int(row["lv_bus"])
        # pandapower takes a transformer at an out-of-service bus out.
        if not _in_service(row) or hb not in live_set or lb not in live_set:
            continue
        names, closed = ends("t", idx, (hb, lb))
        if not any(closed):
            continue
        n = get(row, "parallel", 1.0)
        factor, tap_shift, problem, ignored = _taps(row)
        if problem:
            problems.append(f"trafo/{idx}: {problem}")
            continue
        if ignored:
            assumed["tap position left out as in pandapower's power flow, which applies a tap only with "
                    "tap_changer_type Ratio, Symmetrical or Ideal and tap_side hv or lv"].append(f"trafo/{idx}")
        vhv, vlv = float(row["vn_hv_kv"]) * factor["hv"], float(row["vn_lv_kv"]) * factor["lv"]
        shift = get(row, "shift_degree", 0.0) + tap_shift
        if shift != 0 and not angles:
            problems.append(f"trafo/{idx}: the power flow ran with calculate_voltage_angles=False, without the "
                            f"phase shift of {shift:g} degrees; run it with calculate_voltage_angles=True")
            continue
        letters = row["vector_group"] if "vector_group" in row.index else None
        group, problem = _vector_group(letters, shift)
        if problem:
            problems.append(f"trafo/{idx}: {problem}")
            continue
        if not (isinstance(letters, str) and group.startswith(letters)):
            assumed["no usable vector group; assumed YNyn (even clock) or Dyn (odd clock)"].append(f"trafo/{idx}")
        if not all(closed):
            open_side = closed.index(False)
            grounded = group.startswith("YN") if open_side == 0 else "yn" in group
            if not grounded:
                # The open winding (a delta, or a star without a grounded
                # star point) would float: the transformer is what it takes
                # at no load from the closed side in the power flow.
                r = net.res_trafo.loc[idx]
                side = ("hv", "lv")[1 - open_side]
                s = complex(r[f"p_{side}_mw"], r[f"q_{side}_mvar"])
                assumed["open at a winding without a grounded star point; represented by its no-load impedance "
                        "at the closed side"].append(f"trafo/{idx} ({s.real:.3g} MW, {s.imag:.3g} Mvar)")
                if finite(s.real) and finite(s.imag) and abs(s) > 1e-12:
                    load("trafo", idx, row, (hb, lb)[1 - open_side], s)
                continue
        # pandapower takes the magnetizing susceptance as -sqrt(Ym^2 - G^2)
        # with Ym from i0_percent and G from pfe_kw: the sign of i0_percent
        # does not matter, and below the loss the susceptance is 0.
        i0 = get(row, "i0_percent", 0.0)
        pfe = get(row, "pfe_kw", 0.0)
        if i0 < 0:
            assumed["negative i0_percent taken by its magnitude, as pandapower does"].append(f"trafo/{idx}")
        i0 = max(abs(i0), 100 * pfe / (1000 * float(row["sn_mva"])))
        if (i0 > 0 or pfe > 0) and trafo_model != "t":
            problems.append(f"trafo/{idx}: the power flow ran with trafo_model={trafo_model!r}; the model has the "
                            "magnetizing branch of trafo_model='t' (the default)")
            continue
        add("trafo", idx, row, {
            "type": "three_phase_transformer", "name": f"trafo{idx}", "buses": names,
            "vector_group": group, "rated_voltage_kv": [vhv, vlv],
            "rated_power_mva": float(row["sn_mva"]) * n, "rated_frequency_hz": f_hz,
            "impedance_percent": float(row["vk_percent"]),
            "resistance_percent": get(row, "vkr_percent", 0.0),
            "no_load_current_percent": i0,
            "no_load_loss_kw": pfe * n,
        }, [hb, lb])

    if problems:
        raise ConversionError(problems)

    model_buses = sorted(set(bus_name.values()), key=lambda s: int(s[1:]))
    if outputs in ("buses", "all"):
        outs: list[dict[str, Any]] = [{"type": "bus_voltage", "bus": b} for b in model_buses]
        if outputs == "all":
            outs += [{"type": "branch_current", "component": c["name"]} for c in components
                     if c["type"] in ("pi_line", "coupled_rl", "three_phase_transformer")]
    else:
        outs = list(outputs)

    warnings = summarize(assumed)

    model = model_document(
        name or str(getattr(net, "name", "") or "pandapower network"),
        "Converted from pandapower by openkeito.pandapower.from_pandapower; starts from its power flow.",
        components,
        outs,
        time_step_s=time_step_s,
        end_time_s=end_time_s,
        method=method,
        output_interval_steps=output_interval_steps,
    )
    mapping = {
        "source": "pandapower",
        "frequency_hz": f_hz,
        "buses": {str(b): bus_name[b] for b in live},
        "elements": elements,
    }
    return Conversion(model=model, mapping=mapping, warnings=warnings,
                      power_flow={str(b): pf[b] for b in live}, assumed_machine_ratings=assumed_ratings)


def branch_powers(conversion: Conversion, result: Any) -> dict[str, list[complex]]:
    """Three-phase complex powers (MVA) flowing from each bus into the
    converted lines and transformers, from the steady-state phasors of a run
    with ``outputs="all"``: component -> [S at buses[0], S at buses[1]].
    Comparable with pandapower's res_line / res_trafo p and q."""
    ph = phasors(result)
    out: dict[str, list[complex]] = {}
    for c in conversion.model["components"]:
        if c["type"] not in ("pi_line", "coupled_rl", "three_phase_transformer"):
            continue
        powers = []
        for k, bus in enumerate(c["buses"]):
            if c["type"] == "coupled_rl":
                i = ph.get(f"I({c['name']}.a)")
                i = None if i is None else (i if k == 0 else -i)
            else:
                i = ph.get(f"I({c['name']}.{k}.a)")
            v = ph.get(f"V({bus}.a)")
            if i is None or v is None:
                break
            powers.append(3 * v * i.conjugate() / 1e6)
        if len(powers) == 2:
            out[c["name"]] = powers
    return out


# Schedules -------------------------------------------------------------------

#: Scheduled quantities of each pandapower table and their columns. Values
#: keep the sign of the table: p_mw of gen and sgen is generated, of load
#: and storage consumed (storage: charging).
TABLE_QUANTITIES: dict[str, dict[str, str]] = {
    "gen": {"active_power_mw": "p_mw", "voltage_pu": "vm_pu", "in_service": "in_service"},
    "ext_grid": {"voltage_pu": "vm_pu", "in_service": "in_service"},
    "load": {"active_power_mw": "p_mw", "reactive_power_mvar": "q_mvar", "in_service": "in_service"},
    "sgen": {"active_power_mw": "p_mw", "reactive_power_mvar": "q_mvar", "in_service": "in_service"},
    "storage": {"active_power_mw": "p_mw", "reactive_power_mvar": "q_mvar", "in_service": "in_service"},
    "shunt": {"active_power_mw": "p_mw", "reactive_power_mvar": "q_mvar", "in_service": "in_service"},
    "trafo": {"tap_position": "tap_pos", "in_service": "in_service"},
    "line": {"in_service": "in_service"},
}


def _target(net: Any, target: str, quantity: str) -> tuple[str, int, str] | str:
    """(table, index, column) of a series, or what is wrong with it."""
    table, sep, index = target.partition("/")
    if not sep or not index.isdigit():
        return f"target {target!r} is not '<table>/<index>', such as 'gen/0'"
    if table not in TABLE_QUANTITIES:
        return f"target {target!r}: table {table!r} cannot be scheduled (tables: {', '.join(TABLE_QUANTITIES)})"
    if table not in net or int(index) not in net[table].index:
        return f"target {target!r}: the network has no {table} with index {index}"
    column = TABLE_QUANTITIES[table].get(quantity)
    if column is None:
        return (f"target {target!r}: {quantity} cannot be scheduled for {table} "
                f"(quantities: {', '.join(TABLE_QUANTITIES[table])})")
    return table, int(index), column


def check_schedule(net: Any, schedule: Any) -> None:
    """Raises ConversionError unless every series of the schedule names an
    element of the network and a quantity of its table."""
    problems = []
    for k, s in enumerate(schedule.series):
        found = _target(net, s["target"], s["quantity"])
        if isinstance(found, str):
            problems.append(f"series[{k}]: {found}")
    if problems:
        raise ConversionError(problems)


def _scale(net: Any, table: str, index: int) -> float:
    """The factor by which pandapower's power flow multiplies p_mw and q_mvar
    of an element: its ``scaling``, or the ``step`` of a shunt."""
    column = "step" if table == "shunt" else "scaling"
    if table not in ("load", "sgen", "storage", "gen", "shunt") or column not in net[table].columns:
        return 1.0
    return get(net[table].loc[index], column, 1.0)


def apply_schedule(net: Any, schedule: Any, time: Any) -> Any:
    """A copy of ``net`` with the values of the schedule at ``time``.

    A scheduled active or reactive power is the power that enters the power
    flow (for a shunt: at its rated voltage). pandapower multiplies p_mw and
    q_mvar by the element's ``scaling`` (a shunt's by its ``step``), so the
    copy gets the scheduled value divided by it; with a scaling or step of 0
    only a power of 0 can be scheduled."""
    import copy

    check_schedule(net, schedule)
    out = copy.deepcopy(net)
    problems = []
    for (target, quantity), value in schedule.values_at(time).items():
        table, index, column = _target(out, target, quantity)
        if quantity == "in_service":
            value = bool(value)
        elif quantity == "tap_position":
            value = int(round(value))
        elif quantity in ("active_power_mw", "reactive_power_mvar"):
            scale = _scale(out, table, index)
            if scale != 0:
                value = value / scale
            elif value != 0:
                problems.append(f"{target}: {quantity} = {value:g} cannot be scheduled, because the "
                                f"{'step' if table == 'shunt' else 'scaling'} of the element is 0")
        out[table].at[index, column] = value
    if problems:
        raise ConversionError(problems)
    return out


def schedule_from_opf(
    net: Any,
    inputs: Any,
    *,
    opf: str = "ac",
    slack_tolerance: float = 0.01,
    name: str | None = None,
    **opf_options: Any,
) -> tuple[Any, list[str]]:
    """Generator set points from an optimal power flow at every time of
    ``inputs`` (a Schedule of the loads and other inputs; its values are
    applied first). Returns (schedule, warnings): the schedule holds the
    inputs and, for every time, active_power_mw of the generators (and of
    controllable static generators, storage and loads) and, for opf="ac",
    voltage_pu of generators and external grids and reactive_power_mvar of
    the controllable static generators, storage and loads. The powers are
    those of the OPF's result, as ``apply_schedule`` takes them, so the power
    flow of a time slice is the OPF's. An element that is out of service at
    a time keeps its set point there; one that is out at every time gets no
    series.

    opf="dc" solves ``rundcopp``; reactive power and voltages then come from
    the AC power flow of each time slice, and a warning lists the times at
    which the slack moved by more than ``slack_tolerance`` of the total
    generation from the plan.
    """
    import pandapower as pp

    from .schedule import Schedule

    if opf not in ("ac", "dc"):
        raise ValueError("opf is 'ac' or 'dc'")
    values: dict[tuple[str, str], list[float]] = {
        (s["target"], s["quantity"]): list(s["values"]) for s in inputs.series
    }
    added: dict[tuple[str, str], list[float]] = defaultdict(list)
    # The series with a result of the OPF at one time at least.
    solved: set[tuple[str, str]] = set()
    problems: list[str] = []
    warnings: list[str] = []
    slack_moved = []

    def record(table: str, idx: Any, row: Any, quantity: str, result: Any, set_point: float) -> None:
        """The OPF's result of an element in service; otherwise (out of
        service, or at a bus without voltage) its set point, so that the
        series has a value at every time."""
        key = (f"{table}/{idx}", quantity)
        if _in_service(row) and finite(result):
            added[key].append(float(result))
            solved.add(key)
        else:
            added[key].append(float(set_point))

    for time in inputs.times:
        n = apply_schedule(net, inputs, time)
        try:
            (pp.runopp if opf == "ac" else pp.rundcopp)(n, **opf_options)
        except Exception as e:  # pandapower raises OPFNotConverged and others
            problems.append(f"{time}: the {opf.upper()} optimal power flow failed ({e})")
            continue
        for table in ("gen", "sgen", "storage", "load"):
            res = n[f"res_{table}"]
            for idx, row in n[table].iterrows():
                if table != "gen" and not ("controllable" in row.index and _flag(row["controllable"])):
                    continue
                scale = _scale(n, table, idx)
                record(table, idx, row, "active_power_mw", res.at[idx, "p_mw"], get(row, "p_mw", 0.0) * scale)
                if opf == "ac" and table != "gen":
                    record(table, idx, row, "reactive_power_mvar", res.at[idx, "q_mvar"],
                           get(row, "q_mvar", 0.0) * scale)
        if opf == "ac":
            for idx, row in n.gen.iterrows():
                record("gen", idx, row, "voltage_pu", n.res_gen.at[idx, "vm_pu"], get(row, "vm_pu", 1.0))
            for idx, row in n.ext_grid.iterrows():
                record("ext_grid", idx, row, "voltage_pu", n.res_bus.at[int(row["bus"]), "vm_pu"],
                       get(row, "vm_pu", 1.0))
        else:
            planned = float(n.res_ext_grid.p_mw.sum())
            total = planned + float(n.res_gen.p_mw.sum()) + float(n.res_sgen.p_mw.sum())
            for idx, row in n.gen.iterrows():
                scale = _scale(n, "gen", idx)
                if _in_service(row) and finite(n.res_gen.at[idx, "p_mw"]) and scale != 0:
                    n.gen.at[idx, "p_mw"] = float(n.res_gen.at[idx, "p_mw"]) / scale
            pp.runpp(n)
            moved = float(n.res_ext_grid.p_mw.sum()) - planned
            if abs(moved) > slack_tolerance * max(abs(total), 1e-9):
                slack_moved.append(f"{time} ({moved:+.1f} MW)")
    if problems:
        raise ConversionError(problems)
    if opf == "dc":
        warnings.append("DC optimal power flow: reactive power and voltages come from the AC power flow "
                        "of each time slice")
    if slack_moved:
        warnings.append(f"the slack moved by more than {100 * slack_tolerance:g} % of the generation from "
                        f"the DC plan at: {', '.join(slack_moved)}")
    values.update({key: series for key, series in added.items() if key in solved})
    schedule = Schedule.from_values(
        inputs.times, values, name=name or f"{inputs.name or 'schedule'} ({opf.upper()} OPF)",
        description=f"Inputs of {inputs.name or 'the input schedule'} and the generator set points of "
                    f"pandapower's {opf.upper()} optimal power flow at each time.",
        interpolation=inputs.interpolation,
    )
    return schedule, warnings


# Time slices -----------------------------------------------------------------


def time_slices(
    net: Any,
    schedule: Any,
    disturbances: list[dict[str, Any]] | None = None,
    *,
    times: list[Any] | None = None,
    **convert_options: Any,
) -> list[TimeSlice]:
    """For every time (default: every time of the schedule) and every
    disturbance (default: none), the network with the schedule's values,
    its AC power flow, and the converted model with the disturbance's
    components and outputs added.

    A disturbance is {"name": ..., "components": [...], "outputs": [...]};
    a component's "bus" (or an entry of "buses") that is a pandapower bus
    index becomes its model bus. ``fault`` makes one; disturbances need
    names of their own. convert_options go to ``from_pandapower``. A time
    whose power flow or conversion fails raises ConversionError with the
    time.
    """
    return schedule_slices(net, schedule, disturbances, times, check=check_schedule, apply=apply_schedule,
                           convert=from_pandapower, convert_options=convert_options)


def run_time_slices(
    net: Any,
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
    slices = time_slices(net, schedule, disturbances, times=times, **convert_options)
    return run_slices(slices, jobs=jobs, out=out, force=force, keep_rows=keep_rows)


# Transitions -----------------------------------------------------------------


def transition(net: Any, schedule: Any, from_time: Any = None, to_time: Any = None, *, times: Any = None,
               name: str | None = None, **options: Any) -> Any:
    """The model that starts from the power flow of ``from_time`` and moves
    the set points to the schedule's values of ``to_time`` (default: the
    first two times of the schedule): an ``openkeito.transition.Transition``
    with the model, the mapping, the warnings and the conversions (with
    their power flows) at both times.

    times: instead of from_time and to_time, a list of two or more times of
    the schedule (or "all"): one model that goes through all of them, each
    with its changes and its settling time
    (``openkeito.transition.build_sequence``); ramp_time_s,
    ramp_rate_mw_per_min, settle_time_s and switch_delay_s then also take a
    list with one entry per interval.

    options: those of ``openkeito.transition.build`` (start_time_s,
    ramp_time_s, ramp_rate_mw_per_min, settle_time_s, end_time_s,
    switch_time_s, switch_delay_s, load_steps, exciter, governor, pss,
    machine_controls, sources) and
    those of ``from_pandapower``; unless given, generators="machine",
    inverter_model="converter" and output_interval_steps=20. Elements are
    named as in the schedule ("gen/0") where an option takes them."""
    from .transition import make_transition

    return make_transition(net, schedule, from_time, to_time, times=times, check=check_schedule,
                           apply=apply_schedule, convert=from_pandapower, name=name, options=options)


def run_transitions(net: Any, schedule: Any, intervals: Any = None, *, times: Any = None, chain: bool = False,
                    jobs: int | None = None, out: Any = None, force: bool = False, window_s: float | None = None,
                    **options: Any) -> Any:
    """Makes the transition (see ``transition``) of every pair of
    neighbouring times of the schedule, or of the ``intervals`` given as
    (from_time, to_time) pairs, and runs them in parallel. Returns an
    ``openkeito.transition.TransitionRun``; each of its rows says whether
    the start is the power flow of from_time, how close the end comes to the
    power flow of to_time and what the voltages, the frequency and the
    currents reach on the way (``openkeito.transition.measure``).

    chain=True: one run (the case ``chain``) through all times of the
    schedule instead, or through the ``times`` given (which imply it), or
    through the intervals when each begins where the one before ends. Its
    ``time_rows`` have one row per time, measured at the end of the
    settling time of that time against its power flow. The run is as long
    as all changes and settling times together, on one core, where the
    intervals as cases of their own use several."""
    from .transition import make_and_run

    return make_and_run(net, schedule, intervals, make=transition, jobs=jobs, out=out, force=force,
                        window_s=window_s, options=options, times=times, chain=chain,
                        check=check_schedule, apply=apply_schedule, convert=from_pandapower, rename_buses=False)
