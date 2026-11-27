"""Grid equivalents and disturbances (openkeito.gridtest, #53)."""

from __future__ import annotations

import cmath
import json
import math
from pathlib import Path

import numpy as np
import pytest

import openkeito
from openkeito.gridtest import (Fault, FrequencyRamp, FrequencyStep, GridEquivalent, PhaseJump, VoltageStep,
                                attach, fault_resistance_for)

ROOT = Path(__file__).resolve().parents[2]
DEVICE = ROOT / "examples" / "grid_test" / "converter_device.json"
F = 50.0
H = 5e-5


def open_end(end_time_s: float = 0.4) -> dict:
    """A device with nothing in it: the point of connection is an open end."""
    return {"format": "openkeito-model", "version": "0.5",
            "simulation": {"time_step_s": H, "end_time_s": end_time_s}, "components": []}


def dft(t: np.ndarray, x: np.ndarray, end_time_s: float) -> complex:
    """The rms phasor of one period ending at end_time_s (its own code, not
    that of openkeito.metrics)."""
    n = int(round(1 / (F * H)))
    i = int(round(end_time_s / H))
    tw, xw = t[i - n + 1:i + 1], x[i - n + 1:i + 1]
    return complex(math.sqrt(2) / n * np.sum(xw * np.exp(-2j * math.pi * F * tw)))


def clarke_angle(result, bus: str = "poi") -> np.ndarray:
    va, vb, vc = (result[f"V({bus}.{p})"] for p in "abc")
    alpha = (2 * va - vb - vc) / 3
    beta = (vb - vc) / math.sqrt(3)
    return np.unwrap(np.arctan2(beta, alpha))


@pytest.mark.parametrize("scr", [1.2, 2.0, 3.0, 10.0])
@pytest.mark.parametrize("x_over_r", [3.0, 10.0])
def test_the_impedance_is_that_of_the_short_circuit_ratio(scr, x_over_r):
    grid = GridEquivalent(rated_power_mva=2.0, rated_voltage_kv=6.6, scr=scr, x_over_r=x_over_r)
    z = 6.6**2 / (scr * 2.0)
    r = z / math.sqrt(1 + x_over_r**2)
    x = r * x_over_r
    assert abs(grid.impedance_ohm.real / r - 1) < 1e-12
    assert abs(grid.impedance_ohm.imag / x - 1) < 1e-12
    assert abs(grid.short_circuit_mva / (scr * 2.0) - 1) < 1e-12
    test = attach(open_end(), "poi", grid)
    rl = next(c for c in test.model["components"] if c["name"] == "grid_z")["impedance"]
    assert abs(rl["r1_ohm"] / r - 1) < 1e-12 and abs(rl["r0_ohm"] / r - 1) < 1e-12
    assert abs(rl["l1_h"] * 2 * math.pi * F / x - 1) < 1e-12
    # The frequency scan of the model sees the same impedance at the point of connection.
    model = dict(test.model, frequency_scan={"start_hz": 50.0, "stop_hz": 50.0, "points": 1, "buses": ["poi"]})
    assert abs(openkeito.scan(model)["Z1mag(poi)"][0] / z - 1) < 1e-9


def test_a_reactance_only():
    grid = GridEquivalent(2.0, 6.6, 3.0, math.inf)
    assert grid.impedance_ohm.real == 0.0
    assert abs(grid.impedance_ohm.imag / (6.6**2 / 6.0) - 1) < 1e-15
    with pytest.raises(ValueError):
        GridEquivalent(2.0, 6.6, 0.0, 10.0)


@pytest.mark.parametrize("jump", [20.0, -20.0, 40.0, -40.0])
def test_a_phase_jump_at_an_open_end(jump):
    t0 = 0.2
    grid = GridEquivalent(2.0, 6.6, 3.0, 10.0)
    result = openkeito.run(attach(open_end(), "poi", grid, [PhaseJump(t0, jump)]).model)
    t = result.time
    va = result["V(poi.a)"]
    before = dft(t, va, end_time_s=t0)
    after = dft(t, va, end_time_s=t0 + 2 / F)
    moved = math.degrees(cmath.phase(after / before))
    assert abs(moved - jump) < 1e-9
    assert abs(abs(after) / abs(before) - 1) < 1e-9
    # It takes one step: the angle is the old one at t0, the new one a step later.
    theta = clarke_angle(result)
    k = int(round(t0 / H))
    expected = theta[k - 1] + 2 * math.pi * F * H
    assert abs(theta[k] - expected) < 1e-9
    assert abs(theta[k + 1] - (expected + 2 * math.pi * F * H + math.radians(jump))) < 1e-9


def test_a_frequency_ramp_and_a_step():
    t0, rate, ramp_s = 0.1, 1.0, 0.2
    grid = GridEquivalent(2.0, 6.6, 3.0, 10.0)
    model = attach(open_end(0.5), "poi", grid, [FrequencyRamp(t0, rate, ramp_s)]).model
    result = openkeito.run(model)
    t = result.time
    f = np.gradient(clarke_angle(result), t) / (2 * math.pi)
    expected = F + rate * np.clip(t - t0, 0.0, ramp_s)
    inner = (t > 2 * H) & (t < t[-1] - 2 * H) & (np.abs(t - t0) > 2 * H) & (np.abs(t - t0 - ramp_s) > 2 * H)
    assert np.max(np.abs(f - expected)[inner]) < 1e-8
    # A step of frequency of 0.5 Hz held for 0.1 s.
    model = attach(open_end(0.5), "poi", grid, [FrequencyStep(t0, 0.5, 0.1)]).model
    result = openkeito.run(model)
    f = np.gradient(clarke_angle(result), t) / (2 * math.pi)
    expected = F + 0.5 * ((t >= t0) & (t < t0 + 0.1))
    inner &= (np.abs(t - t0 - 0.1) > 2 * H)
    assert np.max(np.abs(f - expected)[inner]) < 1e-8


