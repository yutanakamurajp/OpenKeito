"""Grid-connection tests: a grid equivalent from a short-circuit ratio and an
X/R ratio, disturbances of its voltage, and a device under test attached to it.

    from openkeito.gridtest import GridEquivalent, PhaseJump, attach
    grid = GridEquivalent(rated_power_mva=2.0, rated_voltage_kv=6.6, scr=3.0, x_over_r=10.0)
    test = attach("device.json", "poi", grid, [PhaseJump(0.2, 30.0)], end_time_s=1.0)
    result = openkeito.run(test.model)

The equivalent is a voltage source behind the impedance Z with
|Z| = V^2 / (SCR * S), R = |Z| / sqrt(1 + (X/R)^2) and X = (X/R) * R, as a
`coupled_rl` from the bus of the source to the point of connection. Its
internal voltage is the one that puts the point of connection at the voltage
of the operating point while the device delivers its power there: the run
starts in that steady state (``"initialization": "steady_state"``).

A disturbance turns the source into a `three_phase_controlled_source` whose
phase voltages are written by control blocks: the magnitude (voltage
steps), the angle (phase jumps) and the frequency (steps and ramps) of the
internal voltage. Such a source follows its signals one time step late
(docs/model.md, "Controlled sources"); the blocks are therefore written one
step ahead, so that in the steady state the source is the sine of the
equivalent at every grid point. A step of the magnitude or of the angle at
``time_s`` takes effect over the step that starts at the first grid point at
or after ``time_s``. A fault (``Fault``) is a `fault` component at the point
of connection (docs/python.md, "Grid-connection tests").

``openkeito.requirements`` runs a requirement file of such tests as one batch
and judges the cases; ``openkeito.metrics`` gives the response metrics.
"""

from __future__ import annotations

import cmath
import copy
import json
import math
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any, Union

import openkeito

__all__ = [
    "Fault",
    "FrequencyRamp",
    "FrequencyStep",
    "GridEquivalent",
    "GridTest",
    "PhaseJump",
    "VoltageStep",
    "attach",
    "fault_resistance_for",
]

PHASES = ("a", "b", "c")

Disturbance = Union["VoltageStep", "PhaseJump", "FrequencyStep", "FrequencyRamp", "Fault"]


def _positive(name: str, value: float, *, infinite: bool = False) -> float:
    value = float(value)
    if math.isnan(value) or value <= 0.0 or (math.isinf(value) and not infinite):
        raise ValueError(f"{name} must be a positive number, not {value!r}")
    return value


@dataclass(frozen=True)
class GridEquivalent:
    """A Thevenin equivalent of the grid at the point of connection.

    ``scr`` is the short-circuit ratio: the short-circuit power at the point
    of connection over ``rated_power_mva``, the rating of the device. The
    impedance has the angle of ``x_over_r`` (``math.inf``: a reactance only).
    ``z0_over_z1`` is the ratio of the zero-sequence to the positive-sequence
    impedance (default 1: the same impedance in every sequence, so that the
    phases are not coupled)."""

    rated_power_mva: float
    rated_voltage_kv: float
    scr: float
    x_over_r: float
    frequency_hz: float = 50.0
    z0_over_z1: float = 1.0

    def __post_init__(self) -> None:
        _positive("rated_power_mva", self.rated_power_mva)
        _positive("rated_voltage_kv", self.rated_voltage_kv)
        _positive("scr", self.scr)
        _positive("x_over_r", self.x_over_r, infinite=True)
        _positive("frequency_hz", self.frequency_hz)
        _positive("z0_over_z1", self.z0_over_z1)

    @property
    def short_circuit_mva(self) -> float:
        """SCR times the rating."""
        return self.scr * self.rated_power_mva

    @property
    def impedance_ohm(self) -> complex:
        """R + jX of the positive sequence, in ohm."""
        z = self.rated_voltage_kv**2 / self.short_circuit_mva
        k = self.x_over_r
        r = z / math.sqrt(1.0 + k * k) if math.isfinite(k) else 0.0
        x = z / math.sqrt(1.0 + 1.0 / (k * k))
        return complex(r, x)

    @property
    def omega(self) -> float:
        return 2.0 * math.pi * self.frequency_hz

    def impedance(self) -> dict[str, Any]:
        """The impedance as the ``impedance`` field of a `coupled_rl`."""
        z = self.impedance_ohm
        return {"type": "sequence", "r1_ohm": z.real, "l1_h": z.imag / self.omega,
                "r0_ohm": self.z0_over_z1 * z.real, "l0_h": self.z0_over_z1 * z.imag / self.omega}

    def base_voltage_v(self) -> float:
        """Peak phase voltage of the rated voltage: the base of per unit."""
        return math.sqrt(2.0 / 3.0) * 1e3 * self.rated_voltage_kv


