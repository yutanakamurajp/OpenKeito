"""Response metrics of waveforms: phasors and symmetrical components by the
DFT, powers, step responses, temporary overvoltages, voltage-time envelopes,
harmonics and the difference of two waveforms.

    from openkeito import metrics
    t, (va, vb, vc) = result.time, metrics.phases(result, "V(b1.{})")
    seq = metrics.sequence_components(t, va, vb, vc, 50.0, end_time_s=0.2)
    abs(seq["positive"]), abs(seq["negative"])          # rms, V

Every function takes numpy arrays: ``t`` is the time axis (uniform, as the
rows of result.csv), the values one per row. Windows of the DFT are a whole
number of periods of the fundamental, which must be a whole number of rows
(``ValueError`` otherwise): N rows ending at ``end_time_s``, the row at the
end included and the row one period earlier excluded, so that the window
holds exactly ``cycles`` periods. During a transient such a phasor is the
average over the window, not the instantaneous value. Phasors are rms with
the cosine reference at t = 0, as in a power flow and in the phasors of
``three_phase_source`` (docs/model.md). docs/python.md lists which window
each metric uses.
"""

from __future__ import annotations

import cmath
import csv
import math
import os
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import numpy as np

__all__ = [
    "StepResponse",
    "difference",
    "dft",
    "envelope",
    "harmonics",
    "overvoltage",
    "phases",
    "power",
    "read_result",
    "sequence_components",
    "sequence_series",
    "settling_time",
    "sliding_phasor",
    "step_response",
    "window_rows",
]

A = cmath.exp(2j * math.pi / 3.0)


def read_result(path: str | os.PathLike) -> dict[str, np.ndarray]:
    """The columns of a result.csv (a file, or the directory of a run) as
    arrays, by the names of its header (``time_s`` included)."""
    path = Path(path)
    if path.is_dir():
        path = path / "result.csv"
    with open(path, newline="", encoding="utf-8") as f:
        rows = list(csv.reader(f))
    if not rows:
        raise ValueError(f"{path} is empty")
    header, body = rows[0], rows[1:]
    data = np.array([[float(x) for x in r] for r in body], dtype=float).reshape(len(body), len(header))
    return {name: data[:, k] for k, name in enumerate(header)}


def _column(result: Any, name: str) -> np.ndarray:
    return np.asarray(result[name], dtype=float)


def phases(result: Any, pattern: str) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """The three columns ``pattern.format(p)`` for p in a, b, c of a Result
    or of ``read_result`` (``"V(b1.{})"``, ``"I(PV1.{})"``)."""
    return tuple(_column(result, pattern.format(p)) for p in "abc")  # type: ignore[return-value]


def _step(t: np.ndarray) -> float:
    t = np.asarray(t, dtype=float)
    if t.ndim != 1 or len(t) < 2:
        raise ValueError("the time axis needs at least two rows")
    d = np.diff(t)
    h = float(np.median(d))
    if h <= 0 or np.max(np.abs(d - h)) > 1e-6 * h:
        raise ValueError("the time axis is not uniform")
    return h


def window_rows(t: np.ndarray, frequency_hz: float, cycles: float = 1) -> int:
    """The number of rows N of ``cycles`` periods of ``frequency_hz``; a
    window that is not a whole number of rows raises ValueError."""
    h = _step(t)
    span = cycles / float(frequency_hz)
    n = int(round(span / h))
    if n < 2 or abs(n * h - span) > 1e-6 * h:
        raise ValueError(f"{cycles} period(s) of {frequency_hz} Hz ({span:g} s) is not a whole number of rows "
                         f"of {h:g} s; choose time_step_s (times output_interval_steps) that divides it")
    return n


def _end_index(t: np.ndarray, end_time_s: float | None, n: int) -> int:
    if end_time_s is None:
        i = len(t) - 1
    else:
        h = _step(t)
        i = int(np.searchsorted(t, float(end_time_s) + 1e-6 * h, side="right")) - 1
    if i < n - 1:
        raise ValueError(f"the window of {n} rows ending at t = {t[max(i, 0)]:g} s starts before the first row")
    return i


