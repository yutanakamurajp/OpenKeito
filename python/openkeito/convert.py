"""What the converters from power-flow tools have in common.

A converter (``openkeito.pandapower``, ``openkeito.powsybl``) returns a
``Conversion``: the model, the table from the tool's elements to the model's
components and buses (``mapping``), the assumptions it made (``warnings``)
and the power-flow voltages, against which ``compare`` checks the steady
state of a run.
"""

from __future__ import annotations

import cmath
import math
import numbers
from dataclasses import dataclass, field
from collections.abc import Callable
from typing import Any

__all__ = [
    "Conversion", "ConversionError", "TimeSlice", "TimeSliceRun", "fault", "make_time_slices", "phasors",
    "place", "run_slices", "schedule_slices", "INVERTER_MODELS", "grid_following_converter", "model_version",
    "machine_rating", "machine_rating_for", "MACHINE_LOADING", "source_component", "consumption_components",
    "model_document",
]


class ConversionError(ValueError):
    """The network cannot be converted; ``problems`` lists what to change."""

    def __init__(self, problems: list[str]):
        self.problems = problems
        super().__init__("cannot convert the network:\n- " + "\n- ".join(problems))


def phasors(result: Any) -> dict[str, complex]:
    """The steady-state phasors (rms, cosine reference) of a run started with
    ``"initialization": "steady_state"``, by column name."""
    summary = result.summary if hasattr(result, "summary") else result
    init = summary.get("initialization") or {}
    return {p["name"]: cmath.rect(p["rms"], math.radians(p["angle_deg"])) for p in init.get("phasors", [])}


@dataclass
class Conversion:
    """An OpenKeito model made from a power-flow tool's network, the table
    from its elements to the model components, and what was assumed.

    mapping: {"source": tool, "frequency_hz": f, "buses": {bus key: model
    bus}, "elements": [{"element", "name", "component", "type", "buses"}]}.
    The bus key is the pandapower index (as text) or the IIDM/CGMES bus id.
    """

    model: dict[str, Any]
    mapping: dict[str, Any]
    warnings: list[str] = field(default_factory=list)
    #: Power flow per bus key: (vm_pu, va_degree, rated line-to-line kV).
    #: Where the PowSyBl converter turns the model by the phase-angle clocks
    #: of the transformers, the angle is the turned one (the load flow's plus
    #: ``mapping["bus_phase_shift_deg"]``).
    power_flow: dict[str, tuple[float, float, float]] = field(default_factory=dict)
    #: The machines whose rating the data does not give, so that it was
    #: assumed (``machine_rating``): {key of the element in the option
    #: ``machine_parameters``: {"component": name, "xd_from_ohm": whether
    #: its synchronous reactance is data in ohm}}. A transition rates these
    #: machines for all its times (``machine_rating_for``).
    assumed_machine_ratings: dict[str, dict[str, Any]] = field(default_factory=dict)

    def bus(self, key: Any) -> str:
        """Name of the model bus of a pandapower bus index or IIDM bus id."""
        return self.mapping["buses"][str(key)]

    def element_of(self) -> dict[str, str]:
        """{component name: element name} from ``mapping['elements']``."""
        return {e["component"]: e["element"] for e in self.mapping.get("elements", [])}

    def compare(self, result: Any) -> dict[str, Any]:
        """Bus voltages of the power flow against the steady state of an
        OpenKeito run (phase a of its initialization phasors). Returns the
        largest differences and one row per bus."""
        ph = phasors(result)
        rows = []
        for key, name in self.mapping["buses"].items():
            vm, va, vn = self.power_flow[key]
            v = ph.get(f"V({name}.a)")
            if v is None:
                continue
            vm_emt = abs(v) * math.sqrt(3) / (vn * 1e3)
            va_emt = math.degrees(cmath.phase(v))
            rows.append({
                "bus": key, "node": f"{name}.a",
                "vm_pu_power_flow": vm, "vm_pu_emt": vm_emt,
                "va_degree_power_flow": va, "va_degree_emt": va_emt,
                "dv_pu": vm_emt - vm,
                "dva_degree": (va_emt - va + 180.0) % 360.0 - 180.0,
            })
        if not rows:
            raise ValueError("the result has no bus voltage phasors: run with outputs='buses' and "
                             "initialization 'steady_state'")
        return {
            "max_abs_dv_pu": max(abs(r["dv_pu"]) for r in rows),
            "max_abs_dva_degree": max(abs(r["dva_degree"]) for r in rows),
            "buses": rows,
        }

    def source_powers(self, result: Any) -> dict[str, complex]:
        """Three-phase complex power (MVA) that each three-phase source,
        current source and converter of the model delivers into its bus,
        from the steady-state phasors (outputs must include their branch
        currents)."""
        ph = phasors(result)
        out = {}
        for c in self.model["components"]:
            if c["type"] not in ("three_phase_source", "three_phase_current_source", "synchronous_machine",
                                 "grid_following_converter"):
                continue
            v, i = ph.get(f"V({c['bus']}.a)"), ph.get(f"I({c['name']}.a)")
            if v is None or i is None:
                continue
            # The current of a source or a machine flows from the bus into
            # it; that of a current source or a converter is its injection.
            sign = 1.0 if c["type"] in ("three_phase_current_source", "grid_following_converter") else -1.0
            out[c["name"]] = 3 * v * (sign * i).conjugate() / 1e6
        return out


