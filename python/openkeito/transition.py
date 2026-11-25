"""Transitions: an EMT run that follows a schedule from one time to another.

A time slice (``run_time_slices``) starts from the power flow of one time of
a schedule and stays there until a disturbance. A transition starts from the
power flow of ``from_time`` and moves the set points of the controls to the
values of ``to_time``: it shows how the network gets from one operating
point of the plan to the next.

The schedule's times are minutes or hours apart; the run is seconds long.
The change is therefore placed on the time axis of the run by arguments:
``start_time_s`` of steady state, then every set point moves over its ramp
time (``ramp_time_s``, or the change of active power over
``ramp_rate_mw_per_min``), then ``settle_time_s``. With
``ramp_time_s="schedule"`` the ramp takes the real time between the two
times (a schedule with ``"interpolation": "linear"`` only).

How the model follows the change (``build``): the network is converted at
both times, and the two models are compared component by component.

- Synchronous machines get an excitation system, a governor and a power
  system stabilizer (``add_machine_controls``; ``pss=False`` leaves the
  stabilizer out). The load reference of the governor moves by the
  difference of the torques, and the voltage reference by the difference of
  the terminal voltages and of the field voltages over the regulator gain,
  that the two power flows ask for: the machine then ends at the power flow
  of ``to_time`` when the rest of the network does.
- Converters (``grid_following_converter``) get their power references as
  signals that move from the one power flow to the other.
- Constant-impedance loads and shunts are split: what both times have stays
  connected, the rest is switched out or in, in ``load_steps`` equal parts
  spread over the ramp. A part that is an inductance alone gets 1 W of
  active power, which holds its node while its switch is open, and a change
  of less than 1 W per part is not switched at all.
- A branch, machine or converter that is in service at one of the times only
  is switched by breakers (a machine or converter that starts is connected
  from the beginning without load: it has been synchronized just before; a
  machine that stops is tripped at its output, and its load reference goes
  to zero with the breaker; a machine that stops and restarts re-synchronizes
  with the bus through its governor and excitation system before closing).
  A bus that a switch joins or splits is joined or split by a breaker.
- Sources (external grids) stay sources at the rated frequency: they are
  the reference of the frequency and take what the rest leaves. Their
  internal voltage moves from that of the one power flow to that of the
  other (``sources="hold"`` keeps it).
- Transformers and series impedances whose tap ratio, phase shift or
  impedance changes between the times step to the values of each time
  during the run (``tap_changes``, ``impedance_changes``).
- A network without a source (machines only) runs as an island: nothing
  holds the frequency but the governors, whose references move by what the
  two power flows ask for, so the frequency returns to the rated one as far
  as the loads take what the power flow of ``to_time`` says.

A transition can go through more than two times in one run
(``build_sequence``; ``times=[...]`` of the converters' ``transition``,
``chain=True`` of their ``run_transitions``): start_time_s of steady state,
then for each following time its changes and its settling time, one after
the other. The hours between the times are not run. Every time between the
first and the last is then both an end and a start: ``measure(..., time=...)``
compares the run at the end of the settling time of a time with the power
flow of that time. Running the intervals as cases of their own (the default
of ``run_transitions``) starts each from an exact steady state and uses
several cores; one run through all of them carries what is left of the
swings of one interval into the next, as the network would.

The functions for a tool are ``openkeito.pandapower.transition`` and
``run_transitions`` (and the same in ``openkeito.powsybl``).
"""

from __future__ import annotations

import cmath
import json
import math
from collections import defaultdict
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

from .convert import (
    MACHINE_LOADING,
    Conversion,
    ConversionError,
    machine_rating_for,
    model_version,
    summarize,
)

__all__ = [
    "EXCITER", "GOVERNOR", "PSS", "PSS_MINIMUM_ACTIVE_POWER_PU", "Stage", "Transition", "TransitionRun",
    "add_machine_controls", "build", "build_sequence", "chain_times", "common_machine_ratings", "estimate",
    "machine_inputs", "make_transition", "make_transitions", "measure", "run", "schedule_intervals",
]

#: A static excitation system reduced to its main path (IEEE Std 421.5 type
#: ST1A without its stabilizing blocks): Efd = KA (Vref - Vt) / (1 + s TA)
#: within the ceilings, which hold the state (non-windup). Values of the
#: thyristor exciter in the examples of Kundur, "Power System Stability and
#: Control" (1994), chapters 12 and 13: KA = 200, 0.02 s, ceilings -6.4 and
#: 7.0 per unit.
EXCITER: dict[str, float] = {"gain": 200.0, "time_constant_s": 0.02, "minimum_pu": -6.4, "maximum_pu": 7.0}

#: A governor with droop and a non-reheat steam turbine (Kundur, section
#: 11.1): valve = (Pref - (speed - 1) / R) / (1 + s TG) within its limits,
#: Pm = valve / (1 + s TCH), torque = Pm / speed. Typical values there:
#: R = 5 %, TG = 0.2 s, TCH = 0.3 s. In per unit of the machine rating.
GOVERNOR: dict[str, float] = {"droop_pu": 0.05, "governor_time_constant_s": 0.2, "turbine_time_constant_s": 0.3,
                              "minimum_pu": 0.0, "maximum_pu": 1.1}

#: A power system stabilizer of the form of IEEE Std 421.5 type PSS1A with the
#: speed deviation (per unit) as its input: Vs = gain * s TW / (1 + s TW) *
#: (1 + s T1) / (1 + s T2) * (1 + s T3) / (1 + s T4) * (speed - 1) within its
#: limits, added to the voltage reference of the excitation system. The
#: transducer lag and the torsional filter of the standard's block diagram
#: are left out (the machine has a rigid shaft, and the speed is the signal
#: of the model itself). Values of the stabilizer that Kundur, "Power System
#: Stability and Control" (1994), chapter 12, designs for a machine with the
#: thyristor exciter of ``EXCITER`` (gain 200) against an infinite bus:
#: gain 9.5, washout 1.4 s, one phase lead stage 0.154 s over 0.033 s. The
#: second stage is not used (both time constants 0: no block). The output
#: limits are inside the ranges that the same book gives for stabilizers
#: (0.1 to 0.2 per unit upwards, 0.05 to 0.1 downwards).
PSS: dict[str, float] = {"gain": 9.5, "washout_time_constant_s": 1.4,
                         "lead_time_constant_1_s": 0.154, "lag_time_constant_1_s": 0.033,
                         "lead_time_constant_2_s": 0.0, "lag_time_constant_2_s": 0.0,
                         "minimum_pu": -0.1, "maximum_pu": 0.1}
#: A machine that delivers less active power than this part of its rating
#: gets no stabilizer unless it is asked for by name. A stabilizer acts on
#: the torque through the change of the flux, in proportion to the load (the
#: constant K2 of the linearized machine: Kundur, chapter 12): for the
#: typical machine against an infinite bus it moves the decay of the swing
#: from -0.2 to -1.9 1/s at half load and by 0.03 1/s at none
#: (python/tests/test_pss.py). There it only moves the voltage with the
#: frequency, and the small synchronous condensers of case118, which absorb
#: 0.85 pu of reactive power, swing more with it than without.
PSS_MINIMUM_ACTIVE_POWER_PU = 0.1

#: Components that carry current between buses.
BRANCHES = ("pi_line", "coupled_rl", "bergeron_line", "three_phase_transformer", "three_phase_ideal_transformer")
SOURCES = ("three_phase_source", "three_phase_controlled_source")
SINE_TYPES = ("three_phase_source", "three_phase_current_source", "synchronous_machine", "grid_following_converter")
PHASES = ("a", "b", "c")


def _has_sine_source(components: list[dict[str, Any]]) -> bool:
    for c in components:
        t = c.get("type")
        if t in SINE_TYPES:
            return True
        if t in ("voltage_source", "current_source") and (c.get("waveform") or {}).get("type", "sine") == "sine":
            return True
    return False
#: Active power (MW) that holds the nodes behind an open breaker at a
#: defined voltage: 1 W at the rated voltage (1e-12 S at 1000 kV, which is
#: above the 1e-13 below which the solver takes a node for floating).
LEAK_MW = 1e-6


# Machines ----------------------------------------------------------------------


def machine_inputs(machine: dict[str, Any], s_mva: complex | None = None,
                   voltage_pu: float | None = None) -> tuple[float, float]:
    """(field voltage, mechanical torque) in per unit that hold a
    synchronous_machine component in the steady state in which it delivers
    ``s_mva`` at the terminal voltage ``voltage_pu`` (default: its
    operating_point), at rated speed and without saturation:
    E_Q = V + (Ra + j Xq) I gives the q axis, Efd = v_q + Ra i_q + Xd i_d and
    T = P + Ra |I|^2 (Kundur, section 3.6.3)."""
    point = machine.get("operating_point") or {}
    if s_mva is None:
        s_mva = complex(point.get("active_power_mw", 0.0), point.get("reactive_power_mvar", 0.0))
    v = float(point.get("voltage_pu", 1.0) if voltage_pu is None else voltage_pu)
    s = s_mva / machine["rated_power_mva"]
    i = (s / v).conjugate()
    ra, xd, xq = machine["ra_pu"], machine["xd_pu"], machine["xq_pu"]
    e_q = v + complex(ra, xq) * i
    turn = cmath.exp(-1j * cmath.phase(e_q))
    v_q, i_q, i_d = (v * turn).real, (i * turn).real, -(i * turn).imag
    return v_q + ra * i_q + xd * i_d, s.real + ra * abs(i) ** 2


def _load_angle_deg(machine: dict[str, Any], s_mva: complex, voltage_pu: float) -> float:
    """The angle in degrees by which the q axis of a machine leads its
    terminal voltage in the steady state of ``machine_inputs``. Beyond 90
    degrees the field of that state points against the voltage: the
    machine absorbs more reactive power than V^2 / Xq of its rating."""
    i = (s_mva / machine["rated_power_mva"] / voltage_pu).conjugate()
    return math.degrees(cmath.phase(voltage_pu + complex(machine["ra_pu"], machine["xq_pu"]) * i))


def _chosen(defaults: dict[str, float], given: Any, what: str) -> dict[str, float] | None:
    if given is False:
        return None
    values = dict(defaults)
    if isinstance(given, dict):
        unknown = sorted(set(given) - set(defaults))
        if unknown:
            raise ValueError(f"{what} has no parameter {', '.join(unknown)}; parameters: {', '.join(defaults)}")
        values.update(given)
    elif given not in (None, True):
        raise ValueError(f"{what} is a dict of parameters, True (typical values) or False (none)")
    return values


def _check_pss(p: dict[str, float]) -> None:
    """The values a stabilizer cannot be built with."""
    if not p["washout_time_constant_s"] > 0:
        raise ValueError("pss: washout_time_constant_s must be > 0")
    for k in ("1", "2"):
        lead, lag = p[f"lead_time_constant_{k}_s"], p[f"lag_time_constant_{k}_s"]
        if lead < 0 or lag < 0 or (lag == 0 and lead != 0):
            raise ValueError(f"pss: stage {k} needs lead_time_constant_{k}_s >= 0 and lag_time_constant_{k}_s > 0, "
                             "or both 0 for no stage")
    if not p["minimum_pu"] <= 0 <= p["maximum_pu"] or p["minimum_pu"] == p["maximum_pu"]:
        raise ValueError("pss: the limits must enclose 0 (minimum_pu <= 0 <= maximum_pu, not both 0)")


