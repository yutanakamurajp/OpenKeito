"""Schedules: values of network elements over time (format openkeito-schedule 0.1).

A schedule lists times and, for each scheduled quantity of an element, one
value per time; ``openkeito schema --schedule`` gives the JSON Schema and
``openkeito validate`` checks a schedule file. The Rust validator checks
every schedule this module loads or makes; whether the targets exist in a
network is checked by the converter (``openkeito.pandapower``).

    from openkeito.schedule import Schedule
    s = Schedule.load("day.json")
    s.times                         # the times as text, ascending
    s.values_at(s.times[3])         # {(target, quantity): value}
"""

from __future__ import annotations

import json
import math
import os
import re
from collections.abc import Iterable, Mapping
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Union

from . import OpenKeitoError, _native

__all__ = ["FORMAT", "VERSION", "QUANTITIES", "Schedule", "parse_time", "format_time"]

FORMAT = "openkeito-schedule"
VERSION = "0.1"
#: Scheduled quantities; the unit is part of the name.
QUANTITIES = ("active_power_mw", "reactive_power_mvar", "voltage_pu", "tap_position", "in_service")
#: Quantities that keep the earlier value between two times, also with
#: linear interpolation.
STEPPED = ("tap_position", "in_service")

TimeLike = Union[str, datetime]

_FRACTION = re.compile(r"\.(\d+)")


def parse_time(time: TimeLike) -> datetime:
    """A schedule time (RFC 3339 text with an offset, or an aware datetime)
    as an aware datetime."""
    if isinstance(time, datetime):
        if time.tzinfo is None:
            raise ValueError(f"{time} has no offset; give it a tzinfo")
        return time
    text = str(time)
    if text[-1:] in ("Z", "z"):
        text = text[:-1] + "+00:00"
    # Python 3.10 reads 3 or 6 digits of a fraction only.
    text = _FRACTION.sub(lambda m: "." + (m.group(1) + "000000")[:6], text, count=1)
    value = datetime.fromisoformat(text.replace("t", "T", 1))
    if value.tzinfo is None:
        raise ValueError(f"{time!r} has no offset, such as +09:00 or Z")
    return value


def format_time(time: TimeLike) -> str:
    """RFC 3339 text of a time (seconds, or microseconds when there are any)."""
    value = parse_time(time)
    spec = "seconds" if value.microsecond == 0 else "microseconds"
    text = value.isoformat(timespec=spec)
    return text[:-6] + "Z" if value.utcoffset().total_seconds() == 0 and text.endswith("+00:00") else text


class Schedule:
    """A checked schedule. ``data`` is the JSON document."""

    def __init__(self, data: Mapping[str, Any]):
        data = json.loads(json.dumps(data))
        report = json.loads(_native.validate_schedule(json.dumps(data)))
        if not report["valid"]:
            raise OpenKeitoError(report)
        self.data: dict[str, Any] = data
        self._instants = [parse_time(t) for t in data["times"]]

    # Making and saving -------------------------------------------------------

    @classmethod
    def load(cls, source: Mapping[str, Any] | str | os.PathLike) -> "Schedule":
        """From a dict, JSON text or a file path."""
        if isinstance(source, Mapping):
            return cls(source)
        text = str(source)
        if not isinstance(source, os.PathLike) and text.lstrip().startswith("{"):
            return cls(json.loads(text))
        return cls(json.loads(Path(text).read_text(encoding="utf-8")))

    @classmethod
    def from_values(
        cls,
        times: Iterable[TimeLike],
        values: Mapping[tuple[str, str], Iterable[float]],
        *,
        name: str = "",
        description: str = "",
        interpolation: str = "step",
    ) -> "Schedule":
        """From the times and {(target, quantity): values}. Every value must
        be finite (no NaN, no gap): ValueError names the first that is not."""
        texts = [format_time(t) for t in times]
        series = []
        for (target, quantity), vs in values.items():
            numbers = [float(v) for v in vs]
            for k, v in enumerate(numbers):
                if not math.isfinite(v):
                    at = texts[k] if k < len(texts) else f"position {k}"
                    raise ValueError(f"{quantity} of {target!r} at {at} is {v}; a schedule needs a finite value "
                                     "at every time")
            series.append({"target": target, "quantity": quantity, "values": numbers})
        return cls({
            "format": FORMAT, "version": VERSION, "name": name, "description": description,
            "interpolation": interpolation, "times": texts, "series": series,
        })

    @classmethod
    def from_frame(cls, frame: Any, **kwargs: Any) -> "Schedule":
        """From a pandas DataFrame: index the times, columns
        "<target>:<quantity>" such as "load/3:active_power_mw"."""
        values = {}
        for column in frame.columns:
            target, sep, quantity = str(column).rpartition(":")
            if not sep:
                raise ValueError(f"column {column!r} is not '<target>:<quantity>'")
            values[(target, quantity)] = frame[column].tolist()
        times = [t.to_pydatetime() if hasattr(t, "to_pydatetime") else t for t in frame.index]
        return cls.from_values(times, values, **kwargs)

    def to_frame(self) -> Any:
        """As a pandas DataFrame (see from_frame). The index keeps the offset
        of the times when they share one; times with different offsets are
        given in UTC."""
        import pandas as pd

        instants = self._instants
        if len({t.utcoffset() for t in instants}) > 1:
            instants = [t.astimezone(timezone.utc) for t in instants]
        return pd.DataFrame(
            {f"{s['target']}:{s['quantity']}": s["values"] for s in self.series},
            index=pd.DatetimeIndex(instants, name="time"),
        )

    def save(self, path: str | os.PathLike) -> None:
        Path(path).write_text(json.dumps(self.data, indent=2) + "\n", encoding="utf-8")

    # Reading -----------------------------------------------------------------

    @property
    def name(self) -> str:
        return self.data.get("name", "")

    @property
    def times(self) -> list[str]:
        return list(self.data["times"])

    @property
    def interpolation(self) -> str:
        return self.data.get("interpolation", "step")

    @property
    def series(self) -> list[dict[str, Any]]:
        return self.data["series"]

    def index(self, time: TimeLike) -> int:
        """Position of one of the schedule's times."""
        at = parse_time(time)
        for k, t in enumerate(self._instants):
            if t == at:
                return k
        raise KeyError(f"{time} is not one of the schedule's times")

    def values_at(self, time: TimeLike) -> dict[tuple[str, str], float]:
        """{(target, quantity): value} at a time between the first and the
        last time of the schedule, by its interpolation."""
        at = parse_time(time)
        first, last = self._instants[0], self._instants[-1]
        if at < first or at > last:
            raise ValueError(f"{format_time(at)} is outside the schedule ({self.times[0]} to {self.times[-1]})")
        k = max(i for i, t in enumerate(self._instants) if t <= at)
        out = {}
        for s in self.series:
            v = s["values"][k]
            if self.interpolation == "linear" and s["quantity"] not in STEPPED and at > self._instants[k]:
                t0, t1 = self._instants[k], self._instants[k + 1]
                x = (at - t0) / (t1 - t0)
                v = v + x * (s["values"][k + 1] - v)
            out[(s["target"], s["quantity"])] = float(v)
        return out

    def __repr__(self) -> str:
        return f"<openkeito.schedule.Schedule {self.name!r}: {len(self.times)} times, {len(self.series)} series>"