def finite(v: Any) -> bool:
    try:
        return v is not None and math.isfinite(float(v))
    except (TypeError, ValueError):
        return False


def get(row: Any, column: str, default: float) -> float:
    """row[column] as a float, or default when missing or not finite."""
    v = row[column] if column in row.index else None
    return float(v) if finite(v) else default


def summarize(assumed: dict[str, list[str]]) -> list[str]:
    """One warning per kind of assumption, naming up to five elements."""
    out = []
    for what, items in assumed.items():
        shown = ", ".join(items[:5]) + (f" and {len(items) - 5} more" if len(items) > 5 else "")
        out.append(f"{what}: {shown}")
    return out


# Inverters as converters -----------------------------------------------------

#: What a static generator, a storage unit or a battery becomes
#: (``inverter_model`` of the converters).
INVERTER_MODELS = ("current_source", "converter")

#: Component types that need the model format 0.5.
FORMAT_0_5_TYPES = {"grid_following_converter", "synchronous_machine"}


def _needs_0_6(component: dict[str, Any]) -> bool:
    return bool(
        component.get("zero_sequence_inverted")
        or component.get("tap_changes")
        or component.get("impedance_changes")
    )


def model_version(components: list[dict[str, Any]]) -> str:
    """The model format version that the components need."""
    if any(_needs_0_6(c) for c in components):
        return "0.6"
    return "0.5" if any(c["type"] in FORMAT_0_5_TYPES for c in components) else "0.4"


def version_rank(version: Any) -> tuple[int, ...]:
    """The rank of a format version: "0.10" is later than "0.5" and "0.9" (a
    version that is not one ranks lowest)."""
    try:
        return tuple(int(x) for x in str(version).split("."))
    except ValueError:
        return ()


def model_document(
    name: str,
    description: str,
    components: list[dict[str, Any]],
    outputs: list[dict[str, Any]],
    *,
    time_step_s: float,
    end_time_s: float,
    method: str,
    output_interval_steps: int,
) -> dict[str, Any]:
    """Assemble an OpenKeito model JSON document."""
    return {
        "format": "openkeito-model",
        "version": model_version(components),
        "name": name,
        "description": description,
        "simulation": {
            "time_step_s": time_step_s,
            "end_time_s": end_time_s,
            "method": method,
            "initialization": "steady_state",
            "output_interval_steps": output_interval_steps,
        },
        "components": components,
        "outputs": outputs,
    }