def add_machine_controls(
    conversion: Conversion,
    *,
    exciter: Any = True,
    governor: Any = True,
    pss: Any = False,
    machines: dict[str, dict[str, Any]] | None = None,
    _validate_keys: bool = True,
) -> dict[str, dict[str, Any]]:
    """Gives every synchronous machine of a conversion that has no field
    voltage or torque signal an excitation system and a governor made of
    control blocks, in place, and on request a power system stabilizer.

    exciter, governor: True (the typical values ``EXCITER`` and
    ``GOVERNOR``), a dict with some of their parameters, or False (the
    field voltage, or the torque, stays constant). pss: the same with
    ``PSS``; False (the default here; a transition asks for True) is no
    stabilizer. A stabilizer acts through the excitation system: a machine
    without one (``exciter=False``, or a field voltage signal of its own)
    gets none. Nor does a machine whose operating point has less active
    power than ``PSS_MINIMUM_ACTIVE_POWER_PU`` of its rating (a synchronous
    condenser), which a warning names. machines: {component name or
    element: {"exciter": ..., "governor": ..., "pss": ...}} for single
    machines; a stabilizer asked for here is given whatever the load.

    The blocks of the machine G are named ``G_<...>``. Excitation:
    ``G_efd`` = lag(gain (``G_vref`` + ``G_pss`` - ``G_vt``)). Governor:
    ``G_valve`` = lag(``G_pref`` + (1 - ``G_speed``) / droop), ``G_pm`` =
    lag(``G_valve``), ``G_tm`` = ``G_pm`` / ``G_speed``. Stabilizer:
    ``G_pss`` = limiter(lead-lag stages(``G_pss_washout``)) with
    ``G_pss_washout`` = gain s TW / (1 + s TW) (``G_speed`` - 1), which is 0
    in a steady state at any speed. The references are the values that
    hold the operating point at t = 0 (``initial_value`` blocks) plus the set
    point changes ``G_dvref`` and ``G_dpref``, constants 0 that a transition
    (or the caller) replaces by functions of time, in per unit of the
    machine's ratings.

    Returns {component name: {"exciter": parameters or None, "governor":
    parameters or None, "pss": parameters or None}} for the machines that
    got controls.
    """
    model = conversion.model
    controls = model.setdefault("controls", [])
    element_of = conversion.element_of()
    own = machines or {}
    if _validate_keys and own:
        known_machines = {
            name for c in model["components"] if c["type"] == "synchronous_machine"
            for name in (c["name"], element_of.get(c["name"])) if name
        }
        unknown_machines = sorted(set(own) - known_machines)
        if unknown_machines:
            raise ValueError(f"machines names no synchronous machine in the model: {', '.join(unknown_machines)}")
    done: dict[str, dict[str, Any]] = {}
    widened = []
    unloaded = []
    for c in model["components"]:
        if c["type"] != "synchronous_machine":
            continue
        g = c["name"]
        special = own.get(g) or own.get(element_of.get(g, "")) or {}
        ex = None if "field_voltage_signal" in c else _chosen(EXCITER, special.get("exciter", exciter), "exciter")
        gov = (None if "mechanical_torque_signal" in c
               else _chosen(GOVERNOR, special.get("governor", governor), "governor"))
        if ex is None and gov is None:
            continue
        stab = None if ex is None else _chosen(PSS, special.get("pss", pss), "pss")
        if stab is not None:
            _check_pss(stab)
            loading = abs((c.get("operating_point") or {}).get("active_power_mw", 0.0)) / c["rated_power_mva"]
            if "pss" not in special and loading < PSS_MINIMUM_ACTIVE_POWER_PU:
                stab = None
                unloaded.append(element_of.get(g, g))

        def signal(block: str, name: str) -> dict[str, Any]:
            return {"type": "measurement", "name": f"{g}_{block}",
                    "quantity": {"type": "component_signal", "component": g, "signal": name}}

        blocks: list[dict[str, Any]] = []
        if gov is not None or stab is not None:
            blocks += [signal("speed", "speed_pu"), {"type": "constant", "name": f"{g}_one", "value": 1.0}]
        if stab is not None:
            # Gain and washout in one block; then the lead-lag stages that
            # are in use, and the limits of the output.
            tw = stab["washout_time_constant_s"]
            blocks += [
                {"type": "sum", "name": f"{g}_pss_input", "inputs": [f"{g}_speed", f"{g}_one"],
                 "signs": [1.0, -1.0]},
                {"type": "transfer_function", "name": f"{g}_pss_washout", "input": f"{g}_pss_input",
                 "numerator": [stab["gain"] * tw, 0.0], "denominator": [tw, 1.0]},
            ]
            last = f"{g}_pss_washout"
            for k in ("1", "2"):
                lead, lag = stab[f"lead_time_constant_{k}_s"], stab[f"lag_time_constant_{k}_s"]
                if lag > 0:
                    blocks.append({"type": "lead_lag", "name": f"{g}_pss_lead{k}", "input": last,
                                   "lead_time_constant_s": lead, "lag_time_constant_s": lag})
                    last = f"{g}_pss_lead{k}"
            blocks.append({"type": "limiter", "name": f"{g}_pss", "input": last,
                           "minimum": stab["minimum_pu"], "maximum": stab["maximum_pu"]})
        if ex is not None:
            blocks += [
                signal("vt", "terminal_voltage_pu"), signal("efd0", "initial_field_voltage_pu"),
                {"type": "gain", "name": f"{g}_efd0_over_gain", "input": f"{g}_efd0", "gain": 1.0 / ex["gain"]},
                {"type": "sum", "name": f"{g}_vref_now", "inputs": [f"{g}_vt", f"{g}_efd0_over_gain"],
                 "signs": [1.0, 1.0]},
                {"type": "initial_value", "name": f"{g}_vref0", "input": f"{g}_vref_now"},
                {"type": "constant", "name": f"{g}_dvref", "value": 0.0},
                {"type": "sum", "name": f"{g}_vref", "inputs": [f"{g}_vref0", f"{g}_dvref"], "signs": [1.0, 1.0]},
                {"type": "sum", "name": f"{g}_v_error",
                 "inputs": [f"{g}_vref", f"{g}_vt"] + ([f"{g}_pss"] if stab is not None else []),
                 "signs": [1.0, -1.0] + ([1.0] if stab is not None else [])},
                {"type": "lag", "name": f"{g}_efd", "input": f"{g}_v_error", "gain": ex["gain"],
                 "time_constant_s": ex["time_constant_s"], "minimum": ex["minimum_pu"], "maximum": ex["maximum_pu"]},
            ]
            c["field_voltage_signal"] = f"{g}_efd"
        if gov is not None:
            gov = dict(gov)
            # The limits of the valve must hold the torque of the start.
            torque = machine_inputs(c)[1]
            if not gov["minimum_pu"] <= torque <= gov["maximum_pu"]:
                gov["minimum_pu"], gov["maximum_pu"] = min(gov["minimum_pu"], torque), max(gov["maximum_pu"], torque)
                widened.append(f"{element_of.get(g, g)} ({torque:.3g} pu)")
            blocks += [
                signal("tm0", "initial_mechanical_torque_pu"),
                {"type": "initial_value", "name": f"{g}_pref0", "input": f"{g}_tm0"},
                {"type": "constant", "name": f"{g}_dpref", "value": 0.0},
                {"type": "sum", "name": f"{g}_pref", "inputs": [f"{g}_pref0", f"{g}_dpref"], "signs": [1.0, 1.0]},
                {"type": "sum", "name": f"{g}_speed_error", "inputs": [f"{g}_one", f"{g}_speed"],
                 "signs": [1.0, -1.0]},
                {"type": "gain", "name": f"{g}_droop", "input": f"{g}_speed_error", "gain": 1.0 / gov["droop_pu"]},
                {"type": "sum", "name": f"{g}_p_command", "inputs": [f"{g}_pref", f"{g}_droop"],
                 "signs": [1.0, 1.0]},
                {"type": "lag", "name": f"{g}_valve", "input": f"{g}_p_command",
                 "time_constant_s": gov["governor_time_constant_s"], "minimum": gov["minimum_pu"],
                 "maximum": gov["maximum_pu"]},
                {"type": "lag", "name": f"{g}_pm", "input": f"{g}_valve",
                 "time_constant_s": gov["turbine_time_constant_s"]},
                {"type": "quotient", "name": f"{g}_tm", "inputs": [f"{g}_pm", f"{g}_speed"],
                 "minimum_denominator": 0.1},
            ]
            c["mechanical_torque_signal"] = f"{g}_tm"
        controls += blocks
        done[g] = {"exciter": ex, "governor": gov, "pss": stab}
    if done:
        model["version"] = "0.5"
    if unloaded:
        conversion.warnings += summarize({
            f"no stabilizer for machines that deliver less than {PSS_MINIMUM_ACTIVE_POWER_PU:g} of their rating as "
            "active power (a stabilizer acts in proportion to the load; {\"pss\": True} for the machine gives one "
            "all the same)": unloaded})
    if widened:
        conversion.warnings.append("the torque of the power flow is outside the limits of the governor valve, "
                                   "which were widened to hold it: " + ", ".join(widened))
    return done


# The change between conversions ------------------------------------------------


@dataclass
class Stage:
    """One time of a transition after its first: where the run is to arrive.

    time: the time of the schedule. conversion: the network converted at
    that time, with its power flow. buses: {bus key: model bus} of the buses
    of that time. expected: what its power flow gives for the machines and
    converters, {component: {"active_power_mw", "reactive_power_mvar"}}.
    change_time_s: when the changes towards this time begin; changes_end_s:
    when the last of them ends; settled_s: the end of the settling time
    after them, which is when the changes towards the next time begin, or
    the end of the run.
    """

    time: str
    conversion: Conversion
    buses: dict[str, str]
    expected: dict[str, dict[str, float]]
    change_time_s: float
    changes_end_s: float
    settled_s: float


@dataclass
class Transition:
    """One transition: the model that starts at the power flow of
    ``from_time`` and moves to the set points of ``to_time``, through the
    times between them when it was made of more than two.

    conversion: the model, its mapping, the warnings and the power flow of
    ``from_time`` (``conversion.compare`` checks the start). start, end: the
    conversions of the network at the first and the last time, as the
    converter returns them. end_buses: {bus key: model bus} for the buses of
    ``to_time``.
    change_time_s: when the first change begins; changes_end_s: when the last
    one ends; the run lasts to end_time_s. expected: what the power flow of
    ``to_time`` gives for the machines and converters, {component:
    {"active_power_mw", "reactive_power_mvar"}}. changes: one line per kind
    of change, for reading. stages: one ``Stage`` per time after the first
    (the last one repeats end, end_buses, expected, changes_end_s and
    end_time_s); ``times`` lists all times.
    """

    name: str
    from_time: str
    to_time: str
    conversion: Conversion
    start: Conversion
    end: Conversion
    end_buses: dict[str, str]
    change_time_s: float
    changes_end_s: float
    end_time_s: float
    expected: dict[str, dict[str, float]] = field(default_factory=dict)
    changes: list[str] = field(default_factory=list)
    #: Machines that stop: they run on alone behind their open breaker.
    stopped: list[str] = field(default_factory=list)
    stages: list[Stage] = field(default_factory=list)

    @property
    def model(self) -> dict[str, Any]:
        return self.conversion.model

    @property
    def warnings(self) -> list[str]:
        return self.conversion.warnings

    @property
    def times(self) -> list[str]:
        """The times the run goes through, the first one included."""
        return [self.from_time] + ([s.time for s in self.stages] or [self.to_time])

    def stage(self, time: Any = None) -> Stage:
        """The stage of a time: the last one (default), a time of the
        schedule after the first, or its position in ``times`` (1 is the
        second time; -1 the last)."""
        stages = self.stages or [Stage(time=self.to_time, conversion=self.end, buses=self.end_buses,
                                       expected=self.expected, change_time_s=self.change_time_s,
                                       changes_end_s=self.changes_end_s, settled_s=self.end_time_s)]
        if time is None:
            return stages[-1]
        if isinstance(time, int) and not isinstance(time, bool):
            if time == 0 or not -len(stages) <= time <= len(stages):
                raise ValueError(f"time {time} is not a position after the first in the {len(stages) + 1} times "
                                 "of the transition")
            return stages[time - 1] if time > 0 else stages[time]
        from .schedule import format_time

        wanted = format_time(time)
        for s in stages:
            if s.time == wanted:
                return s
        raise ValueError(f"{wanted} is not a time that the transition arrives at ({', '.join(s.time for s in stages)})")

    def estimate(self, *, calibrate: bool = True, calibration_steps: int = 200) -> dict[str, Any]:
        """Estimated step count and wall-clock runtime (see ``estimate``)."""
        return estimate(self, calibrate=calibrate, calibration_steps=calibration_steps)


def _lookup(setting: Any, keys: list[str], default: float | None, what: str = "setting") -> float | None:
    """A setting that is a number for all, or {element or component: number,
    "default": number}."""
    if isinstance(setting, dict):
        value = default
        for key in keys:
            if key in setting:
                value = setting[key]
                break
        else:
            if "default" in setting:
                value = setting["default"]
        if isinstance(value, str):
            raise ValueError(f"{what} must be a number in seconds (not {value!r}); "
                             "'schedule' is only for transition(..., ramp_time_s='schedule') on a linear schedule")
        return None if value is None else float(value)
    if isinstance(setting, str):
        raise ValueError(f"{what} must be a number in seconds (not {setting!r}); "
                         "'schedule' is only for transition(..., ramp_time_s='schedule') on a linear schedule")
    return default if setting is None else float(setting)


def _per_interval(setting: Any, intervals: int, what: str) -> list[Any]:
    """A setting given once for all intervals, or as a list with one entry
    per interval."""
    if isinstance(setting, (list, tuple)):
        if len(setting) != intervals:
            raise ValueError(f"{what} has {len(setting)} entries for {intervals} interval"
                             f"{'s' if intervals != 1 else ''} between the times: give one per interval, or one "
                             "value for all")
        return list(setting)
    return [setting] * intervals


def _profile(name: str, start_s: float, ramp_s: float, before: float, after: float) -> dict[str, Any]:
    """The block that is ``before`` until start_s and reaches ``after`` over
    ramp_s (a step when that is 0)."""
    if ramp_s <= 0:
        return {"type": "step", "name": name, "time_s": start_s, "before": before, "after": after}
    return {"type": "time_series", "name": name, "times_s": [start_s, start_s + ramp_s],
            "values": [before, after], "interpolation": "linear"}


def _series(name: str, pieces: list[tuple[float, float, float, float]], time_step_s: float) -> dict[str, Any]:
    """The block that makes the changes ``pieces`` one after the other, each
    (start_s, ramp_s, before, after) as in ``_profile``, and holds the value
    between them. One piece is the block of ``_profile``. Several are one
    time_series: with steps only, one that holds its values; otherwise a
    linear one, in which a step takes the time step before its instant (the
    value at every grid point is that of the step)."""
    if len(pieces) == 1:
        return _profile(name, *pieces[0])
    if all(ramp_s <= 0 for _, ramp_s, _, _ in pieces):
        return {"type": "time_series", "name": name, "times_s": [0.0] + [p[0] for p in pieces],
                "values": [pieces[0][2]] + [p[3] for p in pieces], "interpolation": "step"}
    times: list[float] = []
    values: list[float] = []
    for start_s, ramp_s, before, after in pieces:
        a, b = (start_s, start_s + ramp_s) if ramp_s > 0 else (start_s - time_step_s, start_s)
        if times and a < times[-1] + 1e-9 * time_step_s:
            if a < times[-1] - 0.5 * time_step_s or b <= times[-1]:
                raise ValueError(f"the changes of {name} overlap at {a:g} s: give a longer settle_time_s")
            # The change begins where the last one ended.
        else:
            times.append(a)
            values.append(before)
        times.append(b)
        values.append(after)
    return {"type": "time_series", "name": name, "times_s": times, "values": values, "interpolation": "linear"}


def _breaker(name: str, a: str, b: str, *, closes: bool, time_s: float) -> list[dict[str, Any]]:
    """Three ideal switches between the buses a and b that close, or open
    (each phase at its current zero), at time_s."""
    return _switches(name, a, b, [not closes, closes], [time_s])


def _switches(name: str, a: str, b: str, closed: list[bool], times_s: list[float],
              cycle_s: float = 0.0) -> list[dict[str, Any]]:
    """Ideal switches between the buses a and b, three per set, that are
    closed at the times k of a transition for which closed[k] is true: they
    act at times_s[k] between the times k and k + 1 (opening at the current
    zero of each phase). A switch closes once and opens once, so one set
    serves one time of being closed, or an opening and a closing again;
    further times of being closed get sets of their own in parallel
    (``<name>_r<j>``)."""
    actions = [(times_s[k], closed[k + 1]) for k in range(len(closed) - 1) if closed[k + 1] != closed[k]]
    for (t0, c0), (t1, c1) in zip(actions[:-1], actions[1:]):
        if not c0 and c1 and cycle_s > 0 and t1 - t0 < cycle_s - 1e-12:
            raise ValueError(f"switch {name} opens at {t0:g} s and closes again at {t1:g} s, within one power "
                             f"cycle ({cycle_s:g} s) while an opening waits for a current zero: give a longer "
                             "settle_time_s")
    if closed[0] and len(actions) == 2:
        sets = [{"initially_closed": True, "open_time_s": actions[0][0], "close_time_s": actions[1][0]}]
    else:
        sets = []
        current: dict[str, Any] | None = {"initially_closed": True} if closed[0] else None
        for time_s, closes in actions:
            if closes:
                current = {"close_time_s": time_s}
            else:
                sets.append({**(current or {}), "open_time_s": time_s})
                current = None
        if current is not None and actions:
            sets.append(current)
    out = []
    for j, action in enumerate(sets):
        label = name if j == 0 else f"{name}_r{j}"
        out += [{"type": "switch", "name": f"{label}_{p}", "nodes": [f"{a}.{p}", f"{b}.{p}"], **action}
                for p in PHASES]
    return out


def _ends(c: dict[str, Any]) -> list[str] | None:
    """The bus names of a component, when it names buses."""
    if isinstance(c.get("bus"), str):
        return [c["bus"]]
    if isinstance(c.get("buses"), list) and all(isinstance(b, str) for b in c["buses"]):
        return list(c["buses"])
    return None