@dataclass(frozen=True)
class VoltageStep:
    """The magnitude of the internal voltage changes by ``change_pu`` of its
    value (-0.1: to 90 %) at ``time_s``, and back after ``duration_s`` when
    that is given."""

    time_s: float
    change_pu: float
    duration_s: float | None = None
    kind = "voltage_step"


@dataclass(frozen=True)
class PhaseJump:
    """The angle of the internal voltage jumps by ``angle_deg`` (positive:
    ahead) at ``time_s``, and back after ``duration_s`` when that is given."""

    time_s: float
    angle_deg: float
    duration_s: float | None = None
    kind = "phase_jump"


@dataclass(frozen=True)
class FrequencyStep:
    """The frequency of the internal voltage changes by ``change_hz`` at
    ``time_s`` (its angle then moves as 2*pi*change_hz*(t - time_s)), and back
    after ``duration_s`` when that is given."""

    time_s: float
    change_hz: float
    duration_s: float | None = None
    kind = "frequency_step"


@dataclass(frozen=True)
class FrequencyRamp:
    """The frequency changes at ``rate_hz_per_s`` from ``time_s`` on, for
    ``duration_s`` (then it stays; None: until the end of the run). The angle
    is the integral of the frequency: pi*rate*(t - time_s)^2 during the ramp."""

    time_s: float
    rate_hz_per_s: float
    duration_s: float | None = None
    kind = "frequency_ramp"


@dataclass(frozen=True)
class Fault:
    """A fault at the point of connection (``bus``: another bus) from
    ``time_s`` for ``duration_s`` (each phase opens at its next current
    zero). Its resistance is ``resistance_ohm``, or the one that leaves
    ``residual_voltage_pu`` of the voltage at the point of connection with
    the grid equivalent alone (``fault_resistance_for``); 0 when neither is
    given."""

    time_s: float
    duration_s: float
    phases: tuple[str, ...] = ("a", "b", "c")
    ground: bool = True
    resistance_ohm: float | None = None
    residual_voltage_pu: float | None = None
    bus: str | None = None
    kind = "fault"


def fault_resistance_for(grid: GridEquivalent, residual_voltage_pu: float) -> float:
    """The resistance R of a three-phase fault at the point of connection
    that leaves ``residual_voltage_pu`` of the voltage behind the grid
    impedance Z: |R / (Z + R)| = v, the positive root of
    (1 - v^2) R^2 - 2 v^2 Re(Z) R - v^2 |Z|^2 = 0."""
    v = float(residual_voltage_pu)
    if not 0.0 <= v < 1.0:
        raise ValueError(f"residual_voltage_pu must be at least 0 and below 1, not {v!r}")
    z = grid.impedance_ohm
    a, b, c = 1.0 - v * v, -2.0 * v * v * z.real, -v * v * abs(z) ** 2
    return (-b + math.sqrt(b * b - 4.0 * a * c)) / (2.0 * a)


def _load(model: Any) -> dict[str, Any]:
    return json.loads(openkeito._model_text(model))


def _version(version: Any) -> tuple[int, ...]:
    """The rank of a format version "0.5" (a version that is not one ranks
    lowest)."""
    try:
        return tuple(int(x) for x in str(version).split("."))
    except ValueError:
        return ()


def _names(model: Mapping[str, Any]) -> set[str]:
    names = {c.get("name") for c in model.get("components", [])}
    names |= {c.get("name") for c in model.get("controls", [])}
    for c in model.get("components", []):
        for key in ("bus", "buses"):
            value = c.get(key)
            for b in ([value] if isinstance(value, str) else value or []):
                if isinstance(b, str):
                    names.add(b)
    return {n for n in names if isinstance(n, str)}


def _phasor(xa: float, xb: float, xc: float) -> complex:
    """The rms phasor (cosine reference) of balanced phases at t = 0."""
    a = cmath.exp(2j * math.pi / 3.0)
    return (2.0 / 3.0) * (xa + a * xb + a * a * xc) / math.sqrt(2.0)


def _operating_voltage(model: Mapping[str, Any], bus: str, grid: GridEquivalent) -> tuple[float, float]:
    for c in model.get("components", []):
        if c.get("type") == "grid_following_converter" and c.get("bus") == bus and "operating_point" in c:
            op = c["operating_point"]
            return float(op["voltage_kv"]), float(op.get("angle_deg", 0.0))
    return grid.rated_voltage_kv, 0.0


