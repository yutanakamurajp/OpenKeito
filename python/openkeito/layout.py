"""Sidecar layout helpers (``openkeito-layout`` 0.1) for OpenKeito diagrams.

Generates deterministic sidecar documents from bus coordinates or from a
pandapower network's geodata without modifying the model format or converter
output.
"""

from __future__ import annotations

import json
import math
from collections import defaultdict
from collections.abc import Mapping, Sequence
from typing import Any

LAYOUT_FORMAT = "openkeito-layout"
LAYOUT_VERSION = "0.1"

__all__ = [
    "LAYOUT_FORMAT",
    "LAYOUT_VERSION",
    "empty_layout",
    "layout_from_bus_coordinates",
    "layout_from_pandapower",
    "serialize_layout",
]


def empty_layout() -> dict[str, Any]:
    """An empty ``openkeito-layout`` 0.1 sidecar document."""
    return {
        "format": LAYOUT_FORMAT,
        "version": LAYOUT_VERSION,
        "faces": {
            "circuit": {"rails": {}, "items": {}},
            "controls": {"rails": {}, "items": {}},
        },
    }


def _finite_float(value: Any) -> float | None:
    try:
        num = float(value)
    except (TypeError, ValueError):
        return None
    return num if math.isfinite(num) else None


def _extract_x(coord: Any) -> float | None:
    if isinstance(coord, Mapping):
        if "coordinates" in coord:
            coords = coord["coordinates"]
            if isinstance(coords, Sequence) and not isinstance(coords, (str, bytes)) and len(coords) >= 1:
                return _finite_float(coords[0])
            return None
        if "dx" in coord:
            return _finite_float(coord["dx"])
        if "x" in coord:
            return _finite_float(coord["x"])
        return None
    if isinstance(coord, Sequence) and not isinstance(coord, (str, bytes)):
        return _finite_float(coord[0]) if len(coord) >= 1 else None
    return _finite_float(coord)


def _extract_geojson_x(raw: Any) -> float | None:
    if raw is None or (isinstance(raw, float) and not math.isfinite(raw)):
        return None
    if isinstance(raw, str):
        text = raw.strip()
        if not text:
            return None
        try:
            raw = json.loads(text)
        except ValueError:
            return None
    return _extract_x(raw)


def _rail_id(bus_name: str) -> str:
    return bus_name if bus_name.startswith("bus:") else f"bus:{bus_name}"


def layout_from_bus_coordinates(
    bus_coords: Mapping[str, float | Sequence[float] | Mapping[str, Any]],
    *,
    scale: float = 1.0,
) -> dict[str, Any]:
    """Builds an ``openkeito-layout`` 0.1 sidecar from bus x (or (x, y)) coordinates.

    Maps each finite coordinate to a relative ``dx = (x - min_x) * scale``
    under ``faces.circuit.rails["bus:<name>"]``. Returns an empty layout when
    ``bus_coords`` has no finite coordinates.
    """
    scale_f = _finite_float(scale)
    if scale_f is None:
        raise ValueError("scale must be a finite number")

    xs: dict[str, float] = {}
    for bus_name, coord in bus_coords.items():
        x = _extract_x(coord)
        if x is not None:
            xs[_rail_id(str(bus_name))] = x

    out = empty_layout()
    if not xs:
        return out

    min_x = min(xs.values())
    rails: dict[str, dict[str, float]] = {}
    for rail_id in sorted(xs):
        dx = (xs[rail_id] - min_x) * scale_f
        rails[rail_id] = {"dx": 0.0 if dx == 0.0 else dx}
    out["faces"]["circuit"]["rails"] = rails
    return out


def _table_get(obj: Any, name: str) -> Any:
    if isinstance(obj, Mapping):
        return obj.get(name)
    return getattr(obj, name, None)


def _lookup_bus_x(table: Any, key: str, column: str, extractor: Any) -> float | None:
    if table is None or not hasattr(table, "columns") or column not in table.columns:
        return None
    index = table.index
    idx: Any = None
    if key in index:
        idx = key
    else:
        try:
            int_key = int(key)
        except ValueError:
            int_key = None
        if int_key is not None and int_key in index:
            idx = int_key
    if idx is None:
        return None
    return extractor(table.at[idx, column])


def layout_from_pandapower(
    net: Any,
    conv: Any,
    *,
    width: float = 1200.0,
) -> dict[str, Any]:
    """Builds an ``openkeito-layout`` 0.1 sidecar from a pandapower network.

    Reads ``net.bus["geo"]`` (GeoJSON Point) or ``net.bus_geodata`` (``x``
    column), maps pandapower bus indices through ``conv.mapping["buses"]`` to
    model bus names ``bus:<name>``, and scales their horizontal span to
    ``width``. Returns an empty layout when no bus coordinates are present.
    """
    width_f = _finite_float(width)
    if width_f is None or width_f <= 0.0:
        raise ValueError("width must be positive and finite")

    mapping = conv.mapping if hasattr(conv, "mapping") else conv
    buses = mapping.get("buses", {}) if isinstance(mapping, Mapping) else {}
    if not buses:
        return empty_layout()

    bus_table = _table_get(net, "bus")
    bus_geodata = _table_get(net, "bus_geodata")

    grouped: dict[str, list[float]] = defaultdict(list)
    for bus_key, model_bus in buses.items():
        x = _lookup_bus_x(bus_table, str(bus_key), "geo", _extract_geojson_x)
        if x is None:
            x = _lookup_bus_x(bus_geodata, str(bus_key), "x", _finite_float)
        if x is not None:
            grouped[str(model_bus)].append(x)

    if not grouped:
        return empty_layout()

    averaged = {name: sum(vals) / len(vals) for name, vals in grouped.items()}
    span = max(averaged.values()) - min(averaged.values())
    scale = (width_f / span) if span > 0.0 else 1.0
    return layout_from_bus_coordinates(averaged, scale=scale)


def serialize_layout(layout: Mapping[str, Any]) -> str:
    """Serializes a sidecar layout document as deterministically sorted JSON."""
    return json.dumps(layout, indent=2, sort_keys=True) + "\n"
