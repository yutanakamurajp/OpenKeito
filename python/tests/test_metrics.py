"""Response metrics (openkeito.metrics) against analytic signals (#54)."""

from __future__ import annotations

import cmath
import math
from pathlib import Path

import numpy as np
import pytest

import openkeito
from openkeito import metrics

ROOT = Path(__file__).resolve().parents[2]
F = 50.0
W = 2 * math.pi * F
A = cmath.exp(2j * math.pi / 3)


def axis(h: float, end: float) -> np.ndarray:
    return np.arange(int(round(end / h)) + 1) * h


@pytest.mark.parametrize("h", [1e-4, 3e-5])
def test_first_order_lag_rises_in_tau_ln9_and_settles_in_tau_ln50(h):
    tau, ts = 0.02, 0.1
    t = axis(h, 0.5)
    y = np.where(t >= ts, 1.0 - np.exp(-(t - ts) / tau), 0.0)
    r = metrics.step_response(t, y, ts, initial=0.0, final=1.0)
    assert abs(r.rise_time_s - tau * math.log(9)) < h
    assert abs(r.settling_time_s - tau * math.log(50)) < h
    assert abs(r.delay_time_s - tau * math.log(2)) < h
    assert r.overshoot == 0.0
    # The same with the absolute band and a target.
    r2 = metrics.step_response(t, 2.0 - y, ts, tolerance=0.02, target=1.0, final=1.0)
    assert abs(r2.settling_time_s - tau * math.log(50)) < h
    assert r2.steady_state_error == 0.0


def test_levels_never_reached_and_values_that_are_not_finite():
    t = axis(1e-4, 0.5)
    y = np.where(t >= 0.1, 0.05, 0.0)  # 5 % of the change to final = 1
    r = metrics.step_response(t, y, 0.1, initial=0.0, final=1.0)
    assert r.rise_time_s == math.inf and r.delay_time_s == math.inf  # not inf - inf = NaN
    r = metrics.step_response(t, np.where(t >= 0.1, 0.5, 0.0), 0.1, initial=0.0, final=1.0)
    assert r.rise_time_s == math.inf and r.delay_time_s == 0.0
    # A tolerance or a target that is not a finite number is an error, not a wrong index.
    for target, tolerance in ((0.0, math.nan), (0.0, -0.1), (0.0, math.inf), (math.nan, 0.1)):
        with pytest.raises(ValueError, match="not a finite number"):
            metrics.settling_time(t, y, target, tolerance, start_time_s=0.1)
    with pytest.raises(ValueError, match="not a finite number"):
        metrics.step_response(t, y, 0.1, initial=math.nan, final=1.0)


@pytest.mark.parametrize("zeta", [0.3, 0.7])
def test_second_order_overshoot(zeta):
    wn, ts, h = 2 * math.pi * 5, 0.05, 1e-4
    t = axis(h, 3.0)
    wd = wn * math.sqrt(1 - zeta**2)
    tau = np.maximum(t - ts, 0.0)
    y = np.where(t >= ts, 1 - np.exp(-zeta * wn * tau) * (np.cos(wd * tau) + zeta / math.sqrt(1 - zeta**2)
                                                            * np.sin(wd * tau)), 0.0)
    r = metrics.step_response(t, y, ts, initial=0.0, final=1.0)
    expected = math.exp(-zeta * math.pi / math.sqrt(1 - zeta**2))
    assert abs(r.overshoot / expected - 1) < 1e-3
    assert abs(r.peak_time_s - math.pi / wd) < h


def test_harmonics_and_thd_of_known_amplitudes():
    h = 1 / (F * 400)
    t = axis(h, 0.2)
    parts = {1: (100.0, 0.3), 5: (5.0, 1.0), 7: (3.0, -0.5), 11: (1.5, 2.0)}
    x = sum(a * np.cos(o * W * t + p) for o, (a, p) in parts.items())
    r = metrics.harmonics(t, x, F, end_time_s=0.13)
    for o in range(1, 51):
        amp = parts.get(o, (0.0, 0.0))[0] / math.sqrt(2)
        assert abs(r["amplitudes"][o] - amp) < 1e-9, o
    thd = math.sqrt(5.0**2 + 3.0**2 + 1.5**2) / 100.0
    assert abs(r["thd"] - thd) < 1e-9
    # Over three periods, and the phase of the fundamental (cosine reference).
    r3 = metrics.harmonics(t, x, F, cycles=3)
    assert abs(r3["thd"] - thd) < 1e-9
    assert abs(metrics.dft(t, x, F) - cmath.rect(100 / math.sqrt(2), 0.3)) < 1e-9


def test_symmetrical_components_of_an_unbalance():
    h = 1 / (F * 256)
    t = axis(h, 0.1)
    v1, v2, v0 = cmath.rect(230.0, 0.2), cmath.rect(23.0, -1.1), cmath.rect(7.0, 2.5)
    xa = v1 + v2 + v0
    xb = A * A * v1 + A * v2 + v0
    xc = A * v1 + A * A * v2 + v0
    a, b, c = (np.real(math.sqrt(2) * x * np.exp(1j * W * t)) for x in (xa, xb, xc))
    s = metrics.sequence_components(t, a, b, c, F, end_time_s=0.05)
    assert abs(s["positive"] - v1) < 1e-9
    assert abs(s["negative"] - v2) < 1e-9
    assert abs(s["zero"] - v0) < 1e-9
    series = metrics.sequence_series(t, a, b, c, F)
    assert np.isnan(series["positive"][0])
    assert np.max(np.abs(series["positive"][256:] - v1)) < 1e-9
    assert np.max(np.abs(series["negative"][256:] - v2)) < 1e-9