def _probe(model: dict[str, Any], bus: str, voltage_kv: float, angle_deg: float,
           frequency_hz: float, name: str) -> complex:
    """The phase current (rms phasor, from the bus into the grid) that the
    device delivers at its bus held at the voltage: a one-step run of the
    device with an ideal source at the bus, at t = 0 (the phasor solution)."""
    probe = copy.deepcopy(model)
    sim = probe["simulation"]
    sim["initialization"] = "steady_state"
    sim["end_time_s"] = sim["time_step_s"]
    sim.pop("output_interval_steps", None)
    probe["components"].append({"type": "three_phase_source", "name": name, "bus": bus,
                                "line_voltage_rms_v": 1e3 * voltage_kv, "frequency_hz": frequency_hz,
                                "angle_deg": angle_deg})
    probe["outputs"] = [{"type": "branch_current", "component": name}]
    probe.pop("frequency_scan", None)
    result = openkeito.run(probe)
    return _phasor(*(float(result[f"I({name}.{p})"][0]) for p in PHASES))


@dataclass
class GridTest:
    """A device attached to a grid equivalent: ``model`` is ready to run.

    ``voltage`` is the phasor of the line voltage (rms, V) at the point of
    connection in the steady state, ``injection_mva`` the power P + jQ (MW,
    Mvar) that the device delivers there, ``source_voltage`` the internal
    voltage of the equivalent (line, rms, V) and ``signals`` the control
    signals of the disturbances (magnitude ``m``, angle ``phi``, the phase
    voltages ``a``, ``b``, ``c``)."""

    model: dict[str, Any]
    grid: GridEquivalent
    bus: str
    disturbances: list[Any]
    voltage: complex
    injection_mva: complex
    source_voltage: complex
    name: str
    signals: dict[str, str] = field(default_factory=dict)
    notes: list[str] = field(default_factory=list)

    @property
    def voltage_pu(self) -> float:
        return abs(self.voltage) / (1e3 * self.grid.rated_voltage_kv)

    def disturbance_times(self) -> list[tuple[float, float]]:
        """(start, end) of every disturbance; end is the start for one that
        stays."""
        out = []
        for d in self.disturbances:
            end = d.time_s + (d.duration_s or 0.0)
            out.append((d.time_s, end))
        return out