def source_component(
    name: str,
    bus: str,
    v_phase_v: complex,
    s_mva: complex,
    z_ohm: complex,
    f_hz: float,
    *,
    z0_ohm: complex | None = None,
    neutral: str | None = None,
) -> dict[str, Any]:
    """A three_phase_source behind z_ohm delivering s_mva at phase-to-neutral
    voltage v_phase_v (rms, V)."""
    w = 2 * math.pi * f_hz
    i = (s_mva * 1e6 / (3 * v_phase_v)).conjugate()
    e = v_phase_v + z_ohm * i
    z0 = z_ohm if z0_ohm is None else z0_ohm
    out: dict[str, Any] = {
        "type": "three_phase_source",
        "name": name,
        "bus": bus,
        "line_voltage_rms_v": abs(e) * math.sqrt(3),
        "frequency_hz": f_hz,
        "angle_deg": math.degrees(cmath.phase(e)),
        "impedance": {
            "type": "sequence",
            "r1_ohm": z_ohm.real,
            "l1_h": z_ohm.imag / w,
            "r0_ohm": z0.real,
            "l0_h": z0.imag / w,
        },
    }
    if neutral is not None:
        out["neutral"] = neutral
    return out


def consumption_components(
    cname: str,
    node: str,
    v_ll_kv: float,
    v_phase_v: complex,
    s_mva: complex,
    f_hz: float,
    assumed: dict[str, list[str]],
    element: str,
    assumption_key: str,
    *,
    neutral: str | None = None,
) -> list[dict[str, Any]]:
    """Components taking s_mva at voltage v_ll_kv / v_phase_v: a constant
    impedance when active power >= 0, or reactive impedance (P = 0) plus a
    current injection for negative active power."""
    if s_mva == 0:
        return []
    if s_mva.real >= 0:
        out: dict[str, Any] = {
            "type": "three_phase_load",
            "name": cname,
            "bus": node,
            "rated_voltage_kv": v_ll_kv,
            "rated_frequency_hz": f_hz,
            "active_power_mw": s_mva.real,
            "reactive_power_mvar": s_mva.imag,
        }
        if neutral is not None:
            out["neutral"] = neutral
        return [out]
    comps: list[dict[str, Any]] = []
    if s_mva.imag != 0:
        load_comp: dict[str, Any] = {
            "type": "three_phase_load",
            "name": cname,
            "bus": node,
            "rated_voltage_kv": v_ll_kv,
            "rated_frequency_hz": f_hz,
            "active_power_mw": 0.0,
            "reactive_power_mvar": s_mva.imag,
        }
        if neutral is not None:
            load_comp["neutral"] = neutral
        comps.append(load_comp)
    i = (-complex(s_mva.real, 0.0) * 1e6 / (3 * v_phase_v)).conjugate()
    assumed[assumption_key].append(element)
    comps.append({
        "type": "three_phase_current_source",
        "name": f"{cname}_p" if s_mva.imag != 0 else cname,
        "bus": node,
        "current_rms_a": abs(i),
        "frequency_hz": f_hz,
        "angle_deg": math.degrees(cmath.phase(i)),
    })
    return comps


def check_inverter_model(inverter_model: str, converter_parameters: dict[str, Any] | None) -> None:
    if inverter_model not in INVERTER_MODELS:
        raise ValueError(f"inverter_model is {inverter_model!r}; use one of {', '.join(INVERTER_MODELS)}")
    fixed = {"type", "name", "bus", "rated_power_mva", "rated_voltage_kv", "rated_frequency_hz", "reference",
             "operating_point"} & set(converter_parameters or {})
    if fixed:
        raise ValueError(f"converter_parameters cannot set {', '.join(sorted(fixed))}: the converter takes them "
                         "from the network and its power flow")