def dft(t: np.ndarray, x: np.ndarray, frequency_hz: float, *, order: int = 1, cycles: float = 1,
        end_time_s: float | None = None) -> complex:
    """The rms phasor of harmonic ``order`` of ``x`` over the window of
    ``cycles`` periods of the fundamental ending at ``end_time_s`` (default:
    the last row): sqrt(2)/N * sum x_k exp(-j order w t_k)."""
    t = np.asarray(t, dtype=float)
    x = np.asarray(x, dtype=float)
    n = window_rows(t, frequency_hz, cycles)
    i = _end_index(t, end_time_s, n)
    tw, xw = t[i - n + 1:i + 1], x[i - n + 1:i + 1]
    w = 2.0 * math.pi * float(frequency_hz) * order
    return complex(math.sqrt(2.0) / n * np.sum(xw * np.exp(-1j * w * tw)))


def sliding_phasor(t: np.ndarray, x: np.ndarray, frequency_hz: float, *, order: int = 1,
                   cycles: float = 1) -> np.ndarray:
    """The phasor of ``dft`` for the window ending at every row (NaN for the
    rows before the first full window)."""
    t = np.asarray(t, dtype=float)
    x = np.asarray(x, dtype=float)
    n = window_rows(t, frequency_hz, cycles)
    w = 2.0 * math.pi * float(frequency_hz) * order
    z = x * np.exp(-1j * w * t)
    out = np.full(len(t), np.nan + 0j, dtype=complex)
    if len(t) >= n:
        out[n - 1:] = math.sqrt(2.0) / n * np.convolve(z, np.ones(n), mode="valid")
    return out


def _sequences(xa: Any, xb: Any, xc: Any) -> dict[str, Any]:
    return {"zero": (xa + xb + xc) / 3.0,
            "positive": (xa + A * xb + A * A * xc) / 3.0,
            "negative": (xa + A * A * xb + A * xc) / 3.0}


def sequence_components(t: np.ndarray, a: np.ndarray, b: np.ndarray, c: np.ndarray, frequency_hz: float, *,
                        cycles: float = 1, end_time_s: float | None = None) -> dict[str, complex]:
    """The zero, positive and negative sequence (rms phasors of phase a) of
    three phase quantities, from the DFT of each phase over the window of
    ``dft``."""
    return _sequences(*(dft(t, x, frequency_hz, cycles=cycles, end_time_s=end_time_s) for x in (a, b, c)))


def sequence_series(t: np.ndarray, a: np.ndarray, b: np.ndarray, c: np.ndarray, frequency_hz: float, *,
                    cycles: float = 1) -> dict[str, np.ndarray]:
    """``sequence_components`` for the window ending at every row (NaN before
    the first full window)."""
    return _sequences(*(sliding_phasor(t, x, frequency_hz, cycles=cycles) for x in (a, b, c)))