def _set_ends(c: dict[str, Any], ends: list[str]) -> None:
    if "bus" in c:
        c["bus"] = ends[0]
    else:
        c["buses"] = list(ends)


def _without(c: dict[str, Any], *keys: str) -> dict[str, Any]:
    return {k: v for k, v in c.items() if k not in keys}


def _admittance(c: dict[str, Any] | None) -> complex:
    """G + jB of a three_phase_load in siemens (B > 0: inductive)."""
    if c is None:
        return 0j
    return complex(c["active_power_mw"], c["reactive_power_mvar"]) / c["rated_voltage_kv"] ** 2


def build(
    start: Conversion,
    end: Conversion,
    *,
    name: str = "transition",
    from_time: str = "",
    to_time: str = "",
    start_time_s: float = 1.0,
    ramp_time_s: Any = 5.0,
    ramp_rate_mw_per_min: Any = None,
    settle_time_s: Any = 10.0,
    end_time_s: float | None = None,
    switch_time_s: float | None = None,
    switch_delay_s: Any = None,
    load_steps: int = 4,
    exciter: Any = True,
    governor: Any = True,
    pss: Any = True,
    machine_controls: dict[str, dict[str, Any]] | None = None,
    sources: str = "follow",
    rename_buses: bool = False,
) -> Transition:
    """The transition from the conversion ``start`` to the conversion
    ``end`` of the same network at another time (both made with the same
    options; see the module text for what the model does).
    ``build_sequence`` takes more than two times.

    start_time_s: steady state before the first change.
    ramp_time_s: time over which a set point moves, for all, or {element or
    component: seconds, "default": seconds}; 0 is a step.
    ramp_rate_mw_per_min: instead, the rate at which the active power of a
    machine or a converter moves, for all or per element; its voltage or
    reactive power takes the same time.
    settle_time_s: time after the last change; the run ends then, unless
    end_time_s is given.
    switch_time_s: when breakers act (default: start_time_s);
    switch_delay_s: the same as a time after start_time_s.
    load_steps: parts in which the change of a load is switched, spread over
    its ramp time.
    exciter, governor, pss, machine_controls: see ``add_machine_controls``.
    The machines get a stabilizer unless pss=False: without one the typical
    governor leaves the swings between the machines nearly undamped, and
    the run does not settle at the power flow of ``end`` within its time.
    A machine without active power at both times gets none (see there).
    sources: "follow" (default): a source (an external grid, or a generator
    converted as a source) whose internal voltage differs between the two
    times becomes a three_phase_controlled_source whose voltage moves from
    the one to the other over the ramp time, in magnitude and angle; "hold":
    sources keep the voltage of from_time, and a warning gives the
    difference.
    rename_buses: the bus names of ``end`` are matched to those of ``start``
    by the bus keys of the mappings (for converters that number the buses
    anew at every conversion).
    """
    return build_sequence(
        [start, end], times=[from_time, to_time], name=name, start_time_s=start_time_s, ramp_time_s=ramp_time_s,
        ramp_rate_mw_per_min=ramp_rate_mw_per_min, settle_time_s=settle_time_s, end_time_s=end_time_s,
        switch_time_s=switch_time_s, switch_delay_s=switch_delay_s, load_steps=load_steps, exciter=exciter,
        governor=governor, pss=pss, machine_controls=machine_controls, sources=sources, rename_buses=rename_buses)


def _apply_max_pss_loading(
    conversions: list[Conversion],
    machine_controls: dict[str, dict[str, Any]],
    pss: Any,
    element_of: dict[str, str],
) -> dict[str, dict[str, Any]]:
    """Give a stabilizer to every machine that reaches PSS_MINIMUM_ACTIVE_POWER_PU
    at any time of ``conversions``, unless machine_controls already sets ``pss``."""
    controls = dict(machine_controls)
    if pss is False:
        return controls
    max_loading: dict[str, float] = {}
    for conv in conversions:
        for c in conv.model["components"]:
            if c["type"] != "synchronous_machine":
                continue
            loading = abs((c.get("operating_point") or {}).get("active_power_mw", 0.0)) / c["rated_power_mva"]
            max_loading[c["name"]] = max(max_loading.get(c["name"], 0.0), loading)
    for cname, loading in max_loading.items():
        if loading < PSS_MINIMUM_ACTIVE_POWER_PU:
            continue
        key = next((k for k in (cname, element_of.get(cname)) if k in controls), cname)
        if "pss" not in controls.get(key, {}):
            controls[key] = {**controls.get(key, {}), "pss": pss}
    return controls


