"""The MCP tool run_transitions (see test_transition.py for the transitions)."""

from __future__ import annotations

import pytest

pytest.importorskip("mcp")
pytest.importorskip("pandapower")

import pandapower.networks as pn  # noqa: E402

from test_transition import TIMES, opf_plan  # noqa: E402


def test_mcp_run_transitions(tmp_path):
    import asyncio
    import json

    from mcp import Client

    from openkeito.mcp_server import server

    def call(arguments):
        async def go():
            async with Client(server()) as c:
                return await c.call_tool("run_transitions", arguments)

        r = asyncio.run(go())
        text = r.content[0].text
        return r.is_error, (text if r.is_error else json.loads(text))

    plan = opf_plan(pn.case9())
    plan.save(tmp_path / "plan.json")
    arguments = {"source": "pandapower", "network": "case9", "schedule_path": str(tmp_path / "plan.json"),
                 "out": str(tmp_path / "moves"), "jobs": 2,
                 "options": {"start_time_s": 0.1, "ramp_time_s": 0.2, "settle_time_s": 0.3}}
    error, run = call(arguments)
    assert not error and run["status"] == "completed"
    assert [c["case"] for c in run["cases"]] == ["i000", "i001"]
    for case in run["cases"]:
        assert case["max_abs_dv_pu"] < 1e-9 and case["end_time_s"] == pytest.approx(0.6)
        assert "machines whose set points move: 2" in case["changes"] and "end_max_abs_dv_pu" in case
    assert any("typical machine parameters" in w for w in run["warnings"])
    assert (tmp_path / "moves" / "i001" / "summary.json").exists()
    # One interval, given by its times; a result is not replaced without force.
    error, text = call({**arguments, "intervals": [[TIMES[0], TIMES[2]]]})
    assert error and "force" in text
    error, run = call({**arguments, "intervals": [[TIMES[0], TIMES[2]]], "force": True})
    assert not error and [c["to_time"] for c in run["cases"]] == [TIMES[2]]
    # Mistakes in the call come back as messages.
    error, text = call({**arguments, "out": str(tmp_path / "x"), "options": {"ramp_time_s": "schedule"}})
    assert error and "interpolation" in text
    error, text = call({**arguments, "out": str(tmp_path / "x"), "intervals": [[TIMES[1], TIMES[0]]]})
    assert error and "forward" in text

    # One run through all three times, with a settling time for each interval.
    options = {"start_time_s": 0.1, "ramp_time_s": 0.2, "settle_time_s": [0.3, 0.5]}
    error, run = call({**arguments, "out": str(tmp_path / "chain"), "chain": True, "options": options})
    assert not error and run["status"] == "completed"
    (case,) = run["cases"]
    assert case["case"] == "chain" and (case["from_time"], case["to_time"]) == (TIMES[0], TIMES[2])
    assert case["max_abs_dv_pu"] < 1e-9 and case["end_time_s"] == pytest.approx(0.1 + 0.2 + 0.3 + 0.2 + 0.5)
    assert [at["time"] for at in case["times"]] == TIMES[1:]
    assert [at["time_s"] for at in case["times"]] == pytest.approx([0.6, 1.3])
    assert all("end_max_abs_dv_pu" in at and "end_max_abs_dp_mw" in at for at in case["times"])
    assert case["times"][1]["end_max_abs_dv_pu"] == case["end_max_abs_dv_pu"]
    assert (tmp_path / "chain" / "chain" / "summary.json").exists()
    # The times given; and what cannot be made into one chain.
    error, run = call({**arguments, "out": str(tmp_path / "two"), "times": [TIMES[0], TIMES[2]]})
    assert not error and run["cases"][0]["case"] == "chain" and "times" not in run["cases"][0]
    error, text = call({**arguments, "out": str(tmp_path / "x"), "chain": True,
                        "intervals": [[TIMES[0], TIMES[1]], [TIMES[0], TIMES[2]]]})
    assert error and "do not join into one chain" in text
    error, text = call({**arguments, "out": str(tmp_path / "x"), "chain": True,
                        "options": {"settle_time_s": [0.3]}})
    assert error and "settle_time_s has 1 entries for 2 intervals" in text
    # The stabilizer is an option, and the machines without one are named.
    error, run = call({**arguments, "out": str(tmp_path / "plain"), "intervals": [[TIMES[0], TIMES[1]]],
                       "options": {**arguments["options"], "pss": False}})
    assert not error and run["status"] == "completed"

    # P2: estimate_only returns runtime estimate without creating result files;
    # max_wall_s refuses runs that exceed the budget.
    est_out = tmp_path / "est_only"
    error, est = call({**arguments, "out": str(est_out), "chain": True, "estimate_only": True})
    assert not error and est["estimate_only"] is True
    assert est["steps"] > 0 and est["estimated_wall_s"] > 0
    assert not (est_out / "chain").exists()

    error, text = call({**arguments, "out": str(tmp_path / "budget"), "chain": True, "max_wall_s": 1e-6})
    assert error and "max_wall_s" in text and "settle_time_s" in text and "time_step_s" in text
    assert not (tmp_path / "budget" / "chain").exists()

    error, run = call({**arguments, "out": str(tmp_path / "within_budget"), "chain": True, "max_wall_s": 120.0})
    assert not error and run["status"] == "completed"
    # Short settling (0.3 s) flags settled=False and warns.
    error, short_chain = call({**arguments, "out": str(tmp_path / "short_chain"), "chain": True,
                               "options": {"start_time_s": 0.1, "ramp_time_s": 0.2, "settle_time_s": 0.3}})
    assert not error and short_chain["cases"][0]["times"][0]["settled"] is False
    assert any("did not settle" in w and "increase settle_time_s" in w for w in short_chain["warnings"])