def power(va: np.ndarray, vb: np.ndarray, vc: np.ndarray, ia: np.ndarray, ib: np.ndarray,
          ic: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """Instantaneous active and reactive power of three phases:
    p = va ia + vb ib + vc ic and q = ((vb - vc) ia + (vc - va) ib + (va - vb) ic) / sqrt(3),
    in the direction of the currents (positive q: the currents lag the
    voltages). In a balanced steady state both are constant, P and Q of the
    phasors."""
    va, vb, vc, ia, ib, ic = (np.asarray(x, dtype=float) for x in (va, vb, vc, ia, ib, ic))
    p = va * ia + vb * ib + vc * ic
    q = ((vb - vc) * ia + (vc - va) * ib + (va - vb) * ic) / math.sqrt(3.0)
    return p, q


def _crossing(t: np.ndarray, y: np.ndarray, i: int, level: float) -> float:
    """The time between rows i - 1 and i at which y crosses level (linear)."""
    if i <= 0:
        return float(t[0])
    y0, y1 = y[i - 1], y[i]
    if y1 == y0:
        return float(t[i])
    return float(t[i - 1] + (level - y0) / (y1 - y0) * (t[i] - t[i - 1]))


def settling_time(t: np.ndarray, y: np.ndarray, target: float, tolerance: float, *,
                  start_time_s: float = 0.0) -> float:
    """The time after ``start_time_s`` from which y stays within
    ``tolerance`` of ``target`` until the last row (the crossing of the band
    interpolated linearly); ``math.inf`` when the last row is outside, 0 when
    no row after the start is. ``target`` must be finite and ``tolerance``
    finite and at least 0 (``ValueError`` otherwise)."""
    t = np.asarray(t, dtype=float)
    y = np.asarray(y, dtype=float)
    if not math.isfinite(float(target)):
        raise ValueError(f"the target {target!r} is not a finite number")
    if not (math.isfinite(float(tolerance)) and float(tolerance) >= 0):
        raise ValueError(f"the tolerance {tolerance!r} is not a finite number at least 0")
    k = int(np.searchsorted(t, start_time_s, side="left"))
    e = np.abs(y[k:] - target)
    if len(e) == 0:
        raise ValueError("no rows after start_time_s")
    if e[-1] > tolerance or not np.isfinite(e[-1]):
        return math.inf
    out = np.nonzero(~(e <= tolerance))[0]
    if len(out) == 0:
        return 0.0
    i = k + int(out[-1]) + 1
    tt = _crossing(t, np.abs(y - target), i, tolerance)
    return max(tt - start_time_s, 0.0)


@dataclass(frozen=True)
class StepResponse:
    """Metrics of a step response; times are counted from the step.

    ``rise_time_s`` from 10 % to 90 % of the change (``rise`` levels),
    ``delay_time_s`` to 50 %, ``settling_time_s`` until y stays within the
    band, ``overshoot`` the largest excursion beyond the final value over
    the change (0 when there is none), ``peak_time_s`` its time,
    ``steady_state_error`` final minus ``target`` (None without a target)."""

    initial: float
    final: float
    rise_time_s: float
    delay_time_s: float
    settling_time_s: float
    overshoot: float
    peak_time_s: float
    steady_state_error: float | None


def step_response(t: np.ndarray, y: np.ndarray, step_time_s: float, *, initial: float | None = None,
                  final: float | None = None, target: float | None = None, band: float = 0.02,
                  tolerance: float | None = None, rise: tuple[float, float] = (0.1, 0.9)) -> StepResponse:
    """Step-response metrics of y after a step at ``step_time_s``.

    ``initial`` defaults to the last row at or before the step, ``final`` to
    the last row. The settling band is ``band`` times the change (or the
    absolute ``tolerance``) around ``final``. The levels are crossed in the
    direction of the change; a level that is never reached gives
    ``math.inf``. The peak is refined by a parabola through the largest row
    and its neighbours."""
    t = np.asarray(t, dtype=float)
    y = np.asarray(y, dtype=float)
    k0 = int(np.searchsorted(t, step_time_s, side="right")) - 1
    if k0 < 0:
        raise ValueError("the step is before the first row")
    y0 = float(y[k0]) if initial is None else float(initial)
    y1 = float(y[-1]) if final is None else float(final)
    if not (math.isfinite(y0) and math.isfinite(y1)):
        raise ValueError(f"the initial ({y0!r}) or final ({y1!r}) value is not a finite number")
    span = y1 - y0
    if span == 0:
        raise ValueError("the response has no change (final equals initial)")
    z = (y - y0) / span
    tt, zz = t[k0:], z[k0:]

    def first(level: float) -> float:
        above = np.nonzero(zz >= level)[0]
        if len(above) == 0:
            return math.inf
        return _crossing(tt, zz, int(above[0]), level) - step_time_s

    lo, hi = rise
    t_lo, t_hi = first(lo), first(hi)
    rise_s = math.inf if math.isinf(t_lo) else t_hi - t_lo
    delay_s = first(0.5)
    tol = abs(span) * band if tolerance is None else float(tolerance)
    settle = settling_time(t, y, y1, tol, start_time_s=step_time_s)
    i = int(np.argmax(zz))
    peak, t_peak = float(zz[i]), float(tt[i])
    if 0 < i < len(zz) - 1:
        a, b, c = zz[i - 1], zz[i], zz[i + 1]
        den = a - 2.0 * b + c
        if den < 0:
            off = 0.5 * (a - c) / den
            peak = float(b - 0.25 * (a - c) * off)
            t_peak = float(tt[i] + off * (tt[i + 1] - tt[i]))
    return StepResponse(initial=y0, final=y1, rise_time_s=rise_s, delay_time_s=delay_s,
                        settling_time_s=settle, overshoot=max(peak - 1.0, 0.0), peak_time_s=t_peak - step_time_s,
                        steady_state_error=None if target is None else y1 - float(target))


def _segments_above(t: np.ndarray, y: np.ndarray, level: float) -> list[tuple[float, float]]:
    """Intervals in which y > level, with the crossings interpolated."""
    above = y > level
    out = []
    k, n = 0, len(y)
    while k < n:
        if not above[k]:
            k += 1
            continue
        j = k
        while j + 1 < n and above[j + 1]:
            j += 1
        start = _crossing(t, y, k, level) if k > 0 else float(t[0])
        end = _crossing(t, y, j + 1, level) if j + 1 < n else float(t[-1])
        out.append((start, end))
        k = j + 1
    return out


def overvoltage(t: np.ndarray, v: np.ndarray, threshold: float, *, start_time_s: float | None = None,
                end_time_s: float | None = None) -> dict[str, Any]:
    """A temporary overvoltage: the largest value of v and how long it is
    above ``threshold`` (v is a magnitude: an rms value or a sequence
    component from ``sequence_series``, in any unit). ``duration_s`` is the
    total time above, ``longest_s`` the longest interval, ``first_s`` the
    time it is first exceeded (None if never); crossings are interpolated
    linearly between rows. Rows outside [start_time_s, end_time_s] are left
    out."""
    t = np.asarray(t, dtype=float)
    v = np.asarray(v, dtype=float)
    keep = np.isfinite(v)
    if start_time_s is not None:
        keep &= t >= start_time_s
    if end_time_s is not None:
        keep &= t <= end_time_s
    t, v = t[keep], v[keep]
    if len(t) == 0:
        raise ValueError("no rows in the window")
    segs = _segments_above(t, v, threshold)
    lengths = [b - a for a, b in segs]
    return {"maximum": float(np.max(v)), "time_of_maximum_s": float(t[int(np.argmax(v))]),
            "duration_s": float(sum(lengths)), "longest_s": float(max(lengths, default=0.0)),
            "first_s": segs[0][0] if segs else None}


def envelope(t: np.ndarray, v: np.ndarray, points: Sequence[Sequence[float]], *, start_time_s: float,
             side: str = "lower", interpolation: str = "step") -> dict[str, Any]:
    """Compares v with a voltage-time envelope given as points
    (time after ``start_time_s``, level): with ``side="lower"`` v must stay
    at or above it (a ride-through curve), with ``"upper"`` at or below it.
    With ``"step"`` each level holds until the time of the next point, with
    ``"linear"`` the levels are joined by straight lines; the last level
    holds to the end. Rows before ``start_time_s`` and NaN values are not
    compared.

    ``violated`` says whether any row is outside, ``excess`` the largest
    distance outside (0 when none), ``first_s`` and ``time_of_excess_s`` the
    times of the first row outside and of the largest excess, and
    ``duration_s`` the number of rows outside times the step."""
    if side not in ("lower", "upper"):
        raise ValueError('side must be "lower" or "upper"')
    if interpolation not in ("step", "linear"):
        raise ValueError('interpolation must be "step" or "linear"')
    pts = np.asarray(points, dtype=float)
    if pts.ndim != 2 or pts.shape[1] != 2 or len(pts) == 0 or np.any(np.diff(pts[:, 0]) <= 0):
        raise ValueError("points must be (time, level) pairs with ascending times")
    t = np.asarray(t, dtype=float)
    v = np.asarray(v, dtype=float)
    h = _step(t)
    tau = t - start_time_s
    keep = (tau >= -1e-9 * h) & (tau >= pts[0, 0] - 1e-9 * h) & np.isfinite(v)
    if interpolation == "linear":
        level = np.interp(tau, pts[:, 0], pts[:, 1])
    else:
        idx = np.searchsorted(pts[:, 0], tau + 1e-9 * h, side="right") - 1
        level = pts[np.clip(idx, 0, len(pts) - 1), 1]
    gap = (level - v) if side == "lower" else (v - level)
    gap = np.where(keep, gap, -np.inf)
    out = np.nonzero(gap > 0)[0]
    if len(out) == 0:
        return {"violated": False, "excess": 0.0, "first_s": None, "time_of_excess_s": None, "duration_s": 0.0}
    k = int(np.argmax(gap))
    return {"violated": True, "excess": float(gap[k]), "first_s": float(t[out[0]]),
            "time_of_excess_s": float(t[k]), "duration_s": float(len(out) * h)}


def harmonics(t: np.ndarray, x: np.ndarray, frequency_hz: float, *, orders: Sequence[int] | None = None,
              cycles: int = 1, end_time_s: float | None = None) -> dict[str, Any]:
    """Harmonics of x by the DFT over ``cycles`` whole periods of the
    fundamental ending at ``end_time_s``: ``amplitudes`` maps each order to
    its rms value, ``thd`` is sqrt(sum of the squares of orders 2 and up) /
    the fundamental. ``orders`` default to 1 up to 50 (or the highest below
    half the rows of the window per period); interharmonics and orders not
    listed are left out of the THD."""
    if int(cycles) != cycles or cycles < 1:
        raise ValueError("cycles must be a whole number of periods")
    n = window_rows(t, frequency_hz, cycles)
    if orders is None:
        top = min(50, int((n / cycles - 1) // 2))
        orders = range(1, top + 1)
    orders = [int(o) for o in orders]
    if 1 not in orders:
        orders = [1, *orders]
    amps = {o: abs(dft(t, x, frequency_hz, order=o, cycles=cycles, end_time_s=end_time_s)) for o in orders}
    fund = amps[1]
    if fund == 0:
        raise ValueError("the fundamental is zero")
    thd = math.sqrt(sum(a * a for o, a in amps.items() if o >= 2)) / fund
    return {"fundamental": fund, "amplitudes": amps, "thd": thd}


def _stats(d: np.ndarray) -> dict[str, float]:
    return {"max_abs": float(np.max(np.abs(d))), "mean": float(np.mean(d)), "mean_abs": float(np.mean(np.abs(d))),
            "rms": float(np.sqrt(np.mean(d * d))), "rows": int(len(d))}


def difference(t: np.ndarray, x: np.ndarray, y: np.ndarray, *, t_y: np.ndarray | None = None,
               windows: Sequence[tuple[float, float]] | Mapping[str, tuple[float, float]] | None = None
               ) -> dict[str, Any]:
    """The difference x - y of two waveforms: its largest magnitude
    (``max_abs``), mean (``mean``, with its sign), mean magnitude
    (``mean_abs``) and rms (``rms``) over all rows of ``t``, and the same
    for each window (start, end) (both ends included; a mapping names the
    windows). y on another time axis ``t_y`` (a measurement, a run with
    another step) is interpolated linearly onto ``t``, which must lie
    within it."""
    t = np.asarray(t, dtype=float)
    x = np.asarray(x, dtype=float)
    y = np.asarray(y, dtype=float)
    if t_y is not None:
        t_y = np.asarray(t_y, dtype=float)
        if t[0] < t_y[0] - 1e-12 or t[-1] > t_y[-1] + 1e-12:
            raise ValueError("t lies outside the time axis of y")
        y = np.interp(t, t_y, y)
    if len(x) != len(t) or len(y) != len(t):
        raise ValueError("x and y need one value per row of t")
    d = x - y
    out: dict[str, Any] = _stats(d)
    if windows is not None:
        named = windows.items() if isinstance(windows, Mapping) else enumerate(windows)
        out["windows"] = {}
        for key, (a, b) in named:
            sel = (t >= a) & (t <= b)
            if not np.any(sel):
                raise ValueError(f"window {key} ({a}, {b}) holds no rows")
            out["windows"][key] = {"start_s": float(a), "end_s": float(b), **_stats(d[sel])}
    return out