class _Assembly:
    """State and per-kind builders for assembling a transition model across
    several conversions."""

    def __init__(
        self,
        *,
        conversions: list[Conversion],
        start: Conversion,
        times: list[str],
        ramps: list[Any],
        rates: list[Any],
        delays: list[Any],
        switch_time_s: float | None,
        load_steps: int,
        exciter: Any,
        governor: Any,
        pss: Any,
        machine_controls: dict[str, dict[str, Any]],
        sources: str,
        later_components: list[list[dict[str, Any]]],
        voltage_at: list[dict[str, tuple[float, float, float]]],
        assigned_new: dict[str, str],
        element_of: dict[str, str],
        aux_of: dict[str, list[str]],
    ) -> None:
        self.conversions = conversions
        self.start = start
        self.times = times
        self.n = len(conversions)
        self.m = self.n - 1
        self.two = self.n == 2
        self.step = float(start.model["simulation"]["time_step_s"])
        self.ramps = ramps
        self.rates = rates
        self.delays = delays
        self.switch_time_s = switch_time_s
        self.load_steps = load_steps
        self.exciter = exciter
        self.governor = governor
        self.pss = pss
        self.machine_controls = machine_controls
        self.sources = sources
        self.later_components = later_components
        self.voltage_at = voltage_at
        self.bus_voltage = voltage_at[0]
        self.assigned_new = assigned_new
        self.element_of = element_of
        self.aux_of = aux_of
        self.cycle_s = 1.0 / float(start.mapping.get("frequency_hz", 50.0))

    def when(self, k: int) -> str:
        return self.times[k] or ("the start" if k == 0 else "the end" if k == self.m else f"time {k + 1}")

    def label(self, cname: str) -> str:
        return self.element_of.get(cname, cname)

    def _touch(self, k: int, time_s: float) -> None:
        self.ends_s[k] = max(self.ends_s[k], time_s)

    def _ramp_of(self, cname: str, k: int, delta_mw: float = 0.0) -> float:
        keys = [self.label(cname), cname]
        rate = _lookup(self.rates[k], keys, None, "ramp_rate_mw_per_min")
        if rate is not None and delta_mw != 0:
            if rate <= 0:
                raise ValueError("ramp_rate_mw_per_min must be > 0")
            return abs(delta_mw) / rate * 60.0
        return max(_lookup(self.ramps[k], keys, 5.0, "ramp_time_s") or 0.0, 0.0)

    def _moves(self, block: str, pieces: list[tuple[int, float, float, float, float]], replace: bool = False) -> None:
        for k, start_s, ramp_s, _, _ in pieces:
            self._touch(k, start_s + ramp_s)
        if self.dry:
            return
        profile = _series(block, [piece[1:] for piece in pieces], self.step)
        if replace:
            self.blocks[block].clear()
            self.blocks[block].update(profile)
        else:
            self.added_controls.append(profile)

    def _acts(self, closed: list[bool]) -> None:
        for k in range(self.m):
            if closed[k + 1] != closed[k]:
                self._touch(k, self.t_switch[k])

    def _breaker(self, a: str, b: str, k: int, closes: bool) -> None:
        key = (a, b) if (a, b) in self.pairs else (b, a)
        if key not in self.pairs:
            key = (a, b)
            self.pairs[key] = {}
            self.components.append(key)
            self.counts["breakers that close" if closes else "breakers that open"] += 1
        if self.pairs[key].setdefault(k, closes) != closes and not self.two:
            self.problems.append(
                f"the buses {a} and {b} are joined by one element and split by another between "
                f"{self.when(k)} and {self.when(k + 1)}: such a change cannot be followed")
        self._touch(k, self.t_switch[k])

    def _breaker_states(self, key: tuple[str, str]) -> list[bool] | None:
        actions = self.pairs[key]
        state = [not actions[min(actions)]]
        for k in range(self.m):
            if k in actions and actions[k] == state[-1]:
                self.problems.append(
                    f"the buses {key[0]} and {key[1]} are {'joined' if state[-1] else 'split'} "
                    f"between {self.when(k)} and {self.when(k + 1)} without having been "
                    f"{'split' if state[-1] else 'joined'} since they last were: the bus names "
                    f"of the times do not tell how; make a transition of its own from {self.when(k)}")
                return None
            state.append(actions.get(k, state[-1]))
        return state

    def _leak(self, node: str, like: dict[str, Any], bus: str | None = None, end_index: int = 0) -> dict[str, Any]:
        kv = None
        if bus is not None:
            if bus in self.bus_voltage:
                kv = self.bus_voltage[bus][2]
            else:
                kv = next((vt[bus][2] for vt in self.voltage_at if bus in vt), None)
        if not kv:
            raw_kv = like.get("rated_voltage_kv")
            kv = raw_kv[end_index] if isinstance(raw_kv, list) else raw_kv
        if not kv:
            kv = next((v[2] for v in self.bus_voltage.values()), 1.0)
        return {"type": "three_phase_load", "name": f"{node}_leak", "bus": node, "rated_voltage_kv": kv,
                "rated_frequency_hz": float(self.start.mapping.get("frequency_hz", 50.0)),
                "active_power_mw": LEAK_MW, "reactive_power_mvar": 0.0}

    def _moved_ends(self, cname: str, states: list[dict[str, Any] | None]) -> list[str]:
        present = [k for k in range(self.n) if states[k] is not None]
        out = list(_ends(states[present[0]]) or [])
        for p, q in zip(present[:-1], present[1:]):
            pairs_of_ends = list(zip(_ends(states[p]) or [], _ends(states[q]) or []))
            if self.two:
                out = out[:len(pairs_of_ends)]
            for i, (x, y) in enumerate(pairs_of_ends):
                if x == y or i >= len(out):
                    continue
                if y in self.nodes_at[p]:
                    self._breaker(x, y, p, True)
                else:
                    self._breaker(x, y, p, False)
                    if out[i] not in (x, y):
                        self.problems.append(
                            f"the bus of {self.label(cname)} is {out[i]} when it is on its own, then {x}, and "
                            f"{y} from {self.when(q)}: buses that are joined and split under changing names "
                            f"cannot be followed; make a transition of its own from {self.when(p)}")
                    out[i] = y
        return out

    def _load(self, cname: str, states: list[dict[str, Any] | None], bus: str) -> None:
        like = next(s for s in states if s is not None)
        ys = [_admittance(s) for s in states]
        kv, f = like["rated_voltage_kv"], like["rated_frequency_hz"]
        scale = max(abs(y) for y in ys)

        def part(pname: str, y: complex, at: str) -> dict[str, Any]:
            return {**_without(like, "name", "bus", "active_power_mw", "reactive_power_mvar"), "name": pname,
                    "bus": at, "rated_voltage_kv": kv, "rated_frequency_hz": f,
                    "active_power_mw": max(y.real, 0.0) * kv ** 2, "reactive_power_mvar": y.imag * kv ** 2}

        base = [0.0, 0.0]
        parts: dict[tuple[bool, ...], list[float]] = {}
        for values, axis, sign in (([y.real for y in ys], 0, 1.0), ([max(y.imag, 0.0) for y in ys], 1, 1.0),
                                   ([max(-y.imag, 0.0) for y in ys], 1, -1.0)):
            levels = sorted(set(values))
            base[axis] += sign * levels[0]
            for low, high in zip(levels[:-1], levels[1:]):
                y = parts.setdefault(tuple(v >= high for v in values), [0.0, 0.0])
                y[axis] += sign * (high - low)
        small = 1e-9 * scale
        floor = max(small, self.load_steps * LEAK_MW / kv ** 2)
        switched = sorted(((closed, complex(*y)) for closed, y in parts.items() if abs(complex(*y)) > floor),
                          key=lambda item: (not item[0][0], [not c for c in item[0]]))
        if not switched:
            if states[0] is not None:
                self.components.append({**states[0], "bus": bus})
            return
        if abs(complex(*base)) > small:
            self.components.append(part(cname, complex(*base), bus))
        used: dict[str, int] = defaultdict(int)
        for closed, y in switched:
            changing = [k for k in range(self.m) if closed[k + 1] != closed[k]]
            ramp_s = {k: self._ramp_of(cname, k) for k in changing}
            steps = self.load_steps if any(r > 0 for r in ramp_s.values()) else 1
            kind = "out" if closed[0] else "in"
            used[kind] += 1
            kind += "" if used[kind] == 1 else f"{used[kind]}_"
            for j in range(steps):
                at = [self.begin[k] + (j + 0.5) * ramp_s[k] / steps if k in ramp_s else self.begin[k]
                      for k in range(self.m)]
                node = f"{cname}_{kind}{j}"
                piece = part(node, y / steps, node)
                if piece["reactive_power_mvar"] > 0:
                    piece["active_power_mw"] = max(piece["active_power_mw"], LEAK_MW)
                self.components.append(piece)
                self.components.extend(_switches(f"{node}_sw", bus, node, list(closed), at,
                                                 cycle_s=0.0 if self.dry else self.cycle_s))
                for k in changing:
                    self._touch(k, at[k])
        self.counts["loads and shunts that change"] += 1

    def _add_resync_controls(self, machine: dict[str, Any], bus: str,
                             spans: list[tuple[int, int]], gain: float) -> None:
        g = machine["name"]
        w0 = 2.0 * math.pi * float(self.start.mapping.get("frequency_hz", 50.0))
        v_peak = float(machine["rated_voltage_kv"]) * 1e3 * math.sqrt(2.0 / 3.0)
        wn = 12.0
        k_sync = (2.0 * float(machine["inertia_h_s"]) / w0) * (wn * wn)
        times_s = [0.0]
        values = [0.0]
        for k_open, k_close in spans:
            t_open = self.t_switch[k_open - 1]
            t_close = self.t_switch[k_close - 1]
            t_on = min(t_open + self.cycle_s, t_open + 0.25 * (t_close - t_open)) if self.cycle_s > 0 else t_open
            if t_on >= t_close:
                t_on = t_open
            times_s.extend([t_on, t_close])
            values.extend([1.0, 0.0])
        self.blocks[f"{g}_v_error"]["inputs"][0] = f"{g}_vref_active"
        tm_sig = machine["mechanical_torque_signal"]
        machine["mechanical_torque_signal"] = f"{g}_tm_total"
        self.added_controls.extend([
            {"type": "time_series", "name": f"{g}_sync_gate", "times_s": times_s,
             "values": values, "interpolation": "step"},
            {"type": "ramp", "name": f"{g}_grid_theta", "start_time_s": 0.0,
             "slope_per_s": w0, "initial": -math.pi / 2.0},
            {"type": "measurement", "name": f"{g}_delta",
             "quantity": {"type": "component_signal", "component": g, "signal": "angle_rad"}},
            {"type": "sum", "name": f"{g}_theta", "inputs": [f"{g}_grid_theta", f"{g}_delta"], "signs": [1.0, 1.0]},
            {"type": "three_phase_measurement", "name": f"{g}_bus_v",
             "quantity": {"type": "bus_voltage", "bus": bus}},
            {"type": "abc_to_dq0", "name": f"{g}_bus_dq",
             "inputs": [f"{g}_bus_v.a", f"{g}_bus_v.b", f"{g}_bus_v.c"], "angle": f"{g}_theta"},
            {"type": "function", "name": f"{g}_bus_vpk", "function": "hypot",
             "inputs": [f"{g}_bus_dq.d", f"{g}_bus_dq.q"]},
            {"type": "gain", "name": f"{g}_bus_vpu", "input": f"{g}_bus_vpk", "gain": 1.0 / v_peak},
            {"type": "gain", "name": f"{g}_sync_vref", "input": f"{g}_bus_vpu", "gain": 1.0 + 1.0 / gain},
            {"type": "selector", "name": f"{g}_vref_active", "condition": f"{g}_sync_gate",
             "inputs": [f"{g}_sync_vref", f"{g}_vref"]},
            {"type": "gain", "name": f"{g}_neg_d", "input": f"{g}_bus_dq.d", "gain": -1.0},
            {"type": "function", "name": f"{g}_d_angle", "function": "atan2",
             "inputs": [f"{g}_neg_d", f"{g}_bus_dq.q"]},
            {"type": "product", "name": f"{g}_sync_err", "inputs": [f"{g}_sync_gate", f"{g}_d_angle"]},
            {"type": "gain", "name": f"{g}_sync_kp", "input": f"{g}_sync_err", "gain": k_sync},
            {"type": "lead_lag", "name": f"{g}_sync_lead", "input": f"{g}_sync_kp",
             "lead_time_constant_s": 0.25, "lag_time_constant_s": 0.02},
            {"type": "limiter", "name": f"{g}_sync_lim", "input": f"{g}_sync_lead",
             "minimum": -2.0, "maximum": 2.0},
            {"type": "constant", "name": f"{g}_zero", "value": 0.0},
            {"type": "selector", "name": f"{g}_sync_tm", "condition": f"{g}_sync_gate",
             "inputs": [f"{g}_sync_lim", f"{g}_zero"]},
            {"type": "sum", "name": f"{g}_tm_total", "inputs": [tm_sig, f"{g}_sync_tm"], "signs": [1.0, 1.0]},
        ])

    def _machine(self, cname: str, states: list[dict[str, Any] | None], present: list[int],
                 p0: int, gone: int | None) -> None:
        bus = self._moved_ends(cname, states)[0]
        in_service = [s is not None for s in states]
        resyncs = gone is not None and any(k > gone for k in present)
        if resyncs:
            missing = []
            if f"{cname}_dpref" not in self.blocks:
                missing.append("governor")
            if f"{cname}_dvref" not in self.blocks:
                missing.append("exciter")
            if missing:
                need = " and ".join(missing)
                back = next(k for k in present if k > gone)
                self.problems.append(
                    f"E700: {self.label(cname)} is out of service at {self.when(gone)} and back in service at "
                    f"{self.when(back)}, so it must re-synchronize, which needs {need}; enable {need} "
                    f"(for example {', '.join(f'{m}=True' for m in missing)})")
                return
            for k in range(p0 + 1, self.n):
                if in_service[k] and not in_service[k - 1] and states[k]["bus"] not in self.voltage_at[k - 1]:
                    self.problems.append(
                        f"{self.label(cname)} cannot re-synchronize at {self.when(k)} because bus "
                        f"{states[k]['bus']} has no voltage at {self.when(k - 1)}: energize the bus first")
                    return
        a = states[0]
        if a is None:
            b = states[p0]
            dead = next((k for k in range(p0) if b["bus"] not in self.voltage_at[k]), None)
            if dead is not None:
                self.problems.append(f"{self.label(cname)} starts at a bus that has no voltage at "
                                     f"{self.when(dead)}: energize the bus in an earlier transition")
                return
            vm, va, kv = self.bus_voltage[b["bus"]]
            a = {**b, "bus": bus, "operating_point": {"active_power_mw": 0.0, "reactive_power_mvar": 0.0,
                                                        "voltage_pu": vm * kv / b["rated_voltage_kv"],
                                                        "angle_deg": va}}
            holder = Conversion(model={"components": [a], "controls": self.added_controls},
                                mapping={"elements": [{"component": cname, "element": self.label(cname)}]})
            own = self.machine_controls.get(cname) or self.machine_controls.get(self.label(cname)) or {}
            add_machine_controls(holder, exciter=self.exciter, governor=self.governor, pss=self.pss,
                                 machines={cname: own}, _validate_keys=False)
            for w in holder.warnings:
                if ": " in w and w.endswith(self.label(cname)):
                    prefix, _, _ = w.rpartition(": ")
                    self.notes[prefix].append(self.label(cname))
                else:
                    self.more_warnings.append(w)
            self.blocks.update({x["name"]: x for x in self.added_controls})
            self.counts["machines that start (connected without load from the beginning)"] += 1
        else:
            a = {**a, "bus": bus}
        asked: list[tuple[complex, float]] = []
        start_bus = states[p0]["bus"]
        for k in range(self.n):
            if states[k] is None:
                target_bus = start_bus if k < p0 else bus
                if target_bus in self.voltage_at[k]:
                    vm, _, kv = self.voltage_at[k][target_bus]
                    v_pu = vm * kv / a["rated_voltage_kv"]
                else:
                    v_pu = asked[-1][1]
                asked.append((0j, v_pu))
            else:
                point = states[k]["operating_point"] if k else a["operating_point"]
                asked.append((complex(point["active_power_mw"], point["reactive_power_mvar"]), point["voltage_pu"]))
        inputs = [machine_inputs(a)] + [machine_inputs(a, s, v) for s, v in asked[1:]]
        efd_a, tm_a = inputs[0]
        s_a, v_a = asked[0]
        for k, (s, v) in enumerate(asked):
            if states[k] is None:
                continue
            if inputs[k][0] < 0 or abs(_load_angle_deg(a, s, v)) >= 90.0:
                self.notes["the power flow asks for a steady state with a negative field voltage (the machine "
                           "absorbs more reactive power than V^2 / Xd of its rating, where no machine runs): "
                           "give it a larger rated_power_mva (machine_parameters)"].append(
                    f"{self.label(cname)} ({s.imag / a['rated_power_mva']:.3g} pu of reactive power at "
                    f"{self.when(k)})")
                break
        top = self.blocks.get(f"{cname}_valve", {}).get("maximum")
        running_k = [k for k in range(self.n) if states[k] is not None]
        high = max(running_k, key=lambda k: inputs[k][1])
        if top is not None and high > 0 and inputs[high][1] > top:
            self.notes[f"the torque that the power flow asks for is above the upper limit of the governor valve "
                       f"({top:g} pu of the rating), so the machine will not reach its power: give it a larger "
                       "rated_power_mva (machine_parameters) or another limit (governor)"].append(
                f"{self.label(cname)} ({inputs[high][1]:.3g} pu at {self.when(high)})")
        closed = [True if k < p0 else in_service[k] for k in range(self.n)]
        for k in range(1, self.n):
            if closed[k]:
                self.expected[k][cname] = {"active_power_mw": asked[k][0].real, "reactive_power_mvar": asked[k][0].imag}
        torque: list[tuple[int, float, float, float, float]] = []
        voltage: list[tuple[int, float, float, float, float]] = []
        running_torque_moves = False
        gain = self.blocks[f"{cname}_efd"]["gain"] if f"{cname}_dvref" in self.blocks else 1.0
        for k in range(self.m):
            opening = k >= p0 and in_service[k] and not in_service[k + 1]
            closing = k >= p0 and not in_service[k] and in_service[k + 1]
            start_s = self.t_switch[k] if (opening or closing) else self.begin[k]
            ramp_s = 0.0 if opening else self._ramp_of(cname, k, asked[k + 1][0].real - asked[k][0].real)
            before, after = inputs[k][1] - tm_a if k else 0.0, inputs[k + 1][1] - tm_a
            if abs(after - before) > 1e-12:
                torque.append((k, start_s, ramp_s, before, after))
                if not opening:
                    running_torque_moves = True
            if closed[k + 1] or any(closed[j] for j in range(k + 2, self.n)):
                before_v = (asked[k][1] - v_a) + (inputs[k][0] - efd_a) / gain if k else 0.0
                after_v = (asked[k + 1][1] - v_a) + (inputs[k + 1][0] - efd_a) / gain
                if abs(after_v - before_v) > 1e-12:
                    voltage.append((k, start_s, ramp_s, before_v, after_v))
        has_stop = any(not c for c in closed)
        changed = False
        machine_comp = dict(a)
        if has_stop:
            node = f"{cname}_t"
            self.components.extend(_switches(f"{cname}_brk", a["bus"], node, closed, self.t_switch,
                                             cycle_s=0.0 if self.dry else self.cycle_s))
            self._acts(closed)
            machine_comp["bus"] = node
            if (machine_comp.get("neutral") or {}).get("type") == "ungrounded":
                self.components.append(self._leak(node, machine_comp, a["bus"]))
            self.counts["machines that stop (breaker opens)"] += 1
            if resyncs:
                self.counts["machines that re-synchronize (breaker closes)"] += 1
            if not in_service[-1]:
                self.stopped.append(cname)
            if f"{cname}_dpref" not in self.blocks:
                self.notes["stops without a governor: its speed rises after the breaker opens"].append(
                    self.label(cname))
        if f"{cname}_dpref" in self.blocks:
            if torque:
                self._moves(f"{cname}_dpref", torque, replace=True)
                changed = (not has_stop) or running_torque_moves
        elif any(abs(asked[k][0].real - s_a.real) > 1e-6 * a["rated_power_mva"] for k in running_k if k > 0):
            self.notes["no governor (or a torque signal of its own): the active power stays at that of "
                       f"{self.when(0)}"].append(self.label(cname))
        if f"{cname}_dvref" in self.blocks:
            if voltage:
                self._moves(f"{cname}_dvref", voltage, replace=True)
                changed = True
        elif any(abs(asked[k][1] - v_a) > 1e-6 or abs(inputs[k][0] - efd_a) > 1e-6 for k in running_k if k > 0):
            self.notes["no excitation system (or a field voltage signal of its own): the field voltage stays at "
                       f"that of {self.when(0)}"].append(self.label(cname))
        if resyncs and not self.dry:
            spans: list[tuple[int, int]] = []
            k = p0 + 1
            while k < self.n:
                if closed[k - 1] and not closed[k]:
                    close_k = next((j for j in range(k + 1, self.n) if closed[j]), None)
                    if close_k is not None:
                        spans.append((k, close_k))
                        k = close_k + 1
                        continue
                k += 1
            if spans:
                self._add_resync_controls(machine_comp, bus, spans, gain)
        self.components.append(machine_comp)
        if not has_stop:
            self.counts["machines whose set points move"] += changed
        elif changed:
            self.counts["machines whose set points move"] += 1

    def _converter(self, cname: str, states: list[dict[str, Any] | None], p0: int) -> None:
        bus = self._moved_ends(cname, states)[0]
        a = states[0]
        if a is None:
            b = states[p0]
            dead = next((k for k in range(p0) if b["bus"] not in self.voltage_at[k]), None)
            if dead is not None:
                self.problems.append(f"{self.label(cname)} starts at a bus that has no voltage at "
                                     f"{self.when(dead)}: energize the bus in an earlier transition")
                return
            vm, va, kv = self.bus_voltage[b["bus"]]
            a = {**b, "bus": bus,
                 "reference": {**b["reference"], "active_power_mw": 0.0, "reactive_power_mvar": 0.0},
                 "operating_point": {"active_power_mw": 0.0, "reactive_power_mvar": 0.0,
                                     "voltage_kv": vm * kv, "angle_deg": va}}
        else:
            a = {**a, "bus": bus}
        ref_a = a["reference"]
        if ref_a.get("type") != "power" or not all(isinstance(ref_a.get(k, 0.0), (int, float))
                                                   for k in ("active_power_mw", "reactive_power_mvar")):
            self.notes["references that are not powers given as numbers: kept"].append(self.label(cname))
            self.components.append(a)
            return
        powers = [complex(ref_a["active_power_mw"], ref_a.get("reactive_power_mvar", 0.0))]
        for k in range(1, self.n):
            ref = states[k]["reference"] if states[k] is not None else {}
            powers.append(complex(ref.get("active_power_mw", 0.0), ref.get("reactive_power_mvar", 0.0)))
            self.expected[k][cname] = {"active_power_mw": powers[k].real, "reactive_power_mvar": powers[k].imag}
        tolerance = 1e-12 * max(1.0, a["rated_power_mva"])
        if any(abs(powers[k + 1] - powers[k]) > tolerance for k in range(self.m)):
            reference = dict(ref_a)
            for key, block, part in (("active_power_mw", f"{cname}_p_ref", "real"),
                                     ("reactive_power_mvar", f"{cname}_q_ref", "imag")):
                values = [getattr(s, part) for s in powers]
                pieces = [(k, self.begin[k], self._ramp_of(cname, k, powers[k + 1].real - powers[k].real),
                           values[k], values[k + 1])
                          for k in range(self.m) if values[k] != values[k + 1]
                          and (self.two or abs(powers[k + 1] - powers[k]) > tolerance)]
                if pieces:
                    self._moves(block, pieces)
                    reference[key] = block
            a = {**a, "reference": reference}
            limit = float(a.get("current_limit_pu", 1.1)) * a["rated_power_mva"]
            worst = max(range(1, self.n), key=lambda k: abs(powers[k]))
            if abs(powers[worst]) > limit:
                self.notes["the powers of the end are above the current limit of the converter, which will not "
                           "reach them" if self.two else
                           "the powers of a time are above the current limit of the converter, which will not "
                           "reach them"].append(
                    f"{self.label(cname)} ({abs(powers[worst]):.4g} MVA, limit {limit:.4g})" if self.two else
                    f"{self.label(cname)} ({abs(powers[worst]):.4g} MVA at {self.when(worst)}, limit {limit:.4g})")
            self.counts["converters whose references move"] += 1
        self.components.append(a)

    def _branch_tap_changes(
        self, cname: str, kind: str, c: dict[str, Any], states: list[dict[str, Any] | None],
        present: list[int], count_change: bool,
    ) -> tuple[str, ...]:
        """Applies tap or impedance changes across ``present`` times onto ``c``,
        returning the fields handled so non-tap changes still warn."""
        if kind == "three_phase_transformer":
            fields = ("rated_voltage_kv", "impedance_percent", "resistance_percent")
            prev = (list(c["rated_voltage_kv"]), float(c["impedance_percent"]), float(c.get("resistance_percent", 0.0)))
            changes = []
            for k in present[1:]:
                s = states[k]
                cur = (list(s["rated_voltage_kv"]), float(s["impedance_percent"]),
                       float(s.get("resistance_percent", 0.0)))
                if cur != prev:
                    t = self.t_switch[k - 1]
                    changes.append({
                        "time_s": t,
                        "rated_voltage_kv": cur[0],
                        "impedance_percent": cur[1],
                        "resistance_percent": cur[2],
                    })
                    self._touch(k - 1, t)
                    prev = cur
            if changes:
                c["tap_changes"] = changes
                if count_change:
                    self.counts["transformers whose taps move"] += 1
            return fields
        if kind == "three_phase_ideal_transformer":
            fields = ("rated_voltage_kv", "phase_shift_deg")
            prev = (list(c["rated_voltage_kv"]), float(c.get("phase_shift_deg", 0.0)))
            changes = []
            for k in present[1:]:
                s = states[k]
                cur = (list(s["rated_voltage_kv"]), float(s.get("phase_shift_deg", 0.0)))
                if cur != prev:
                    t = self.t_switch[k - 1]
                    changes.append({
                        "time_s": t,
                        "rated_voltage_kv": cur[0],
                        "phase_shift_deg": cur[1],
                    })
                    self._touch(k - 1, t)
                    prev = cur
            if changes:
                c["tap_changes"] = changes
                if count_change:
                    self.counts["transformers whose taps move"] += 1
            return fields
        if kind == "coupled_rl":
            fields = ("impedance",)
            prev = c["impedance"]
            changes = []
            for k in present[1:]:
                s = states[k]
                cur = s["impedance"]
                if cur != prev:
                    t = self.t_switch[k - 1]
                    changes.append({"time_s": t, "impedance": cur})
                    self._touch(k - 1, t)
                    prev = cur
            if changes:
                c["impedance_changes"] = changes
                if count_change:
                    self.counts["impedances that change"] += 1
            return fields
        return ()

    def _branch(self, cname: str, kind: str, states: list[dict[str, Any] | None],
                present: list[int], p0: int, always: bool) -> None:
        a = states[0]
        is_aux_z = kind == "coupled_rl" and any(cname in aux for aux in self.aux_of.values())
        if always:
            c = {**a, "buses": self._moved_ends(cname, states)}
            handled = self._branch_tap_changes(cname, kind, c, states, present, count_change=not is_aux_z)
            differs = next((k for k in present[1:] if _without(states[p0], "buses", *handled) !=
                            _without(states[k], "buses", *handled)), None)
            if differs is not None:
                self.notes["the values differ between the two times (a tap change, most likely), which cannot be "
                           f"changed during a run: kept at those of {self.when(0)}" if self.two else
                           "the values differ between the times (a tap change, most likely), which cannot be "
                           f"changed during a run: kept at those of {self.when(p0)}"].append(
                    self.label(cname) if self.two else f"{self.label(cname)} (from {self.when(differs)})")
            self.components.append(c)
            return
        c = dict(states[p0])
        handled = self._branch_tap_changes(cname, kind, c, states, present, count_change=not is_aux_z)
        differs = next((k for k in present[1:] if _without(states[p0], "buses", *handled) !=
                        _without(states[k], "buses", *handled)), None)
        if differs is not None:
            self.notes["the values differ between the two times (a tap change, most likely), which cannot be "
                       f"changed during a run: kept at those of {self.when(0)}" if self.two else
                       "the values differ between the times (a tap change, most likely), which cannot be "
                       f"changed during a run: kept at those of {self.when(p0)}"].append(
                self.label(cname) if self.two else f"{self.label(cname)} (from {self.when(differs)})")
        closed = [s is not None for s in states]
        aux_names = self.aux_of.get(cname, [])
        z_name = f"{cname}_z"
        zc = dict(self.of[p0][z_name]) if kind == "three_phase_ideal_transformer" and z_name in aux_names and z_name in self.of[p0] else None
        if zc is not None and len(c["buses"]) == 2 and len(zc["buses"]) == 2 and c["buses"][1] == zc["buses"][0]:
            z_states = [at.get(z_name) for at in self.of]
            z_present = [k for k in present if z_states[k] is not None]
            if len(z_present) > 1:
                self._branch_tap_changes(z_name, "coupled_rl", zc, z_states, z_present, count_change=False)
            bus1, m_node, bus2 = c["buses"][0], c["buses"][1], zc["buses"][1]
            node0, node1 = f"{cname}_x0", f"{cname}_x1"
            self.components.extend(_switches(f"{cname}_brk0", bus1, node0, closed, self.t_switch,
                                             cycle_s=0.0 if self.dry else self.cycle_s))
            self.components.append(self._leak(node0, c, bus1, 0))
            self.components.extend(_switches(f"{cname}_brk1", bus2, node1, closed, self.t_switch,
                                             cycle_s=0.0 if self.dry else self.cycle_s))
            self.components.append(self._leak(node1, zc, bus2, 1))
            self._acts(closed)
            self.components.append({**c, "buses": [node0, m_node]})
            self.components.append({**zc, "buses": [m_node, node1]})
            self.handled_aux.add(z_name)
            for aname in aux_names:
                if aname == z_name:
                    continue
                ac = self.of[p0].get(aname)
                if ac is None:
                    continue
                if ac["type"] == "three_phase_load" and ac.get("bus") == m_node:
                    self.components.append(dict(ac))
                    self.handled_aux.add(aname)
                elif ac["type"] == "three_phase_transformer" and isinstance(ac.get("buses"), list) \
                        and ac["buses"][0] == bus2:
                    self.components.append({**ac, "buses": [node1, ac["buses"][1]]})
                    self.handled_aux.add(aname)
        else:
            orig_buses = list(c["buses"])
            ends = []
            for k, bus in enumerate(orig_buses):
                node = f"{cname}_x{k}"
                self.components.extend(_switches(f"{cname}_brk{k}", bus, node, closed, self.t_switch,
                                                 cycle_s=0.0 if self.dry else self.cycle_s))
                self.components.append(self._leak(node, c, bus, k))
                ends.append(node)
            self._acts(closed)
            self.components.append({**c, "buses": ends})
            for aname in aux_names:
                ac = self.of[p0].get(aname)
                if ac is None or ac["type"] != "three_phase_load":
                    continue
                abus = ac.get("bus")
                if abus not in orig_buses:
                    continue
                if len(orig_buses) == 2 and orig_buses[0] != orig_buses[1]:
                    end_idx = orig_buses.index(abus)
                else:
                    end_idx = 0 if aname.endswith(("_y1", "_g0", "_y1_u")) else 1
                self.components.append({**ac, "bus": ends[end_idx]})
                self.handled_aux.add(aname)
        self.counts["branches switched in" if a is None else "branches switched out"] += 1

    def _other(self, cname: str, kind: str, states: list[dict[str, Any] | None],
               always: bool, gone: int | None) -> None:
        a = states[0]
        if a is None:
            self.notes[f"in service at {self.when(self.m)} only and of a type that cannot be switched in: left out"
                       if self.two else
                       f"not in service at {self.when(0)} and of a type that cannot be switched in: left "
                       "out"].append(f"{self.label(cname)} ({kind})")
            return
        if not always:
            self.notes[f"not in service at {self.when(gone)} but of a type that cannot be switched out: "
                       "kept"].append(f"{self.label(cname)} ({kind})")
            self.components.append(a)
            return
        ends = self._moved_ends(cname, states)
        if kind == "three_phase_source":
            e = [cmath.rect(s["line_voltage_rms_v"], math.radians(s.get("angle_deg", 0.0))) for s in states]
            ea = e[0]
            moving = [k for k in range(self.m) if abs(e[k + 1] - e[k]) > 1e-9 * abs(ea)]
            if moving and self.sources == "follow":
                w0 = 2 * math.pi * a["frequency_hz"]
                if "transition_theta" not in self.blocks:
                    theta = {"type": "ramp", "name": "transition_theta", "start_time_s": 0.0,
                             "slope_per_s": w0, "initial": w0 * self.step}
                    self.added_controls.append(theta)
                    self.blocks["transition_theta"] = theta
                kd = math.sqrt(2.0 / 3.0)
                ramp_s = {k: self._ramp_of(cname, k) for k in moving}
                self._moves(f"{cname}_ed", [(k, self.begin[k], ramp_s[k], kd * e[k].real, kd * e[k + 1].real)
                                            for k in moving])
                self._moves(f"{cname}_eq", [(k, self.begin[k], ramp_s[k], kd * e[k].imag, kd * e[k + 1].imag)
                                            for k in moving])
                if not self.dry:
                    self.added_controls.append({"type": "dq0_to_abc", "name": f"{cname}_e", "d": f"{cname}_ed",
                                                "q": f"{cname}_eq", "angle": "transition_theta"})
                c = {"type": "three_phase_controlled_source", "name": cname, "bus": ends[0],
                     "signals": [f"{cname}_e.{p}" for p in PHASES],
                     "steady_state": {"line_voltage_rms_v": a["line_voltage_rms_v"],
                                      "angle_deg": a.get("angle_deg", 0.0)}}
                c.update({key: a[key] for key in ("neutral", "impedance") if key in a})
                self.components.append(c)
                self.counts["sources whose internal voltage moves"] += 1
                return
            worst = max(range(1, self.n), key=lambda k: abs(e[k] - ea))
            if abs(e[worst] - ea) > 1e-6 * abs(ea):
                self.notes["sources keep the internal voltage (magnitude and angle) of "
                           f"{self.when(0)}; that of {self.when(self.m)} differs by" if self.two else
                           "sources keep the internal voltage (magnitude and angle) of "
                           f"{self.when(0)}; that of a later time differs by up to"].append(
                    f"{self.label(cname)} {100 * abs(e[worst] - ea) / abs(ea):.3g} %")
        elif any(_without(a, "bus", "buses") != _without(s, "bus", "buses") for s in states[1:]):
            self.notes[f"of a type that cannot follow a set point: kept at the values of "
                       f"{self.when(0)}"].append(f"{self.label(cname)} ({kind})")
        c = dict(a)
        if ends:
            _set_ends(c, ends)
        self.components.append(c)

    def run_pass(self, begin: list[float], dry: bool) -> dict[str, Any]:
        """Assemble the model with interval changes beginning at ``begin``.
        When ``dry`` is true, only compute ``ends_s``."""
        self.begin = begin
        self.dry = dry
        model = json.loads(json.dumps(self.start.model))
        self.of = [{c["name"]: c for c in model["components"]}] + [
            {c["name"]: c for c in json.loads(json.dumps(part))} for part in self.later_components[1:]]
        self.nodes_at = [{b for c in at.values() for b in (_ends(c) or [])} for at in self.of]
        self.blocks = {b["name"]: b for b in model.get("controls", [])}
        self.t_switch = [begin[k] if not self.delays[k] else begin[k] + float(self.delays[k])
                         for k in range(self.m)]
        if self.switch_time_s is not None:
            self.t_switch[0] = float(self.switch_time_s)
        self.components = []
        self.added_controls = []
        self.notes = defaultdict(list)
        self.counts = defaultdict(int)
        self.problems = []
        self.more_warnings = []
        self.expected = [{} for _ in range(self.n)]
        self.ends_s = list(begin)
        self.pairs = {}
        self.stopped = []
        self.handled_aux = set()

        names = list(self.of[0])
        for at in self.of[1:]:
            names += [cname for cname in at if cname not in self.of[0] and cname not in names[len(self.of[0]):]]
        for cname in names:
            if cname in self.handled_aux:
                continue
            states = [at.get(cname) for at in self.of]
            present = [k for k in range(self.n) if states[k] is not None]
            p0 = present[0]
            kinds = [states[k]["type"] for k in present]
            other = next((k for k in present if states[k]["type"] != kinds[0]), None)
            if other is not None:
                self.problems.append(f"{self.label(cname)} is a {kinds[0]} at {self.when(p0)} and a "
                                     f"{states[other]['type']} at {self.when(other)}: such a change cannot be followed")
                continue
            kind = kinds[0]
            always = len(present) == self.n
            gone = next((k for k in range(p0, self.n) if states[k] is None), None)
            if kind == "three_phase_load":
                self._load(cname, states, self._moved_ends(cname, states)[0])
            elif kind == "synchronous_machine":
                self._machine(cname, states, present, p0, gone)
            elif kind == "grid_following_converter":
                self._converter(cname, states, p0)
            elif kind in BRANCHES:
                self._branch(cname, kind, states, present, p0, always)
            else:
                self._other(cname, kind, states, always, gone)

        placed: list[dict[str, Any]] = []
        for c in self.components:
            if isinstance(c, tuple):
                closed = self._breaker_states(c)
                if closed is not None:
                    placed.extend(_switches(f"brk_{c[0]}_{c[1]}", c[0], c[1], closed, self.t_switch,
                                            cycle_s=0.0 if dry else self.cycle_s))
            else:
                placed.append(c)
        if self.problems:
            raise ConversionError(self.problems)
        if not dry:
            held_at_start = {b for c in placed if c.get("type") not in ("switch", "three_phase_current_source")
                             for b in (_ends(c) or [])}
            switch_buses = {n.rsplit(".", 1)[0] for c in placed if c.get("type") == "switch"
                            for n in c.get("nodes", []) if "." in n}
            for b in sorted(switch_buses - held_at_start):
                if b in self.assigned_new.values():
                    placed.append(self._leak(b, {}, b))
            if not _has_sine_source(placed):
                first_bus = next(iter(self.start.mapping["buses"].values()), None)
                if first_bus is None:
                    first_bus = next((b for c in placed for b in (_ends(c) or [])), None)
                if first_bus is not None:
                    used_names = {c["name"] for c in placed}
                    ref_name = "transition_freq_ref"
                    idx = 0
                    while ref_name in used_names:
                        ref_name = f"transition_freq_ref_{idx}"
                        idx += 1
                    placed.append({
                        "type": "three_phase_current_source",
                        "name": ref_name,
                        "bus": first_bus,
                        "current_rms_a": 0.0,
                        "frequency_hz": float(self.start.mapping.get("frequency_hz", 50.0)),
                        "angle_deg": 0.0,
                    })
        model["components"] = placed
        model["controls"] = list(self.blocks.values()) + [c for c in self.added_controls if c["name"] not in self.blocks]
        return {"model": model, "ends_s": self.ends_s, "t_switch": self.t_switch, "expected": self.expected,
                "notes": self.notes, "counts": self.counts, "warnings": self.more_warnings,
                "stopped": self.stopped, "names_at_first": set(self.of[0])}