def test_power_of_balanced_phases_is_that_of_the_phasors():
    h = 1 / (F * 200)
    t = axis(h, 0.04)
    v, i = cmath.rect(230.0, 0.1), cmath.rect(10.0, -0.4)
    vs = [np.real(math.sqrt(2) * v * k * np.exp(1j * W * t)) for k in (1, A * A, A)]
    cs = [np.real(math.sqrt(2) * i * k * np.exp(1j * W * t)) for k in (1, A * A, A)]
    p, q = metrics.power(*vs, *cs)
    s = 3 * v * i.conjugate()
    assert np.max(np.abs(p - s.real)) < 1e-9 * abs(s)
    assert np.max(np.abs(q - s.imag)) < 1e-9 * abs(s)


@pytest.mark.parametrize("h", [1e-4, 7e-5])
def test_trapezoidal_overvoltage_duration(h):
    # 1.0 -> 1.3 from 0.1 s to 0.11 s, held to 0.16 s, back to 1.0 at 0.17 s.
    t = axis(h, 0.3)
    v = np.interp(t, [0.0, 0.1, 0.11, 0.16, 0.17, 0.3], [1.0, 1.0, 1.3, 1.3, 1.0, 1.0])
    r = metrics.overvoltage(t, v, 1.1)
    assert abs(r["duration_s"] - (0.05 + 2 * (2 / 3) * 0.01)) < h
    assert abs(r["first_s"] - (0.1 + 0.01 / 3)) < h
    assert r["longest_s"] == r["duration_s"]
    assert abs(r["maximum"] - 1.3) < 1e-12
    none = metrics.overvoltage(t, v, 1.4)
    assert none["duration_s"] == 0.0 and none["first_s"] is None


def test_envelope_lower_and_upper():
    h = 1e-3
    t = axis(h, 1.0)
    v = np.where((t >= 0.2) & (t < 0.35), 0.3, 1.0)
    # Ride-through: at least 0.2 for 0.15 s, at least 0.85 after 0.15 s.
    ok = metrics.envelope(t, v, [(0.0, 0.2), (0.16, 0.85)], start_time_s=0.2)
    assert not ok["violated"] and ok["excess"] == 0.0
    bad = metrics.envelope(t, v, [(0.0, 0.2), (0.1, 0.85)], start_time_s=0.2)
    assert bad["violated"]
    assert abs(bad["excess"] - 0.55) < 1e-12
    assert abs(bad["first_s"] - 0.3) < 1e-9
    assert abs(bad["duration_s"] - 0.05) < 1.5 * h
    upper = metrics.envelope(t, v, [(0.0, 0.9)], start_time_s=0.0, side="upper")
    assert upper["violated"] and abs(upper["excess"] - 0.1) < 1e-12
    lin = metrics.envelope(t, v, [(0.0, 0.0), (1.0, 1.0)], start_time_s=0.0, interpolation="linear")
    assert lin["violated"] and abs(lin["first_s"] - 0.301) < 1e-9


def test_difference_of_two_waveforms_with_windows():
    t = axis(1e-3, 1.0)
    x = np.sin(t)
    y = x + np.where(t < 0.5, 0.01, -0.02)
    d = metrics.difference(t, x, y, windows={"before": (0.0, 0.499), "after": (0.5, 1.0)})
    assert abs(d["max_abs"] - 0.02) < 1e-15
    assert abs(d["windows"]["before"]["mean"] + 0.01) < 1e-15
    assert abs(d["windows"]["after"]["mean"] - 0.02) < 1e-15
    assert abs(d["windows"]["after"]["rms"] - 0.02) < 1e-15
    # y on a finer axis is interpolated onto t.
    tf = axis(5e-4, 1.0)
    d2 = metrics.difference(t, x, np.sin(tf), t_y=tf)
    assert d2["max_abs"] < 1e-7


def test_windows_must_be_whole_rows():
    t = axis(3e-4, 0.1)
    with pytest.raises(ValueError, match="whole number of rows"):
        metrics.dft(t, np.cos(W * t), F)


def test_every_metric_runs_on_an_example(tmp_path):
    result = openkeito.run(ROOT / "examples" / "converter_voltage_support.json", tmp_path / "run")
    for data in (result, metrics.read_result(tmp_path / "run")):
        t = result.time
        va, vb, vc = metrics.phases(data, "V(b1.{})")
        ia, ib, ic = metrics.phases(data, "I(PV1.{})")
        seq = metrics.sequence_series(t, va, vb, vc, F)
        v1 = np.abs(seq["positive"]) / (6600 / math.sqrt(3))
        assert abs(v1[-1] - 0.99) < 0.02
        assert abs(seq["negative"][-1]) < 1e-3 * abs(seq["positive"][-1])
        p, q = metrics.power(va, vb, vc, ia, ib, ic)
        # The converter delivers 1.6 MW before the fault (the currents flow into the bus).
        assert abs(p[np.searchsorted(t, 0.09)] / 1.6e6 - 1) < 1e-3
        sr = metrics.step_response(t, np.asarray(data["S(PV1.q)"]), 0.1)
        assert math.isfinite(sr.rise_time_s)
        dip = metrics.envelope(t, v1, [(0.0, 0.4), (0.2, 0.9)], start_time_s=0.1)
        assert not dip["violated"]
        assert metrics.overvoltage(t, v1, 1.05)["duration_s"] < 0.05
        assert metrics.harmonics(t, va, F, end_time_s=0.09)["thd"] < 1e-3
        assert metrics.settling_time(t, np.asarray(data["S(PV1.p)"]), 0.8, 0.02, start_time_s=0.25) < 0.25
        assert metrics.difference(t, va, va)["max_abs"] == 0.0