def grid_following_converter(
    name: str,
    bus: str,
    s_injected_mva: complex,
    v_ll_kv: complex,
    f_hz: float,
    rated_mva: float | None,
    rated_kv: float,
    parameters: dict[str, Any] | None,
    assumed: dict[str, list[str]],
    element: str,
) -> dict[str, Any]:
    """The averaged converter that injects ``s_injected_mva`` at a bus with
    the line-to-line voltage phasor ``v_ll_kv`` (kV): its references are the
    powers of the power flow, and its operating point their bus voltage, so
    the run starts in the steady state of the power flow.

    rated_mva: the rating of the element, or None when the network gives
    none: the injected apparent power over 0.9 is then assumed (the converter
    runs at 0.9 of its rating). parameters: more fields of the component
    (``openkeito components``), such as {"current_limit_pu": 1.2}.
    assumed: receives what was assumed, by kind, for ``element``.
    """
    s = abs(s_injected_mva)
    if rated_mva is None or not rated_mva > 0:
        rated_mva = s / 0.9
        assumed["no rated power (sn_mva, max_p); the converter is rated at its apparent power of the power "
                "flow / 0.9"].append(element)
    component = {
        "type": "grid_following_converter", "name": name, "bus": bus,
        "rated_power_mva": rated_mva, "rated_voltage_kv": rated_kv, "rated_frequency_hz": f_hz,
        "reference": {"type": "power", "active_power_mw": s_injected_mva.real,
                      "reactive_power_mvar": s_injected_mva.imag},
        "operating_point": {"active_power_mw": s_injected_mva.real, "reactive_power_mvar": s_injected_mva.imag,
                            "voltage_kv": abs(v_ll_kv), "angle_deg": math.degrees(cmath.phase(v_ll_kv))},
    }
    component.update(parameters or {})
    # Above its current limit the converter cannot hold the power flow.
    current_pu = s / rated_mva / (abs(v_ll_kv) / rated_kv)
    limit = float(component.get("current_limit_pu", 1.1))
    if current_pu > limit:
        assumed[f"the current of the power flow is above the current limit of the converter ({limit:g} pu of its "
                "rating): it starts at the limit and does not hold the power flow"].append(
            f"{element} ({current_pu:.3g} pu)")
    return component


# Time slices -----------------------------------------------------------------


def fault(
    bus: Any,
    start_time_s: float,
    clear_time_s: float | None = None,
    *,
    phases: str = "abc",
    ground: bool = True,
    fault_resistance_ohm: float = 0.0,
    name: str | None = None,
) -> dict[str, Any]:
    """A disturbance for the time slices: a fault at ``bus`` (a pandapower
    bus index, an IIDM bus id or a model bus name; default: three-phase to
    ground), cleared at clear_time_s.

    name: name of the disturbance and of its component (default:
    ``fault_<bus>_<phases>``). The disturbances of a set of time slices need
    different names: give one to each of several faults at the same bus."""
    import re

    if isinstance(bus, numbers.Integral):
        bus = int(bus)
    where = f"b{bus}" if isinstance(bus, int) else re.sub(r"[^A-Za-z0-9_.-]", "_", str(bus))
    label = name or f"fault_{where}_{phases}"
    component = {
        "type": "fault", "name": label, "bus": bus, "phases": list(phases), "ground": ground,
        "fault_resistance_ohm": fault_resistance_ohm, "start_time_s": start_time_s,
    }
    if clear_time_s is not None:
        component["clear_time_s"] = clear_time_s
    return {"name": label, "components": [component]}


def place(conversion: Conversion, component: dict[str, Any]) -> dict[str, Any]:
    """A disturbance component with its "bus" (or the entries of "buses")
    given as a bus of the source network replaced by the model's bus name;
    other names (model buses and nodes) are kept. A bus index that the
    conversion does not have raises ConversionError."""
    buses = conversion.mapping["buses"]

    def name(b: Any) -> Any:
        if str(b) not in buses and isinstance(b, numbers.Integral):
            known = ", ".join(list(buses)[:10]) + (", ..." if len(buses) > 10 else "")
            raise ConversionError([f"component {component.get('name', '')!r}: the network has no bus {b} in "
                                   f"service with a power-flow voltage (buses: {known})"])
        return buses.get(str(b), b)

    c = dict(component)
    if "bus" in c:
        c["bus"] = name(c["bus"])
    if isinstance(c.get("buses"), list):
        c["buses"] = [name(b) for b in c["buses"]]
    return c