def build_sequence(
    conversions: list[Conversion],
    *,
    times: list[str] | None = None,
    name: str = "transition",
    start_time_s: float = 1.0,
    ramp_time_s: Any = 5.0,
    ramp_rate_mw_per_min: Any = None,
    settle_time_s: Any = 10.0,
    end_time_s: float | None = None,
    switch_time_s: float | None = None,
    switch_delay_s: Any = None,
    load_steps: int = 4,
    exciter: Any = True,
    governor: Any = True,
    pss: Any = True,
    machine_controls: dict[str, dict[str, Any]] | None = None,
    sources: str = "follow",
    rename_buses: bool = False,
) -> Transition:
    """One transition through the conversions of the same network at two or
    more times (all made with the same options), in one run: it starts at
    the power flow of the first, and for every following time the set
    points move to those of that time and the run settles there before the
    changes towards the next time begin. With two conversions this is
    ``build``, whose text describes the arguments.

    The time axis of the run: start_time_s of steady state; then, for the
    interval from each time to the next, the changes (ramps from the
    beginning of the interval, breakers at its beginning, or switch_delay_s
    after it) and the settling time after the last of them. The hours
    between the times of a schedule are not run: the run is the changes and
    the settling, one after the other.

    times: the times, for the texts and for ``Transition.stage``.
    ramp_time_s, ramp_rate_mw_per_min, settle_time_s, switch_delay_s: one
    value for all intervals (as in ``build``), or a list with one entry per
    interval. settle_time_s must be at least two time steps between two
    intervals (the last one may be 0). switch_time_s (a time of the run) is
    for two times only. end_time_s replaces the settling time of the last
    interval.

    What happens at the times between the first and the last:

    - Set points (machines, converters, sources) are one function of time
      through all their values.
    - Loads and shunts: what all times have stays connected; every part
      that some times have and others do not is switched in and out as
      often as needed, in load_steps parts over the ramp of the interval.
    - Branches may go out and come back any number of times. So may the
      breakers that join and split buses, as long as a bus keeps the name
      it had when it was last on its own.
    - A machine that starts at a later time is connected from the beginning
      without load, and its voltage reference follows the voltage of its
      bus until its time. A machine may stop once (its breaker opens and it
      runs on alone); one that is to come back after that is refused: a
      machine cannot be synchronized during a run. Make one transition up
      to the time at which it is out and another from there.
    - A converter that is out at a time has references of zero there.
    - Tap changes stay at the ratio of the first time, sources that are out
      at a time stay in: the warnings name the element and the time.
    """
    conversions = list(conversions)
    n = len(conversions)
    if n < 2:
        raise ValueError("a transition needs the conversions of two times or more")
    times = [""] * n if times is None else ["" if t is None else str(t) for t in times]
    if len(times) != n:
        raise ValueError(f"{len(times)} times for {n} conversions")
    m = n - 1
    two = n == 2
    settles = _per_interval(settle_time_s, m, "settle_time_s")
    if start_time_s <= 0 or any(s < 0 for s in settles):
        raise ValueError("start_time_s must be > 0 and settle_time_s >= 0")
    if load_steps < 1:
        raise ValueError("load_steps is the number of parts of a load change: 1 or more")
    if sources not in ("follow", "hold"):
        raise ValueError("sources is 'follow' or 'hold'")
    ramps = _per_interval(ramp_time_s, m, "ramp_time_s")
    rates = _per_interval(ramp_rate_mw_per_min, m, "ramp_rate_mw_per_min")
    delays = _per_interval(switch_delay_s, m, "switch_delay_s")
    if switch_time_s is not None and (not two or delays[0] is not None):
        raise ValueError("switch_time_s is a time of the run, for a transition between two times and without "
                         "switch_delay_s; with more times give switch_delay_s (seconds after the beginning of "
                         "the changes of each interval)")
    if any(d is not None and d < 0 for d in delays):
        raise ValueError("switch_delay_s must be >= 0")

    def when(k: int) -> str:
        return times[k] or ("the start" if k == 0 else "the end" if k == m else f"time {k + 1}")

    first = conversions[0]
    start = Conversion(model=json.loads(json.dumps(first.model)), mapping=json.loads(json.dumps(first.mapping)),
                       warnings=list(first.warnings), power_flow=dict(first.power_flow))
    element_of: dict[str, str] = {}
    for conv in reversed(conversions):
        element_of.update(conv.element_of())
    # Validate user-supplied per-element dicts against all elements across the
    # sequence so typos are caught instead of silently ignored.
    known_machines = {
        key for conv in conversions for c in conv.model["components"]
        if c["type"] == "synchronous_machine"
        for key in (c["name"], element_of.get(c["name"])) if key
    }
    machine_controls = dict(machine_controls or {})
    unknown_controls = sorted(set(machine_controls) - known_machines)
    if unknown_controls:
        raise ValueError(f"machine_controls names no synchronous machine in the network: {', '.join(unknown_controls)}")
    known_elements = {
        key for conv in conversions for c in conv.model["components"]
        for key in (c["name"], element_of.get(c["name"])) if key
    } | {"default"}
    for opt_name, per_interval in (("ramp_time_s", ramps), ("ramp_rate_mw_per_min", rates)):
        for setting in per_interval:
            if isinstance(setting, dict):
                unknown = sorted(set(setting) - known_elements)
                if unknown:
                    raise ValueError(f"{opt_name} names no element in the network: {', '.join(unknown)}")
    machine_controls = _apply_max_pss_loading(conversions, machine_controls, pss, element_of)
    add_machine_controls(start, exciter=exciter, governor=governor, pss=pss, machines=machine_controls,
                         _validate_keys=False)
    step = float(start.model["simulation"]["time_step_s"])
    if any(s < 2 * step for s in settles[:-1]):
        raise ValueError("settle_time_s must be at least two time steps between the changes of two intervals "
                         "(only the last one may be 0)")

    # The components and buses of the later times, under the bus names of
    # the first where the converter numbers the buses anew.
    later_components: list[list[dict[str, Any]]] = [[]]
    buses_at: list[dict[str, str]] = [dict(start.mapping["buses"])]
    used_buses: set[str] = set(start.mapping["buses"].values())
    assigned_new: dict[str, str] = {}
    next_bus_id = 0
    for conv in conversions[1:]:
        components_k = json.loads(json.dumps(conv.model["components"]))
        buses_k = dict(conv.mapping["buses"])
        if rename_buses:
            rename: dict[str, str] = {}
            for key, b in conv.mapping["buses"].items():
                a = start.mapping["buses"].get(key)
                if a is None:
                    a = assigned_new.get(key)
                    if a is None:
                        while f"b{next_bus_id}" in used_buses:
                            next_bus_id += 1
                        a = f"b{next_bus_id}"
                        next_bus_id += 1
                        assigned_new[key] = a
                        used_buses.add(a)
                if rename.setdefault(b, a) != a:
                    raise ConversionError([
                        f"bus {b} in a later conversion maps to multiple buses ({rename[b]} and {a}); "
                        "cannot rename buses consistently"])
            if len(set(rename.values())) != len(rename):
                raise ConversionError(["bus renaming produced duplicate bus names"])
            for c in components_k:
                ends = _ends(c)
                if ends:
                    _set_ends(c, [rename.get(b, b) for b in ends])
            buses_k = {key: rename.get(b, b) for key, b in buses_k.items()}
        later_components.append(components_k)
        buses_at.append(buses_k)
    #: Power flow (vm_pu, va_degree, kV) by model bus, at every time.
    voltage_at = [{b: conv.power_flow[key] for key, b in buses.items() if key in conv.power_flow}
                  for conv, buses in zip(conversions, buses_at)]
    aux_of: dict[str, list[str]] = {}
    for conv in reversed(conversions):
        for e in conv.mapping.get("elements", []):
            if e.get("type") in BRANCHES and len(e.get("components", [])) > 1:
                aux_of[e["component"]] = [x for x in e["components"] if x != e["component"]]

    assembly = _Assembly(
        conversions=conversions, start=start, times=times, ramps=ramps, rates=rates, delays=delays,
        switch_time_s=switch_time_s, load_steps=load_steps, exciter=exciter, governor=governor, pss=pss,
        machine_controls=machine_controls, sources=sources, later_components=later_components,
        voltage_at=voltage_at, assigned_new=assigned_new, element_of=element_of, aux_of=aux_of,
    )

    # Where the changes of each interval begin: after those of the one
    # before and its settling time, on a grid point.
    begin: list[float] = [start_time_s] * m
    if not two:
        lengths = assembly.run_pass(begin, True)["ends_s"]
        for k in range(1, m):
            reach = begin[k - 1] + (lengths[k - 1] - start_time_s) + float(settles[k - 1])
            begin[k] = round(reach / step) * step
    made = assembly.run_pass(begin, False)
    model, ends_s, t_switch, notes = made["model"], made["ends_s"], made["t_switch"], made["notes"]
    start.warnings.extend(made["warnings"])

    changes_end = ends_s[-1]
    end_s = float(end_time_s) if end_time_s is not None else changes_end + float(settles[-1])
    if end_s < changes_end:
        raise ValueError(f"end_time_s = {end_s:g} is before the end of the changes at {changes_end:g} s")
    # Whole time steps, so that the last grid point is the end.
    end_s = round(end_s / step) * step
    components = model["components"]
    ver = model_version(components)
    if ver == "0.6" or model.get("version") == "0.6":
        model["version"] = "0.6"
    elif model["controls"] and model.get("version", "0.4") < "0.5":
        model["version"] = "0.5"
    model["simulation"]["end_time_s"] = end_s
    model["name"] = f"{model.get('name', 'network')}: {name}"
    settled = begin[1:] + [end_s]
    if two:
        model["description"] = (f"Transition from {when(0)} to {when(1)}, made by "
                                "openkeito.transition: starts from the power flow of the first time; the set points "
                                f"move from {start_time_s:g} s on.")
    else:
        model["description"] = (
            f"Transition from {when(0)} through {n - 2} time{'s' if n > 3 else ''} to {when(m)}, made by "
            "openkeito.transition: starts from the power flow of the first time; the set points move from "
            f"{start_time_s:g} s on and the run has settled at "
            + ", ".join(f"{when(k + 1)} by {settled[k]:g} s" for k in range(m)) + ".")
    _add_outputs(model)

    names = {c["name"] for c in components}
    elements = list(start.mapping["elements"])
    listed = set(made["names_at_first"])
    for conv in conversions[1:]:
        new = [e for e in conv.mapping["elements"] if e["component"] in names and e["component"] not in listed]
        listed.update(e["component"] for e in new)
        elements += new
    mapping = {**start.mapping, "elements": elements}
    nodes = {b for c in components for b in (_ends(c) or [])}
    nodes |= {n.rsplit(".", 1)[0] for c in components if c.get("type") == "switch"
              for n in c.get("nodes", []) if "." in n}
    changes = [f"{what}: {count}" for what, count in made["counts"].items() if count]
    if not changes:
        if two:
            notes["nothing changes between the two times"].append(f"{times[0]} and {times[1]}")
        else:
            notes["nothing changes between the times"].append(", ".join(when(k) for k in range(n)))
    warnings = list(start.warnings) + summarize(notes)
    # Warn before running if an intermediate settling time is shorter than the
    # slow control time constants attached (governor/turbine or PSS washout).
    slow_taus = [
        float(b["denominator"][0]) for b in model.get("controls", [])
        if b.get("type") == "transfer_function" and b.get("name", "").endswith("_pss_washout")
        and b.get("denominator")
    ] + [
        float(b["time_constant_s"]) for b in model.get("controls", [])
        if b.get("type") == "lag" and (b.get("name", "").endswith("_valve") or b.get("name", "").endswith("_pm"))
        and float(b.get("time_constant_s", 0.0)) >= 0.1
    ]
    if slow_taus and m > 1:
        min_settle = max(2.0, max(slow_taus))
        for k in range(m - 1):
            if settles[k] < min_settle:
                warnings.append(
                    f"intermediate stage {when(k + 1)} has settle_time_s={settles[k]:g} s, shorter than "
                    f"the slow control time constants (recommended >= {min_settle:g} s); swings may carry "
                    "into the next stage"
                )
    stages = [Stage(time=times[k], conversion=conversions[k],
                    buses={key: b for key, b in buses_at[k].items() if b in nodes}, expected=made["expected"][k],
                    change_time_s=min(begin[k - 1], t_switch[k - 1]), changes_end_s=ends_s[k - 1],
                    settled_s=settled[k - 1]) for k in range(1, n)]
    return Transition(
        name=name, from_time=times[0], to_time=times[-1],
        conversion=Conversion(model=model, mapping=mapping, warnings=warnings, power_flow=dict(start.power_flow)),
        start=start, end=conversions[-1], end_buses=stages[-1].buses,
        change_time_s=min(start_time_s, t_switch[0]), changes_end_s=changes_end, end_time_s=end_s,
        expected=stages[-1].expected, changes=changes, stopped=made["stopped"], stages=stages,
    )