def attach(
    model: Any,
    bus: str,
    grid: GridEquivalent,
    disturbances: Iterable[Disturbance] = (),
    *,
    voltage_kv: float | None = None,
    angle_deg: float | None = None,
    injection_mva: complex | None = None,
    end_time_s: float | None = None,
    time_step_s: float | None = None,
    name: str = "grid",
) -> GridTest:
    """Attaches the grid equivalent at ``bus`` of the device ``model`` (a
    dict, JSON text or a path) and adds the disturbances.

    The steady state is the device at its operating point: ``bus`` at
    ``voltage_kv`` (line, rms) and ``angle_deg``. By default these are the
    operating point of a `grid_following_converter` at ``bus``, or the rated
    voltage of the grid and 0. ``injection_mva`` (P + jQ delivered by the
    device at ``bus``, MW and Mvar) is by default found by solving the
    device with ``bus`` held at that voltage (one step of a run from the
    steady state; the device is taken as balanced). The internal voltage of
    the equivalent is then E = V - Z I, I = conj(S / (3 V)) the current
    into the grid. ``end_time_s`` and ``time_step_s`` replace those of the
    model. The components and blocks added are named ``name``, ``<name>_z``,
    ``<name>_<...>``; a name that the model already has is refused. What is
    added needs format 0.3 (the source, ``coupled_rl``, ``fault`` and the
    steady-state initialization) and, with a disturbance of the source,
    0.5 (``three_phase_controlled_source`` and its control blocks); a device
    of an older ``version`` is refused, its version is not changed."""
    device = _load(model)
    if not isinstance(device.get("components"), list):
        raise ValueError("the device model has no list of components")
    disturbances = list(disturbances)
    needed = "0.5" if any(d.kind != "fault" for d in disturbances) else "0.3"
    version = device.get("version")
    if _version(version) < _version(needed):
        what = ("the three_phase_controlled_source and the control blocks of the disturbances" if needed == "0.5"
                else "the three_phase_source, the coupled_rl, the faults and the steady-state initialization")
        raise ValueError(f"the device model has version {version!r}, and {what} need format {needed}; "
                         f'set "version": "{needed}" or later in the device model (attach does not change it)')
    device.setdefault("controls", [])
    device.setdefault("outputs", [])
    sim = device.setdefault("simulation", {})
    if time_step_s is not None:
        sim["time_step_s"] = _positive("time_step_s", time_step_s)
    if end_time_s is not None:
        sim["end_time_s"] = _positive("end_time_s", end_time_s)
    if "time_step_s" not in sim or "end_time_s" not in sim:
        raise ValueError("the simulation needs time_step_s and end_time_s")
    h = float(sim["time_step_s"])
    taken = _names(device)
    if device["components"] and bus not in taken:
        raise ValueError(f"the device model has no bus {bus!r}")
    src_bus = f"{name}_src"
    ours = {name, f"{name}_z", src_bus}
    clash = sorted(ours & taken)
    if clash:
        raise ValueError(f"the device model already uses the name(s) {', '.join(clash)}; pass another name=")

    v_kv, ang = _operating_voltage(device, bus, grid)
    v_kv = v_kv if voltage_kv is None else float(voltage_kv)
    ang = ang if angle_deg is None else float(angle_deg)
    voltage = cmath.rect(1e3 * v_kv, math.radians(ang))
    if injection_mva is None and not device["components"]:
        injection_mva = 0j
    if injection_mva is None:
        current = _probe(device, bus, v_kv, ang, grid.frequency_hz, f"{name}_probe")
        s = 3.0 * (voltage / math.sqrt(3.0)) * current.conjugate() / 1e6
    else:
        s = complex(injection_mva)
        current = (s * 1e6 / (3.0 * voltage / math.sqrt(3.0))).conjugate()
    z = grid.impedance_ohm
    e_phase = voltage / math.sqrt(3.0) - z * current
    source = e_phase * math.sqrt(3.0)
    notes: list[str] = []
    if sim.get("initialization") != "steady_state":
        notes.append("simulation.initialization set to steady_state")
    sim["initialization"] = "steady_state"

    wave = [d for d in disturbances if d.kind != "fault"]
    faults = [d for d in disturbances if d.kind == "fault"]
    components = device["components"]
    controls = device["controls"]
    signals: dict[str, str] = {}
    e_ll, e_deg = abs(source), math.degrees(cmath.phase(source))
    if wave:
        signals = _source_blocks(controls, name, grid, h, e_ll, math.radians(e_deg), wave, taken)
        components.append({"type": "three_phase_controlled_source", "name": name, "bus": src_bus,
                           "signals": [signals[p] for p in PHASES],
                           "steady_state": {"line_voltage_rms_v": e_ll, "angle_deg": e_deg}})
    else:
        components.append({"type": "three_phase_source", "name": name, "bus": src_bus,
                           "line_voltage_rms_v": e_ll, "frequency_hz": grid.frequency_hz, "angle_deg": e_deg})
    components.append({"type": "coupled_rl", "name": f"{name}_z", "buses": [src_bus, bus],
                       "impedance": grid.impedance()})
    for k, f in enumerate(faults):
        fname = f"{name}_fault{k + 1}" if len(faults) > 1 else f"{name}_fault"
        if fname in taken:
            raise ValueError(f"the device model already uses the name {fname}; pass another name=")
        if f.resistance_ohm is not None and f.residual_voltage_pu is not None:
            raise ValueError("give a fault resistance_ohm or residual_voltage_pu, not both")
        r = (f.resistance_ohm if f.resistance_ohm is not None else
             fault_resistance_for(grid, f.residual_voltage_pu) if f.residual_voltage_pu is not None else 0.0)
        components.append({"type": "fault", "name": fname, "bus": f.bus or bus, "phases": list(f.phases),
                           "ground": bool(f.ground), "fault_resistance_ohm": r,
                           "start_time_s": f.time_s, "clear_time_s": f.time_s + f.duration_s})
    if not any(o.get("type") == "bus_voltage" and o.get("bus") == bus for o in device["outputs"]):
        device["outputs"].append({"type": "bus_voltage", "bus": bus})
    return GridTest(model=device, grid=grid, bus=bus, disturbances=disturbances, voltage=voltage,
                    injection_mva=s, source_voltage=source, name=name, signals=signals, notes=notes)