@dataclass
class TimeSlice:
    """One case of a set of time slices: the network at ``time`` with a
    disturbance (or none)."""

    name: str
    time: str
    disturbance: str | None
    conversion: Conversion

    @property
    def model(self) -> dict[str, Any]:
        return self.conversion.model


def make_time_slices(
    times: list[Any],
    convert_at: Callable[[Any], Conversion],
    disturbances: list[dict[str, Any]] | None = None,
) -> list[TimeSlice]:
    """For every time, the conversion ``convert_at(time)``; for every
    disturbance (default: none), a copy of its model with the disturbance's
    components and outputs added (see ``place``).

    The case of time k and disturbance d is named ``t<k>.<name of d>`` (a
    disturbance without a name: ``d<j>``, j its position), so the
    disturbances need different names: equal ones raise ConversionError, as
    does a time whose conversion fails (with the time in front of each
    problem)."""
    import json

    from .schedule import format_time

    labels = [d.get("name") or f"d{j}" for j, d in enumerate(disturbances or [])]
    repeated = sorted({label for label in labels if labels.count(label) > 1})
    if repeated:
        raise ConversionError([
            f"{labels.count(label)} disturbances are named {label!r}, so their cases would have the same name; "
            "give each a name of its own (fault(..., name=...))" for label in repeated])
    slices = []
    for k, time in enumerate(times):
        try:
            conv = convert_at(time)
        except ConversionError as e:
            raise ConversionError([f"{format_time(time)}: {p}" for p in e.problems]) from e
        for d, label in zip(disturbances or [None], labels or [None]):
            model = json.loads(json.dumps(conv.model))
            if d is not None:
                model["components"] += [place(conv, c) for c in d.get("components", [])]
                model["outputs"] += list(d.get("outputs", []))
            case = Conversion(model=model, mapping=conv.mapping, warnings=conv.warnings,
                              power_flow=conv.power_flow)
            slices.append(TimeSlice(name=f"t{k:03d}" + (f".{label}" if label else ""),
                                    time=format_time(time), disturbance=label, conversion=case))
    return slices


def schedule_slices(
    network: Any,
    schedule: Any,
    disturbances: list[dict[str, Any]] | None,
    times: list[Any] | None,
    *,
    check: Callable[[Any, Any], None],
    apply: Callable[[Any, Any, Any], Any],
    convert: Callable[..., Conversion],
    convert_options: dict[str, Any],
) -> list[TimeSlice]:
    """The time slices of a converter (its ``time_slices``): ``check`` the
    schedule against the network; then, for every time (default: every time
    of the schedule), ``convert`` the network that ``apply`` makes of the
    schedule's values, and add the disturbances (see ``make_time_slices``)."""
    check(network, schedule)
    return make_time_slices(
        list(times if times is not None else schedule.times),
        lambda time: convert(apply(network, schedule, time), **convert_options),
        disturbances,
    )


@dataclass
class TimeSliceRun:
    """Time slices run together: the cases, the batch result and one row
    per case."""

    slices: list[TimeSlice]
    batch: Any
    rows: list[dict[str, Any]]

    def to_pandas(self) -> Any:
        import pandas as pd

        return pd.DataFrame(self.rows)


def run_slices(
    slices: list[TimeSlice],
    *,
    jobs: int | None = None,
    out: Any = None,
    force: bool = False,
    keep_rows: bool = False,
) -> TimeSliceRun:
    """Runs time slices in parallel with ``openkeito.run_batch``. Each row
    gives the case, time, disturbance, status, the largest difference of the
    steady state from the power flow (bus voltage magnitude in pu, angle in
    degrees), the largest relative departure of the waveforms from the
    steady state before the first switching action, and the number of
    warnings."""
    import openkeito

    batch = openkeito.run_batch([(s.name, s.model) for s in slices], out, jobs=jobs, force=force,
                                keep_rows=keep_rows)
    rows = []
    for s in slices:
        result = batch[s.name]
        row: dict[str, Any] = {"case": s.name, "time": s.time, "disturbance": s.disturbance,
                               "status": result.status}
        init = result.summary.get("initialization") or {}
        if init.get("phasors"):
            c = s.conversion.compare(result)
            row["max_abs_dv_pu"] = c["max_abs_dv_pu"]
            row["max_abs_dva_degree"] = c["max_abs_dva_degree"]
            row["relative_voltage_deviation"] = init.get("relative_voltage_deviation")
        row["warnings"] = len(result.warnings)
        rows.append(row)
    return TimeSliceRun(slices=slices, batch=batch, rows=rows)