def _add_outputs(model: dict[str, Any]) -> None:
    """Adds what ``measure`` reads: of every machine the speed, powers,
    terminal voltage and dq currents; of every converter its powers,
    currents and frequency; of every source its currents."""
    outputs = model["outputs"]
    have = {json.dumps(o, sort_keys=True) for o in outputs}
    controls = model.setdefault("controls", [])
    blocks = {b["name"] for b in controls}

    def out(o: dict[str, Any]) -> None:
        key = json.dumps(o, sort_keys=True)
        if key not in have:
            have.add(key)
            outputs.append(o)

    for c in model["components"]:
        g = c["name"]
        if c["type"] == "synchronous_machine":
            for block, signal in (("speed", "speed_pu"), ("vt", "terminal_voltage_pu"), ("p", "active_power_pu"),
                                  ("q", "reactive_power_pu"), ("id", "id_pu"), ("iq", "iq_pu")):
                if f"{g}_{block}" not in blocks:
                    blocks.add(f"{g}_{block}")
                    controls.append({"type": "measurement", "name": f"{g}_{block}",
                                     "quantity": {"type": "component_signal", "component": g, "signal": signal}})
                out({"type": "signal", "signal": f"{g}_{block}"})
        elif c["type"] == "grid_following_converter":
            for signal in ("p", "q", "i_d", "i_q", "pll.frequency_hz"):
                out({"type": "signal", "signal": f"{g}.{signal}"})
        elif c["type"] in SOURCES:
            out({"type": "branch_current", "component": g})


# From a schedule ---------------------------------------------------------------


def schedule_intervals(schedule: Any, intervals: Any = None) -> list[tuple[str, str]]:
    """The (from_time, to_time) pairs of a schedule: every pair of
    neighbouring times (default), or the pairs given."""
    from .schedule import format_time, parse_time

    if intervals is None:
        times = schedule.times
        if len(times) < 2:
            raise ValueError("a transition needs two times; the schedule has one")
        return list(zip(times[:-1], times[1:]))
    pairs = []
    for pair in intervals:
        if len(pair) != 2:
            raise ValueError("an interval is [from_time, to_time]")
        a, b = (format_time(t) for t in pair)
        if parse_time(a) >= parse_time(b):
            raise ValueError(f"the interval {a} to {b} does not go forward in time")
        pairs.append((a, b))
    return pairs


