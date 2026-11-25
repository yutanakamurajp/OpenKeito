"""Tests for openkeito.layout sidecar generation helpers."""

from __future__ import annotations

import json
import math
from types import SimpleNamespace

import pytest

from openkeito.layout import (
    LAYOUT_FORMAT,
    LAYOUT_VERSION,
    empty_layout,
    layout_from_bus_coordinates,
    layout_from_pandapower,
    serialize_layout,
)


def test_empty_layout_and_constants():
    assert LAYOUT_FORMAT == "openkeito-layout"
    assert LAYOUT_VERSION == "0.1"
    doc = empty_layout()
    assert doc == {
        "format": "openkeito-layout",
        "version": "0.1",
        "faces": {
            "circuit": {"rails": {}, "items": {}},
            "controls": {"rails": {}, "items": {}},
        },
    }


def test_layout_from_bus_coordinates_relative_and_deterministic():
    doc = layout_from_bus_coordinates(
        {"b2": (15.0, 2.0), "bus:b0": 5.0, "b1": {"x": 10.0, "y": -3.0}, "bad": float("nan")},
        scale=10.0,
    )
    assert doc["format"] == LAYOUT_FORMAT
    assert doc["version"] == LAYOUT_VERSION
    assert list(doc["faces"]["circuit"]["rails"].keys()) == ["bus:b0", "bus:b1", "bus:b2"]
    assert doc["faces"]["circuit"]["rails"] == {
        "bus:b0": {"dx": 0.0},
        "bus:b1": {"dx": 50.0},
        "bus:b2": {"dx": 100.0},
    }
    assert doc["faces"]["controls"] == {"rails": {}, "items": {}}

    assert layout_from_bus_coordinates({}) == empty_layout()
    assert layout_from_bus_coordinates({"b0": float("inf")}) == empty_layout()

    with pytest.raises(ValueError):
        layout_from_bus_coordinates({"b0": 1.0}, scale=float("nan"))

    text = serialize_layout(doc)
    assert text.endswith("\n")
    assert json.loads(text) == doc


def test_layout_from_pandapower_with_and_without_coordinates():
    pp = pytest.importorskip("pandapower")
    pn = pytest.importorskip("pandapower.networks")

    # Network without coordinates -> empty faces.
    net_empty = pp.create_empty_network()
    pp.create_bus(net_empty, 110.0)
    pp.create_bus(net_empty, 110.0)
    conv_empty = SimpleNamespace(
        model={"version": "0.6", "components": []},
        mapping={"buses": {"0": "b0", "1": "b1"}},
    )
    assert layout_from_pandapower(net_empty, conv_empty) == empty_layout()

    # case39 has net.bus["geo"] GeoJSON coordinates -> scaled to width.
    net39 = pn.case39()
    conv39 = SimpleNamespace(
        model={"version": "0.6", "components": []},
        mapping={"buses": {str(int(b)): f"b{int(b)}" for b in net39.bus.index}},
    )
    doc39 = layout_from_pandapower(net39, conv39, width=1200.0)
    rails39 = doc39["faces"]["circuit"]["rails"]
    assert len(rails39) == len(net39.bus)
    dxs = [entry["dx"] for entry in rails39.values()]
    assert min(dxs) == pytest.approx(0.0)
    assert max(dxs) == pytest.approx(1200.0)
    assert all(math.isfinite(dx) for dx in dxs)
    assert conv39.model == {"version": "0.6", "components": []}

    # bus_geodata fallback and merged buses (closed bus-bus switch).
    pd = pytest.importorskip("pandas")
    net_geo = pp.create_empty_network()
    b0 = pp.create_bus(net_geo, 110.0)
    b1 = pp.create_bus(net_geo, 110.0)
    b2 = pp.create_bus(net_geo, 110.0)
    net_geo.bus_geodata = pd.DataFrame({"x": [0.0, 2.0, 4.0], "y": [0.0, 0.0, 0.0]}, index=[b0, b1, b2])
    conv_merged = SimpleNamespace(mapping={"buses": {"0": "b0", "1": "b0", "2": "b2"}})
    doc_geo = layout_from_pandapower(net_geo, conv_merged, width=600.0)
    assert doc_geo["faces"]["circuit"]["rails"] == {
        "bus:b0": {"dx": 0.0},
        "bus:b2": {"dx": 600.0},
    }

    with pytest.raises(ValueError):
        layout_from_pandapower(net_empty, conv_empty, width=0.0)