# Generators as synchronous machines -------------------------------------------

#: Parameters of a synchronous machine that the network data does not hold:
#: a round-rotor (thermal) unit with values inside the ranges of Kundur,
#: "Power System Stability and Control" (1994), table 4.2 (reactances in per
#: unit of the rating, open-circuit time constants in seconds) and of its
#: table 3.2 for the inertia constant H of thermal units (2.5 to 6 s for
#: 3600 r/min, 4 to 10 s for 1800 r/min).
TYPICAL_MACHINE: dict[str, float] = {
    "xd_pu": 1.8, "xq_pu": 1.7, "xd_transient_pu": 0.3, "xq_transient_pu": 0.55,
    "xd_subtransient_pu": 0.25, "xq_subtransient_pu": 0.25, "xl_pu": 0.15, "ra_pu": 0.003,
    "td0_transient_s": 8.0, "td0_subtransient_s": 0.03, "tq0_transient_s": 1.0, "tq0_subtransient_s": 0.07,
    "inertia_h_s": 4.0, "damping_pu": 0.0,
}

#: Rating assumed for a generator without one: its power-flow output (or its
#: largest active power, when larger) is this fraction of the rating.
MACHINE_LOADING = 0.85
#: A largest active power above this many times the output is taken for a
#: placeholder (9999 MW in the IEEE test cases) and not for a size.
MACHINE_HEADROOM = 10.0

GENERATOR_MODELS = ("source", "machine")


def check_generators(generators: str) -> None:
    if generators not in GENERATOR_MODELS:
        raise ValueError(f"generators is {generators!r}; use one of {', '.join(map(repr, GENERATOR_MODELS))}")


def machine_rating(rated_mva: Any, s_mva: complex, p_max_mw: Any = None) -> tuple[float, bool]:
    """(rating in MVA, whether it was assumed): the rating of the data, or
    the larger of the power-flow output and the largest active power (when
    it is within MACHINE_HEADROOM of the output) over MACHINE_LOADING, and
    at least 1 MVA."""
    if finite(rated_mva) and float(rated_mva) > 0:
        return float(rated_mva), False
    largest = abs(s_mva)
    if finite(p_max_mw) and 0 < float(p_max_mw) <= MACHINE_HEADROOM * max(largest, 1.0):
        largest = max(largest, float(p_max_mw))
    return max(largest, 1.0) / MACHINE_LOADING, True


def machine_rating_for(points: list[tuple[complex, float]], xd_pu: float | None = None) -> float:
    """The rating (MVA) for a machine without one that is to run at every
    one of ``points``, each (S in MVA delivered, terminal voltage in pu):

    - its largest apparent power is MACHINE_LOADING of the rating, as
      ``machine_rating`` assumes for one point, and
    - with ``xd_pu`` (the synchronous reactance in pu of the rating), the
      reactive power it absorbs is at most MACHINE_LOADING of V^2 / Xd of
      the rating. Without active power the field voltage of the steady
      state is V - Xd (-Q) / V in pu: at -Q = V^2 / Xd it is zero, and
      beyond it negative, where no machine runs (the under-excited side of
      the capability curve ends before that, at the limit of stability and
      of the heating of the stator end region: Kundur, "Power System
      Stability and Control" (1994), chapter 5). A generator of a
      power-flow case that only absorbs reactive power (a synchronous
      condenser) and is rated by its apparent power alone would be at
      -Q = 0.85 pu, beyond V^2 / Xd = 0.56 pu of the typical machine.
    """
    need = 0.0
    for s_mva, voltage_pu in points:
        need = max(need, abs(s_mva) / MACHINE_LOADING)
        if xd_pu is not None and s_mva.imag < 0 and voltage_pu > 0:
            need = max(need, -s_mva.imag * xd_pu / (MACHINE_LOADING * voltage_pu ** 2))
    return need


