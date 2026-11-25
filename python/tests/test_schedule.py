"""Schedules by themselves: the values ``Schedule`` refuses and its frames."""

from __future__ import annotations

from datetime import timezone

import pytest

from openkeito.schedule import Schedule

TIMES = ["2026-11-21T09:00:00+09:00", "2026-11-21T10:00:00+09:00"]


@pytest.mark.parametrize("bad", [float("nan"), float("inf"), -float("inf")])
def test_values_that_are_not_finite_are_refused_by_name(bad):
    with pytest.raises(ValueError) as e:
        Schedule.from_values(TIMES, {("load/0", "active_power_mw"): [1.0, 2.0],
                                     ("gen/0", "voltage_pu"): [1.0, bad]})
    message = str(e.value)
    assert "gen/0" in message and "voltage_pu" in message and TIMES[1] in message


def test_a_frame_with_a_gap_is_refused_by_name():
    pd = pytest.importorskip("pandas")
    frame = pd.DataFrame({"load/0:active_power_mw": [1.0, float("nan")]},
                         index=pd.date_range("2026-11-21", periods=2, freq="h", tz="Asia/Tokyo"))
    with pytest.raises(ValueError) as e:
        Schedule.from_frame(frame)
    assert "load/0" in str(e.value) and "2026-11-21T01:00:00+09:00" in str(e.value)


def test_a_frame_of_times_with_different_offsets_is_in_utc():
    pytest.importorskip("pandas")
    values = {("load/0", "active_power_mw"): [1.0, 2.0]}
    mixed = Schedule.from_values(["2026-11-21T09:00:00+09:00", "2026-11-21T00:30:00Z"], values)
    frame = mixed.to_frame()
    assert frame.index.tz == timezone.utc
    assert [t.isoformat() for t in frame.index] == ["2026-11-21T00:00:00+00:00", "2026-11-21T00:30:00+00:00"]
    back = Schedule.from_frame(frame)
    assert back.times == ["2026-11-21T00:00:00Z", "2026-11-21T00:30:00Z"]
    assert back.values_at(mixed.times[1]) == mixed.values_at(mixed.times[1])
    # One offset is kept.
    assert Schedule.from_frame(Schedule.from_values(TIMES, values).to_frame()).times == TIMES