def chain_times(schedule: Any, times: Any = None, intervals: Any = None) -> list[str]:
    """The times of one transition through several times of a schedule: the
    ``times`` given (in order), the ends of the ``intervals`` when each
    begins where the one before ends, or all times of the schedule."""
    from .schedule import format_time, parse_time

    if times is not None and intervals is not None:
        raise ValueError("give the times of the chain or its intervals, not both")
    if intervals is not None:
        pairs = schedule_intervals(schedule, intervals)
        if not pairs:
            raise ValueError("a transition needs two times; no interval is given")
        broken = next((k for k in range(1, len(pairs)) if pairs[k][0] != pairs[k - 1][1]), None)
        if broken is not None:
            raise ValueError(f"the intervals do not join into one chain: {pairs[broken - 1][1]} is followed by an "
                             f"interval from {pairs[broken][0]}; give intervals that each begin where the one "
                             "before ends, or run them one by one (without chain)")
        return [pairs[0][0]] + [b for _, b in pairs]
    if isinstance(times, str) and times != "all":
        raise ValueError('times is a list of times of the schedule, or "all"')
    chosen = [format_time(t) for t in (schedule.times if times is None or isinstance(times, str) else times)]
    if len(chosen) < 2:
        raise ValueError(f"a transition needs two times; {len(chosen)} given")
    for a, b in zip(chosen[:-1], chosen[1:]):
        if parse_time(a) >= parse_time(b):
            raise ValueError(f"the times do not go forward: {b} is not after {a}")
    return chosen


def common_machine_ratings(conversions: list[Conversion]) -> dict[str, tuple[float, str]]:
    """For the machines whose rating the data does not give
    (``Conversion.assumed_machine_ratings``): the rating that holds every
    time of ``conversions`` (``openkeito.convert.machine_rating_for``, and
    not below the one that any time alone was given), where it differs from
    the rating of one of the times. {key of the element in
    machine_parameters: (rating in MVA, component)}."""
    points: dict[str, list[tuple[complex, float]]] = defaultdict(list)
    had: dict[str, list[float]] = defaultdict(list)
    about: dict[str, tuple[str, float | None]] = {}
    for conv in conversions:
        machines = {c["name"]: c for c in conv.model["components"] if c["type"] == "synchronous_machine"}
        for key, entry in conv.assumed_machine_ratings.items():
            c = machines.get(entry["component"])
            if c is None:
                continue
            point = c["operating_point"]
            points[key].append((complex(point["active_power_mw"], point["reactive_power_mvar"]), point["voltage_pu"]))
            had[key].append(c["rated_power_mva"])
            # A reactance in ohm is another one in per unit at another
            # rating: a larger rating would not help the machine to absorb.
            about[key] = (entry["component"], None if entry.get("xd_from_ohm") else c["xd_pu"])
    out = {}
    for key, (component, xd_pu) in about.items():
        rating = max(machine_rating_for(points[key], xd_pu), max(had[key]))
        if any(abs(rating - r) > 1e-9 * rating for r in had[key]):
            out[key] = (rating, component)
    return out


def _parse_make_options(
    schedule: Any,
    times: list[str],
    convert: Callable[..., Conversion],
    options: dict[str, Any],
    *,
    per_pair: bool = False,
) -> tuple[dict[str, Any], dict[str, Any], bool]:
    """Split ``options`` into (build_options, convert_options, schedule_ramp)."""
    import inspect

    from .schedule import parse_time

    opts = dict(options)
    known = set(inspect.signature(build).parameters) - {"start", "end", "name", "from_time", "to_time",
                                                        "rename_buses"}
    build_options = {k: opts.pop(k) for k in list(opts) if k in known}
    ramp = build_options.get("ramp_time_s")
    listed = isinstance(ramp, (list, tuple))
    intervals = max(len(times) - 1, 1)
    per_interval = list(ramp) if listed else [ramp] * intervals
    wrong = next((r for r in per_interval if isinstance(r, str) and r != "schedule"), None)
    if wrong is not None:
        raise ValueError(f'ramp_time_s is seconds, a dict of them per element, or "schedule"; not {wrong!r}')
    real = [isinstance(r, str) for r in per_interval]
    schedule_ramp = False
    if any(real):
        if schedule.interpolation != "linear":
            raise ValueError('ramp_time_s="schedule" takes the real time between the two times, which a schedule '
                             'with "interpolation": "step" does not give: its values jump. Give ramp_time_s in '
                             "seconds, or 0 for a step")
        if per_pair and not listed:
            schedule_ramp = True
        elif len(times) == 2 and not listed:
            seconds = (parse_time(times[-1]) - parse_time(times[0])).total_seconds()
            build_options["ramp_time_s"] = seconds
            if build_options.get("switch_delay_s") is None:
                build_options.setdefault("switch_time_s", build_options.get("start_time_s", 1.0) + seconds)
        else:
            if len(per_interval) != len(times) - 1:
                raise ValueError(f"ramp_time_s has {len(per_interval)} entries for {len(times) - 1} intervals")
            between = [(parse_time(y) - parse_time(x)).total_seconds() for x, y in zip(times[:-1], times[1:])]
            build_options["ramp_time_s"] = [d if r else given for d, r, given in zip(between, real, per_interval)]
            build_options.setdefault("switch_delay_s", [d if r else None for d, r in zip(between, real)])
    accepted = set(inspect.signature(convert).parameters)
    opts.setdefault("generators", "machine")
    opts.setdefault("inverter_model", "converter")
    opts.setdefault("output_interval_steps", 20)
    unknown = sorted(set(opts) - accepted)
    if unknown:
        raise ValueError(f"unknown option {', '.join(unknown)}; the options are those of the transition "
                         f"({', '.join(sorted(known))}) and of the converter ({', '.join(sorted(accepted))})")
    return build_options, opts, schedule_ramp


def _convert_schedule_times(
    network: Any,
    schedule: Any,
    times: list[str],
    apply: Callable[[Any, Any, Any], Any],
    convert: Callable[..., Conversion],
    options: dict[str, Any],
) -> tuple[dict[str, Conversion], dict[str, tuple[float, str]]]:
    """Convert each unique time in ``times`` once, rate unrated machines across
    all conversions once, and re-convert once when ratings change."""
    import inspect

    accepted = set(inspect.signature(convert).parameters)
    opts = dict(options)

    def convert_all() -> dict[str, Conversion]:
        made: dict[str, Conversion] = {}
        for time in times:
            try:
                made[time] = convert(apply(network, schedule, time), **opts)
            except ConversionError as e:
                raise ConversionError([f"{time}: {p}" for p in e.problems]) from e
        return made

    conversions = convert_all()
    ratings = common_machine_ratings(list(conversions.values())) if "machine_parameters" in accepted else {}
    if ratings:
        given = dict(opts.get("machine_parameters") or {})
        for key, (rating, _) in ratings.items():
            given[key] = {**(given.get(key) or {}), "rated_power_mva": rating}
        opts["machine_parameters"] = given
        conversions = convert_all()
    return conversions, ratings


def _attach_rating_warning(made: Transition, ratings: dict[str, tuple[float, str]]) -> None:
    if not ratings:
        return
    element_of = made.conversion.element_of()
    made.warnings.extend(summarize({
        f"no rating; rated for all times of the transition (the largest apparent power is "
        f"{100 * MACHINE_LOADING:g} % of the rating, and the reactive power absorbed at most "
        f"{100 * MACHINE_LOADING:g} % of V^2 / Xd)": [
            f"{element_of.get(component, component)} ({rating:.4g} MVA)"
            for rating, component in ratings.values()]}))


def make_transition(
    network: Any,
    schedule: Any,
    from_time: Any = None,
    to_time: Any = None,
    *,
    times: Any = None,
    check: Callable[[Any, Any], None],
    apply: Callable[[Any, Any, Any], Any],
    convert: Callable[..., Conversion],
    name: str | None = None,
    rename_buses: bool = False,
    options: dict[str, Any],
) -> Transition:
    """The transition of a converter (its ``transition``): ``check`` the
    schedule, ``convert`` the networks that ``apply`` makes of the schedule's
    values at the two times (default: its first two times; any two times
    within the schedule), and ``build`` the model. With ``times`` (a list of
    two or more times, or "all" for those of the schedule) the transition
    goes through all of them in one run (``build_sequence``). ``options``
    holds the arguments of ``build`` and those of the converter.

    A generator without a rating in the data is rated for all times of the
    transition (``common_machine_ratings``), and not, as a time slice, for
    the output of one: a warning lists these machines with their ratings.
    ``machine_parameters={element: {"rated_power_mva": ...}}`` gives a
    machine its rating instead.

    Unless the options say otherwise, generators become synchronous
    machines and inverters converters, the outputs are thinned to every
    20th step, and ``ramp_time_s="schedule"`` takes the real time between
    the two times, which needs a schedule with linear interpolation (in a
    list of ramp times, for the intervals that say so)."""
    from .schedule import format_time, parse_time

    check(network, schedule)
    if times is not None:
        if from_time is not None or to_time is not None:
            raise ValueError("give from_time and to_time, or times, not both")
        chosen = chain_times(schedule, times)
    else:
        if from_time is None:
            from_time = schedule.times[0]
        if to_time is None:
            later = [t for t in schedule.times if parse_time(t) > parse_time(from_time)]
            if not later:
                raise ValueError(f"the schedule has no time after {format_time(from_time)}")
            to_time = later[0]
        chosen = [format_time(from_time), format_time(to_time)]
    a, b = chosen[0], chosen[-1]
    if (parse_time(b) - parse_time(a)).total_seconds() <= 0:
        raise ValueError(f"to_time {b} is not after from_time {a}")

    build_options, conv_options, _ = _parse_make_options(schedule, chosen, convert, options)
    conversions, ratings = _convert_schedule_times(network, schedule, chosen, apply, convert, conv_options)
    try:
        made = build_sequence([conversions[t] for t in chosen], times=chosen, name=name or "transition",
                              rename_buses=rename_buses, **build_options)
    except ConversionError as e:
        raise ConversionError([f"{a} to {b}: {p}" for p in e.problems]) from e
    _attach_rating_warning(made, ratings)
    return made


def estimate(transition: Transition, *, calibrate: bool = True, calibration_steps: int = 200) -> dict[str, Any]:
    """Counts and optional short-run calibration estimating the wall-clock time
    of running ``transition``."""
    import time

    import openkeito

    if calibrate and calibration_steps < 1:
        raise ValueError("calibration_steps must be >= 1")
    model = transition.model
    sim = model["simulation"]
    time_step_s = float(sim["time_step_s"])
    end_time_s = float(transition.end_time_s)
    steps = int(round(end_time_s / time_step_s))
    comps = model.get("components", [])
    out: dict[str, Any] = {
        "time_step_s": time_step_s,
        "end_time_s": end_time_s,
        "steps": steps,
        "stages": len(transition.stages) or 1,
        "components": len(comps),
        "controls": len(model.get("controls", [])),
        "switches": sum(1 for c in comps if c.get("type") == "switch"),
    }
    if calibrate:
        sample = {**model, "simulation": {**sim, "end_time_s": min(end_time_s, calibration_steps * time_step_s)}}
        t0 = time.perf_counter()
        result = openkeito.run(sample)
        elapsed = time.perf_counter() - t0
        cal_steps = max(int(result.summary.get("solver", {}).get("steps")
                            or round(sample["simulation"]["end_time_s"] / time_step_s)), 1)
        out["step_wall_us"] = elapsed / cal_steps * 1e6
        out["estimated_wall_s"] = elapsed * (steps / cal_steps)
    return out


# Running -----------------------------------------------------------------------


@dataclass
class TransitionRun:
    """Transitions run together: the cases, the batch result and one row per
    case (see ``measure``). time_rows: one row per case and time that the
    case arrives at, measured at the end of the settling time of that time
    against its power flow (for a transition between two times, the row of
    the case again)."""

    transitions: list[Transition]
    batch: Any
    rows: list[dict[str, Any]]
    time_rows: list[dict[str, Any]] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)

    def to_pandas(self) -> Any:
        import pandas as pd

        return pd.DataFrame(self.rows)


def _magnitudes(result: Any, bus: str) -> Any:
    """(line-to-line rms voltage in V, angle in rad against cos(w0 t) without
    the rotation) of a bus at every recorded time, from the three phase
    voltages: for balanced phases sqrt(va^2 + vb^2 + vc^2) is the
    line-to-line rms value at every instant."""
    import numpy as np

    try:
        va, vb, vc = (result[f"V({bus}.{p})"] for p in PHASES)
    except KeyError:
        return None
    return np.sqrt(va * va + vb * vb + vc * vc), np.arctan2((vb - vc) / math.sqrt(3), (2 * va - vb - vc) / 3)


def _measure_whole_run(
    transition: Transition,
    result: Any,
    conv: Conversion,
    machines: list[dict[str, Any]],
    converters: list[dict[str, Any]],
    f0: float,
    before: Any,
) -> tuple[dict[str, Any], Any]:
    """Compute whole-run min/max voltages, drift, frequencies and peak currents once."""
    import numpy as np

    out: dict[str, Any] = {}
    low, high, drift = (math.inf, ""), (-math.inf, ""), 0.0
    seen = set()
    for key, bus in conv.mapping["buses"].items():
        if bus in seen:
            continue
        seen.add(bus)
        found = _magnitudes(result, bus)
        if found is None:
            continue
        vm = found[0] / (conv.power_flow[key][2] * 1e3)
        if before.any():
            drift = max(drift, float(np.max(np.abs(vm[before] - vm[0]))))
        if vm.min() < low[0]:
            low = (float(vm.min()), bus)
        if vm.max() > high[0]:
            high = (float(vm.max()), bus)
    if seen and math.isfinite(low[0]):
        out.update({"drift_before_change_pu": drift, "voltage_min_pu": low[0], "voltage_min_bus": low[1],
                    "voltage_max_pu": high[0], "voltage_max_bus": high[1]})

    frequencies = [f0 * result[f"S({c['name']}_speed)"] for c in machines
                   if f"S({c['name']}_speed)" in result and c["name"] not in transition.stopped]
    if not frequencies:
        frequencies = [result[f"S({c['name']}.pll.frequency_hz)"] for c in converters
                       if f"S({c['name']}.pll.frequency_hz)" in result]
    stacked = np.vstack(frequencies) if frequencies else None
    if stacked is not None:
        out["frequency_min_hz"], out["frequency_max_hz"] = float(stacked.min()), float(stacked.max())
    for kind, group, d, q in (("machine", machines, "S({}_id)", "S({}_iq)"),
                              ("converter", converters, "S({}.i_d)", "S({}.i_q)")):
        peaks = [float(np.max(np.hypot(result[d.format(c["name"])], result[q.format(c["name"])])))
                 for c in group if d.format(c["name"]) in result]
        if peaks:
            out[f"max_{kind}_current_pu"] = max(peaks)
    return out, stacked