def test_a_voltage_step_and_a_fault_of_a_residual_voltage():
    grid = GridEquivalent(2.0, 6.6, 3.0, 10.0)
    result = openkeito.run(attach(open_end(), "poi", grid, [VoltageStep(0.2, -0.1, 0.1)]).model)
    t, va = result.time, result["V(poi.a)"]
    base = abs(dft(t, va, end_time_s=0.2))
    assert abs(base / (6600 / math.sqrt(3)) - 1) < 1e-9
    assert abs(abs(dft(t, va, end_time_s=0.25)) / base - 0.9) < 1e-9
    assert abs(abs(dft(t, va, end_time_s=0.4)) / base - 1) < 1e-9
    # A three-phase fault that leaves 0.3 per unit at the point of connection.
    r = fault_resistance_for(grid, 0.3)
    z = grid.impedance_ohm
    assert abs(abs(r / (z + r)) - 0.3) < 1e-14
    result = openkeito.run(attach(open_end(), "poi", grid, [Fault(0.1, 0.15, residual_voltage_pu=0.3)]).model)
    v = abs(dft(result.time, result["V(poi.a)"], end_time_s=0.24)) / base
    assert abs(v - 0.3) < 1e-4


@pytest.mark.parametrize("scr", [10.0, 3.0, 1.2])
def test_a_converter_attached_stays_at_its_power_flow(scr):
    grid = GridEquivalent(2.0, 6.6, scr, 10.0)
    for disturbances in ([], [PhaseJump(0.5, 30.0)]):
        test = attach(DEVICE, "poi", grid, disturbances, end_time_s=0.5)
        assert abs(test.injection_mva - 1.6) < 1e-6
        result = openkeito.run(test.model)
        pre = result.time < 0.5
        for column, value in (("S(PV1.p)", 0.8), ("S(PV1.q)", 0.0), ("S(PV1.v_d)", 1.0)):
            assert np.max(np.abs(result[column][pre] - value)) < 5e-5, (scr, column)
        assert result.ok and not [w for w in result.warnings if w["code"] not in ("W183",)]


def test_the_source_voltage_is_that_of_the_power_flow():
    # E = V - Z I with I the current into the grid: P = 0.8 per unit at 1 per unit.
    grid = GridEquivalent(2.0, 6.6, 1.2, 10.0)
    test = attach(DEVICE, "poi", grid)
    z_pu = grid.impedance_ohm / (6.6**2 / 2.0)
    assert abs(test.source_voltage / 6600 - (1 - z_pu * 0.8)) < 1e-6
    # Given the injection, nothing is probed and the same comes out.
    given = attach(DEVICE, "poi", grid, injection_mva=1.6)
    assert abs(given.source_voltage - test.source_voltage) < 1e-6 * 6600


def test_names_that_the_device_uses_are_refused():
    device = json.loads(DEVICE.read_text(encoding="utf-8"))
    device["components"][0]["name"] = "grid"
    with pytest.raises(ValueError, match="grid"):
        attach(device, "poi", GridEquivalent(2.0, 6.6, 3.0, 10.0))
    with pytest.raises(ValueError, match="no bus"):
        attach(DEVICE, "b9", GridEquivalent(2.0, 6.6, 3.0, 10.0))
    with pytest.raises(ValueError, match="one time step"):
        attach(DEVICE, "poi", GridEquivalent(2.0, 6.6, 3.0, 10.0), [PhaseJump(0.0, 10.0)])


def test_a_device_of_a_format_older_than_what_is_added_is_refused():
    grid = GridEquivalent(2.0, 6.6, 3.0, 10.0)
    old = dict(open_end(), version="0.4")
    # A disturbance of the source needs format 0.5 (three_phase_controlled_source and its blocks).
    with pytest.raises(ValueError, match=r"version '0\.4'.*format 0\.5"):
        attach(old, "poi", grid, [PhaseJump(0.2, 10.0)])
    # A fault alone needs 0.3 (three_phase_source, coupled_rl, fault), which 0.4 has.
    test = attach(old, "poi", grid, [Fault(0.2, 0.1)])
    assert test.model["version"] == "0.4"
    assert openkeito.validate(test.model)["errors"] == []
    with pytest.raises(ValueError, match=r"version '0\.2'.*format 0\.3"):
        attach(dict(open_end(), version="0.2"), "poi", grid, [Fault(0.2, 0.1)])
    # 0.5 is enough for the disturbances, and the version is kept.
    test = attach(open_end(), "poi", grid, [PhaseJump(0.2, 10.0), FrequencyRamp(0.25, 1.0, 0.05)])
    assert test.model["version"] == "0.5"
    assert openkeito.validate(test.model)["errors"] == []