def _source_blocks(controls: list[dict[str, Any]], name: str, grid: GridEquivalent, h: float,
                   e_ll: float, e_rad: float, wave: Sequence[Any], taken: set[str]) -> dict[str, str]:
    """Control blocks that write the phase voltages of the source, one time
    step ahead (the source follows its signals one step late): the d value
    is the peak phase voltage times the magnitude, the angle
    omega*(t + h) + angle of the internal voltage + jumps + the integral of
    the change of frequency. Blocks of format 0.5 only (step, ramp, limiter,
    product, gain, sum, constant, dq0_to_abc, with the
    three_phase_controlled_source), so that a device of format 0.5 or later
    keeps its version (``attach`` refuses an older one)."""
    blocks: list[dict[str, Any]] = []
    w0 = grid.omega
    kd = math.sqrt(2.0 / 3.0) * e_ll
    theta = f"{name}_theta"
    blocks.append({"type": "ramp", "name": theta, "start_time_s": 0.0, "slope_per_s": w0,
                   "initial": w0 * h + e_rad})
    magnitude: list[str] = []
    angle: list[str] = [theta]
    count = 0

    def new(prefix: str) -> str:
        nonlocal count
        count += 1
        return f"{name}_{prefix}{count}"

    def step(prefix: str, value: float, time_s: float, duration_s: float | None) -> list[str]:
        sname = new(prefix)
        blocks.append({"type": "step", "name": sname, "time_s": float(time_s), "before": 0.0,
                       "after": float(value)})
        if duration_s is None:
            return [sname]
        back = f"{sname}_back"
        blocks.append({"type": "step", "name": back, "time_s": float(time_s) + float(duration_s),
                       "before": 0.0, "after": -float(value)})
        return [sname, back]

    def ramp(prefix: str, start_s: float, slope: float) -> str:
        # Written one step ahead: the value at t is slope * max(t + h - start_s, 0).
        rname = new(prefix)
        blocks.append({"type": "ramp", "name": rname, "start_time_s": start_s - h, "slope_per_s": slope,
                       "initial": 0.0})
        return rname

    for d in wave:
        if not math.isfinite(d.time_s) or d.time_s < h or (
                d.duration_s is not None and not (math.isfinite(d.duration_s) and d.duration_s > 0)):
            raise ValueError(f"{d}: time_s must be at least one time step and duration_s positive")
        if d.kind == "voltage_step":
            magnitude += step("dv", kd * float(d.change_pu), d.time_s, d.duration_s)
        elif d.kind == "phase_jump":
            angle += step("jump", math.radians(d.angle_deg), d.time_s, d.duration_s)
        elif d.kind == "frequency_step":
            k = 2.0 * math.pi * float(d.change_hz)
            r = ramp("df", d.time_s, k)
            if d.duration_s is not None:
                lim = f"{r}_held"
                a, b = sorted((0.0, k * float(d.duration_s)))
                blocks.append({"type": "limiter", "name": lim, "input": r, "minimum": a, "maximum": b})
                r = lim
            angle.append(r)
        elif d.kind == "frequency_ramp":
            rate = float(d.rate_hz_per_s)
            u = ramp("ramp", d.time_s, 1.0)
            if d.duration_s is not None:
                T = float(d.duration_s)
                blocks.append({"type": "limiter", "name": f"{u}_held", "input": u, "minimum": 0.0, "maximum": T})
                u = f"{u}_held"
                angle.append(ramp("ramp_after", d.time_s + T, 2.0 * math.pi * rate * T))
            blocks.append({"type": "product", "name": f"{u}_sq", "inputs": [u, u]})
            blocks.append({"type": "gain", "name": f"{u}_angle", "input": f"{u}_sq", "gain": math.pi * rate})
            angle.append(f"{u}_angle")
        else:
            raise ValueError(f"unknown disturbance {d!r}")
    m_name, phi_name = f"{name}_ed", f"{name}_angle"
    blocks.append({"type": "constant", "name": f"{name}_e0", "value": kd})
    blocks.append({"type": "sum", "name": m_name, "inputs": [f"{name}_e0", *magnitude],
                   "signs": [1.0] * (1 + len(magnitude))})
    blocks.append({"type": "constant", "name": f"{name}_eq", "value": 0.0})
    blocks.append({"type": "sum", "name": phi_name, "inputs": angle, "signs": [1.0] * len(angle)})
    blocks.append({"type": "dq0_to_abc", "name": f"{name}_e", "d": m_name, "q": f"{name}_eq",
                   "angle": phi_name})
    clash = sorted({b["name"] for b in blocks} & taken)
    if clash:
        raise ValueError(f"the device model already uses the name(s) {', '.join(clash)}; pass another name=")
    controls.extend(blocks)
    out = {p: f"{name}_e.{p}" for p in PHASES}
    out.update({"magnitude": m_name, "angle": phi_name})
    return out