def _measure_stage_end(
    transition: Transition,
    result: Any,
    conv: Conversion,
    stage: Stage,
    last: Any,
    last_check: Any,
    f0: float,
    t: Any,
    element_of: dict[str, str],
    machines: list[dict[str, Any]],
    converters: list[dict[str, Any]],
    stacked: Any,
) -> dict[str, Any]:
    """Compute end-of-stage comparisons against ``stage`` power flow."""
    import numpy as np

    out: dict[str, Any] = {}
    check_f_swing = 0.0
    if stacked is not None:
        out["end_frequency_hz"] = float(stacked[:, last].mean())
        out["end_frequency_swing_hz"] = float(np.max(stacked[:, last].max(axis=1) - stacked[:, last].min(axis=1)))
        check_f_swing = float(np.max(stacked[:, last_check].max(axis=1) - stacked[:, last_check].min(axis=1)))

    dv, dva, swing, check_v_swing, reference = 0.0, 0.0, 0.0, 0.0, None
    seen = set()
    for key, bus in stage.buses.items():
        if bus in seen:
            continue
        seen.add(bus)
        found = _magnitudes(result, bus)
        if found is None:
            continue
        vm_pu, va_degree, kv = stage.conversion.power_flow[key]
        vm = found[0][last] / (kv * 1e3)
        dv = max(dv, abs(float(vm.mean()) - vm_pu))
        swing = max(swing, float(vm.max() - vm.min()))
        vm_check = found[0][last_check] / (kv * 1e3)
        check_v_swing = max(check_v_swing, float(vm_check.max() - vm_check.min()))
        turn = np.exp(1j * (found[1][last] - 2 * math.pi * f0 * t[last]))
        if reference is None:
            reference = (turn, va_degree)
            continue
        angle = math.degrees(cmath.phase(complex(np.mean(turn / reference[0])))) - (va_degree - reference[1])
        dva = max(dva, abs((angle + 180.0) % 360.0 - 180.0))
    if reference is not None:
        out.update({"end_max_abs_dv_pu": dv, "end_max_abs_dva_degree": dva, "end_voltage_swing_pu": swing})

    worst = (0.0, "")
    stages = transition.stages
    stage_idx = next((idx for idx, s in enumerate(stages) if s is stage), -1)
    total_dp = 0.0
    total_rating = 0.0
    for c in machines + converters:
        want = stage.expected.get(c["name"])
        column = f"S({c['name']}_p)" if c["type"] == "synchronous_machine" else f"S({c['name']}.p)"
        if want is None or column not in result:
            continue
        if stage_idx > 0:
            prev_p = stages[stage_idx - 1].expected.get(c["name"], {}).get("active_power_mw", 0.0)
        elif c["type"] == "synchronous_machine":
            prev_p = float((c.get("operating_point") or {}).get("active_power_mw", 0.0))
        else:
            ref_p = (c.get("reference") or {}).get("active_power_mw", 0.0)
            prev_p = float(ref_p) if isinstance(ref_p, (int, float)) else 0.0
        total_dp += abs(want["active_power_mw"] - prev_p)
        total_rating += float(c["rated_power_mva"])
        p = float(result[column][last].mean()) * c["rated_power_mva"]
        if abs(p - want["active_power_mw"]) >= worst[0]:
            worst = (abs(p - want["active_power_mw"]), element_of.get(c["name"], c["name"]))
    if worst[1]:
        out["end_max_abs_dp_mw"], out["end_max_abs_dp_element"] = worst
    dp_limit = max(0.01 * (total_dp if total_dp > 0 else total_rating), 1e-3) if worst[1] else 0.0
    out["settled"] = check_v_swing <= 1e-3 and check_f_swing <= 5e-3 and (not worst[1] or worst[0] <= dp_limit)

    slack = 0.0
    sources = [c for c in conv.model["components"] if c["type"] in SOURCES]
    for c in sources:
        try:
            p = sum(result[f"V({c['bus']}.{ph})"][last] * result[f"I({c['name']}.{ph})"][last] for ph in PHASES)
        except (KeyError, TypeError):
            sources = []
            break
        slack += -float(np.mean(p)) / 1e6
    if sources:
        out["end_source_power_mw"] = slack
    return out


def measure(transition: Transition, result: Any, *, window_s: float | None = None,
            end_time_s: float | None = None, time: Any = None,
            _whole: tuple[dict[str, Any], Any] | None = None) -> dict[str, Any]:
    """What a run of a transition shows, as one row:

    - status; the largest difference of the steady state at t = 0 from the
      power flow of from_time (``max_abs_dv_pu``, ``max_abs_dva_degree``);
    - ``drift_before_change_pu``: the largest departure of a bus voltage
      magnitude from its first value before the first change;
    - the smallest and largest bus voltage (pu, with the bus), the smallest
      and largest frequency of the machines that stay connected (of the
      converters' phase-locked loops when there are none) and the largest
      currents of machines
      and converters (pu of their ratings), over the whole run;
    - the end, averaged over the last ``window_s`` (default: 0.1 s, at most
      the time after the last change): the largest difference of the bus
      voltages from the power flow of to_time (``end_max_abs_dv_pu``;
      ``end_max_abs_dva_degree`` for the angles against the first bus), of
      the active powers of machines and converters from that power flow
      (``end_max_abs_dp_mw``, with the element), the frequency, and how much
      voltage and frequency still move in that window
      (``end_voltage_swing_pu``, ``end_frequency_swing_hz``). With
      ``end_time_s`` the window ends at that time of the run and not at its
      end: several calls show how the run settles.

    time: for a transition through more than two times, the time (of the
    schedule, or its position: 1 is the second) at which to take "the end":
    the window then ends where the settling time of that time ends
    (``Transition.stage(time).settled_s``, unless end_time_s says
    otherwise), and the differences are those from the power flow of that
    time. The row then has ``time`` and ``time_s`` as well. The values over
    the whole run are the same in every such row.

    Voltage magnitudes are sqrt(va^2 + vb^2 + vc^2) at every recorded
    instant, which is the line-to-line rms value for balanced phases: after
    a switching action the decaying dc components show in it as a ripple.
    """
    row: dict[str, Any] = {"case": transition.name, "from_time": transition.from_time,
                           "to_time": transition.to_time, "status": result.status}
    conv = transition.conversion
    stage = transition.stage(time)
    init = result.summary.get("initialization") or {}
    has_w184 = any(w.get("code") == "W184" for w in result.warnings)
    if init.get("phasors") and not has_w184:
        c = conv.compare(result)
        row["max_abs_dv_pu"], row["max_abs_dva_degree"] = c["max_abs_dv_pu"], c["max_abs_dva_degree"]
    else:
        row["start_checked"] = False
        if has_w184:
            row["start_warning"] = "steady-state initialization had no sine source (W184: started from zero)"
        elif result.ok:
            row["start_warning"] = "steady-state initialization produced no phasors"
    row["warnings"] = len(result.warnings)
    t = result.time
    if not result.ok or len(t) < 2:
        return row
    f0 = float(conv.mapping.get("frequency_hz", 50.0))
    before = t < transition.change_time_s - 1e-12
    if end_time_s is not None and not t[0] <= end_time_s <= t[-1] + 1e-9:
        raise ValueError(f"end_time_s = {end_time_s:g} is not within the run (0 to {t[-1]:g} s)")
    if end_time_s is not None:
        end_s = float(end_time_s)
    else:
        end_s = float(t[-1]) if stage is transition.stage() else min(stage.settled_s, float(t[-1]))
    if time is not None:
        row["time"], row["time_s"] = stage.time, end_s
    settle = max(min(stage.settled_s, end_s) - stage.changes_end_s, 0.0)
    window = min(0.1 if window_s is None else window_s, settle) if settle > 0 else 0.0
    last = (t >= end_s - window - 1e-12) & (t <= end_s + 1e-12)
    if not last.any():
        last = t == t[t <= end_s + 1e-12][-1]
    check_window = min(1.0, 0.5 * settle) if settle > 0 else 0.0
    last_check = (t >= end_s - check_window - 1e-12) & (t <= end_s + 1e-12)
    if not last_check.any():
        last_check = last

    machines = [c for c in conv.model["components"] if c["type"] == "synchronous_machine"]
    converters = [c for c in conv.model["components"] if c["type"] == "grid_following_converter"]
    if _whole is not None:
        whole_row, stacked = _whole
    else:
        whole_row, stacked = _measure_whole_run(transition, result, conv, machines, converters, f0, before)
    row.update(whole_row)
    row.update(_measure_stage_end(
        transition, result, conv, stage, last, last_check, f0, t, conv.element_of(),
        machines, converters, stacked,
    ))
    row["events"] = len(result.summary.get("events") or [])
    return row


def run(
    transitions: list[Transition],
    *,
    jobs: int | None = None,
    out: Any = None,
    force: bool = False,
    window_s: float | None = None,
) -> TransitionRun:
    """Runs transitions in parallel with ``openkeito.run_batch`` and
    measures each (``measure``): one row per transition."""
    import openkeito

    names = [t.name for t in transitions]
    repeated = sorted({n for n in names if names.count(n) > 1})
    if repeated:
        raise ValueError(f"transitions with the same name: {', '.join(repeated)}")
    batch = openkeito.run_batch([(t.name, t.model) for t in transitions], out, jobs=jobs, force=force,
                                keep_rows=True)
    rows: list[dict[str, Any]] = []
    time_rows: list[dict[str, Any]] = []
    run_warnings: list[str] = []
    for t in transitions:
        res = batch[t.name]
        if any(w.get("code") == "W184" for w in res.warnings):
            msg = "steady-state initialization had no sine source (W184), so the run started from zero"
            if msg not in t.warnings:
                t.warnings.append(msg)
        for w in t.warnings:
            if w not in run_warnings:
                run_warnings.append(w)
        if res.ok and len(res.time) >= 2 and len(t.stages) > 1:
            conv = t.conversion
            f0 = float(conv.mapping.get("frequency_hz", 50.0))
            machines = [c for c in conv.model["components"] if c["type"] == "synchronous_machine"]
            converters = [c for c in conv.model["components"] if c["type"] == "grid_following_converter"]
            whole = _measure_whole_run(t, res, conv, machines, converters, f0, res.time < t.change_time_s - 1e-12)
            row = measure(t, res, window_s=window_s, _whole=whole)
            t_rows = [measure(t, res, window_s=window_s, time=k, _whole=whole) for k in range(1, len(t.stages))]
            t_rows.append({**row, "time": t.to_time, "time_s": float(res.time[-1])})
        else:
            row = measure(t, res, window_s=window_s)
            if len(t.stages) > 1:
                t_rows = [measure(t, res, window_s=window_s, time=k) for k in range(1, len(t.stages) + 1)]
            else:
                t_rows = [{**row, "time": t.to_time, **({"time_s": t.end_time_s} if "events" in row else {})}]
        rows.append(row)
        time_rows += t_rows
        for tr_row in t_rows:
            if tr_row.get("settled") is False:
                label = tr_row.get("time") or t.to_time
                at_s = tr_row.get("time_s", t.end_time_s)
                parts = [
                    f"voltage swing {tr_row.get('end_voltage_swing_pu', 0.0):.2e} pu",
                    f"frequency swing {tr_row.get('end_frequency_swing_hz', 0.0):.2e} Hz",
                ]
                if "end_max_abs_dp_mw" in tr_row:
                    parts.append(f"power mismatch {tr_row['end_max_abs_dp_mw']:.3g} MW")
                msg = (f"{t.name}: stage {label} did not settle by {at_s:g} s ({', '.join(parts)}); "
                       "increase settle_time_s")
                if msg not in run_warnings:
                    run_warnings.append(msg)
    return TransitionRun(transitions=transitions, batch=batch, rows=rows, time_rows=time_rows, warnings=run_warnings)


def make_transitions(
    network: Any,
    schedule: Any,
    intervals: Any,
    *,
    make: Callable[..., Transition],
    options: dict[str, Any],
    times: Any = None,
    chain: bool = False,
    check: Callable[[Any, Any], None] | None = None,
    apply: Callable[[Any, Any, Any], Any] | None = None,
    convert: Callable[..., Conversion] | None = None,
    rename_buses: bool = False,
) -> list[Transition]:
    """Builds the transitions for ``make_and_run`` without running them.
    When ``chain`` is false and ``check``/``apply``/``convert`` are given,
    converts each unique schedule time once and rates unrated machines across
    all times once."""
    if chain or times is not None:
        return [make(network, schedule, times=chain_times(schedule, times, intervals), name="chain", **options)]
    pairs = schedule_intervals(schedule, intervals)
    if check is None or apply is None or convert is None or not pairs:
        return [make(network, schedule, a, b, name=f"i{k:03d}", **options) for k, (a, b) in enumerate(pairs)]
    from .schedule import parse_time

    check(network, schedule)
    unique_times = list(dict.fromkeys(t for pair in pairs for t in pair))
    build_options, conv_options, schedule_ramp = _parse_make_options(
        schedule, unique_times, convert, options, per_pair=True)
    conversions, ratings = _convert_schedule_times(network, schedule, unique_times, apply, convert, conv_options)
    cases: list[Transition] = []
    for k, (a, b) in enumerate(pairs):
        pair_build = dict(build_options)
        if schedule_ramp:
            seconds = (parse_time(b) - parse_time(a)).total_seconds()
            pair_build["ramp_time_s"] = seconds
            if pair_build.get("switch_delay_s") is None:
                pair_build.setdefault("switch_time_s", pair_build.get("start_time_s", 1.0) + seconds)
        try:
            made = build_sequence([conversions[a], conversions[b]], times=[a, b], name=f"i{k:03d}",
                                  rename_buses=rename_buses, **pair_build)
        except ConversionError as e:
            raise ConversionError([f"{a} to {b}: {p}" for p in e.problems]) from e
        _attach_rating_warning(made, ratings)
        cases.append(made)
    return cases


def make_and_run(
    network: Any,
    schedule: Any,
    intervals: Any,
    *,
    make: Callable[..., Transition],
    jobs: int | None,
    out: Any,
    force: bool,
    window_s: float | None,
    options: dict[str, Any],
    times: Any = None,
    chain: bool = False,
    check: Callable[[Any, Any], None] | None = None,
    apply: Callable[[Any, Any, Any], Any] | None = None,
    convert: Callable[..., Conversion] | None = None,
    rename_buses: bool = False,
) -> TransitionRun:
    """The ``run_transitions`` of a converter: ``make`` the transition of
    every interval (named ``i<k>``, k its position) and run them. With
    ``chain`` (or ``times``) it is one transition, named ``chain``, through
    the times (see ``chain_times``)."""
    cases = make_transitions(
        network, schedule, intervals, make=make, options=options, times=times, chain=chain,
        check=check, apply=apply, convert=convert, rename_buses=rename_buses)
    return run(cases, jobs=jobs, out=out, force=force, window_s=window_s)