def _ordered(values: dict[str, Any], given: set[str]) -> bool:
    """Moves the typical reactances so that the order xd > xd(transient) >
    xd(subtransient) > xl (and the same on the q axis) holds around the ones
    that are given; True when one was moved. The given ones are never
    changed: if they contradict each other, validation says so (E280)."""
    moved = False
    for axis in ("d", "q"):
        x, x1, x2 = f"x{axis}_pu", f"x{axis}_transient_pu", f"x{axis}_subtransient_pu"
        has_transient = x1 in values
        if "xl_pu" not in given and values["xl_pu"] >= values[x2]:
            values["xl_pu"], moved = 0.6 * values[x2], True
        if x2 not in given and values[x2] <= values["xl_pu"]:
            values[x2], moved = values["xl_pu"] / 0.6, True
        if has_transient and x1 not in given and values[x1] <= values[x2]:
            values[x1], moved = 1.25 * values[x2], True
        below = values[x1] if has_transient else values[x2]
        if x not in given and values[x] <= below:
            values[x], moved = 3.0 * below, True
    return moved


def synchronous_machine(
    name: str,
    bus: str,
    *,
    frequency_hz: float,
    rated_power_mva: float,
    rated_voltage_kv: float,
    s_mva: complex,
    voltage_pu: float,
    angle_deg: float,
    data: dict[str, float] | None = None,
    parameters: dict[str, Any] | None = None,
) -> tuple[dict[str, Any], list[str]]:
    """A synchronous_machine component that delivers ``s_mva`` (MW + j Mvar)
    at the power-flow voltage of its bus, and notes on what was assumed.

    data: parameters found in the network data (model field names).
    parameters: parameters given by the caller, which win over both. Any
    field of the component can be given, also the signals of an excitation
    system and a governor; "xq_transient_pu": None makes a machine with one
    damper winding on the q axis.
    """
    known = {k: v for k, v in (data or {}).items() if finite(v)}
    chosen = dict(parameters or {})
    values: dict[str, Any] = {**TYPICAL_MACHINE, **known, **chosen}
    if "xq_transient_pu" in chosen and chosen["xq_transient_pu"] is None:
        # One damper winding on the q axis.
        values.pop("xq_transient_pu")
        values.pop("tq0_transient_s", None)
    given = set(known) | set(chosen)
    moved = _ordered(values, given)
    typical = sorted(k for k in TYPICAL_MACHINE if k in values and k not in given)
    component = {
        "type": "synchronous_machine", "name": name, "bus": bus,
        "rated_power_mva": rated_power_mva, "rated_voltage_kv": rated_voltage_kv, "frequency_hz": frequency_hz,
        **values,
        "operating_point": {"active_power_mw": s_mva.real, "reactive_power_mvar": s_mva.imag,
                            "voltage_pu": voltage_pu, "angle_deg": angle_deg},
    }
    notes = []
    if typical:
        notes.append("typical machine parameters (openkeito.convert.TYPICAL_MACHINE: Kundur, tables 3.2 and "
                     "4.2) for " + ", ".join(typical))
    if moved:
        notes.append("typical reactances moved to keep the order xd > xd' > xd'' > xl around the given ones")
    return component, notes


def split_machine_parameters(machine_parameters: dict[str, Any] | None) -> tuple[dict[str, Any], dict[str, dict]]:
    """(parameters of every machine, parameters per element): the entries of
    ``machine_parameters`` whose value is a dict belong to the element of
    that key."""
    common: dict[str, Any] = {}
    per_element: dict[str, dict] = {}
    for key, value in (machine_parameters or {}).items():
        if isinstance(value, dict):
            per_element[str(key)] = value
        else:
            common[key] = value
    return common, per_element
