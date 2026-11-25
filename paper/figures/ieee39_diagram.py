"""Single-line diagram of the IEEE 39-bus New England test system (F14).

Uses `pandapower.networks.case39()`, `openkeito.pandapower.from_pandapower`, and
`openkeito.layout.layout_from_pandapower` together with the deterministic bus
coordinates bundled in `net.bus["geo"]`.

Highlights the generator buses (30-39), the IBR location at bus 30, and the two
verification fault buses (bus 16 and bus 2) using the colour-vision-deficiency-
checked palette and markers from `style.py`.
"""

from __future__ import annotations

import json
import math
from collections import defaultdict

import matplotlib.patches as mpatches

import lib
import style

TITLE = "IEEE 39-bus New England single-line diagram (generator buses 30-39, IBR at bus 30, fault points at buses 16 and 2)"

IBR_BUS = 30
FAULT_BUSES = {2: "3LG fault", 16: "SLG fault"}

# Deterministic label offsets (dx, dy) in data coordinates to keep all 39 bus
# numbers legible without overlapping lines, markers, or neighbouring labels.
LABEL_OFFSETS: dict[int, tuple[float, float]] = {
    1: (0.0, -0.34),
    2: (0.82, 0.28),
    3: (-0.32, 0.22),
    4: (-0.30, 0.24),
    5: (0.05, -0.34),
    6: (0.28, 0.24),
    7: (0.30, -0.10),
    8: (0.30, -0.22),
    9: (0.30, -0.18),
    10: (-0.30, 0.22),
    11: (0.30, 0.14),
    12: (0.05, -0.34),
    13: (-0.30, 0.22),
    14: (-0.26, 0.26),
    15: (0.05, -0.34),
    16: (0.85, -0.38),
    17: (-0.30, -0.22),
    18: (-0.28, -0.26),
    19: (-0.18, -0.34),
    20: (-0.10, -0.36),
    21: (0.30, 0.14),
    22: (0.32, -0.05),
    23: (-0.32, -0.12),
    24: (-0.34, 0.08),
    25: (0.18, 0.28),
    26: (-0.22, 0.28),
    27: (-0.30, 0.16),
    28: (-0.32, 0.14),
    29: (0.28, 0.16),
    30: (0.0, -0.40),
    31: (0.15, 0.34),
    32: (0.32, 0.18),
    33: (-0.32, 0.22),
    34: (-0.32, 0.20),
    35: (0.30, 0.18),
    36: (-0.32, 0.20),
    37: (0.32, -0.18),
    38: (-0.34, -0.20),
    39: (0.32, -0.20),
}


def _bus_coords(net) -> dict[int, tuple[float, float]]:
    """Extracts deterministic (x, y) coordinates from pandapower `net.bus['geo']`."""
    coords: dict[int, tuple[float, float]] = {}
    for idx in sorted(net.bus.index):
        bus_num = int(net.bus.at[idx, "name"])
        raw = net.bus.at[idx, "geo"]
        geom = json.loads(raw) if isinstance(raw, str) else raw
        x, y = float(geom["coordinates"][0]), float(geom["coordinates"][1])
        if not (math.isfinite(x) and math.isfinite(y)):
            raise ValueError(f"Non-finite bus coordinate for bus {bus_num}")
        coords[bus_num] = (x, y)
    return coords


def _draw_transformer_symbol(ax, p1: tuple[float, float], p2: tuple[float, float], radius: float = 0.14) -> None:
    """Draws the midpoint two-circle transformer glyph along segment p1-p2."""
    x1, y1 = p1
    x2, y2 = p2
    dx, dy = x2 - x1, y2 - y1
    length = math.hypot(dx, dy)
    if length <= 1e-12:
        return
    ux, uy = dx / length, dy / length
    mx, my = 0.5 * (x1 + x2), 0.5 * (y1 + y2)
    shift = 0.55 * radius
    for sign in (-1.0, 1.0):
        circ = mpatches.Circle(
            (mx + sign * shift * ux, my + sign * shift * uy),
            radius,
            facecolor="white",
            edgecolor="#333333",
            linewidth=0.75,
            zorder=3,
        )
        ax.add_patch(circ)


def make(ctx: lib.Context) -> dict:
    try:
        import pandapower.networks as ppn

        from openkeito.layout import layout_from_pandapower
        from openkeito.pandapower import from_pandapower
    except ImportError:
        return {"skipped": "pandapower is not installed"}

    net = ppn.case39()
    conv = from_pandapower(net)
    layout = layout_from_pandapower(net, conv)
    coords = _bus_coords(net)
    rails = layout["faces"]["circuit"]["rails"]

    idx_to_num = {int(idx): int(net.bus.at[idx, "name"]) for idx in net.bus.index}
    num_to_idx = {num: idx for idx, num in idx_to_num.items()}

    gen_buses = {idx_to_num[int(b)] for b in net.gen["bus"]}
    slack_buses = {idx_to_num[int(b)] for b in net.ext_grid["bus"]}
    load_buses = {idx_to_num[int(b)] for b in net.load["bus"]}
    all_gen_buses = sorted(gen_buses | slack_buses)

    # Branch records (deterministic order by table and index).
    branch_rows = []
    seen_pairs: dict[tuple[int, int], int] = defaultdict(int)
    line_segments: list[tuple[tuple[float, float], tuple[float, float], int]] = []
    for line_idx in sorted(net.line.index):
        fb = idx_to_num[int(net.line.at[line_idx, "from_bus"])]
        tb = idx_to_num[int(net.line.at[line_idx, "to_bus"])]
        pair = (min(fb, tb), max(fb, tb))
        mult = seen_pairs[pair]
        seen_pairs[pair] += 1
        line_segments.append((coords[fb], coords[tb], mult))
        branch_rows.append(("line", int(line_idx), fb, tb, bool(net.line.at[line_idx, "in_service"])))

    trafo_segments: list[tuple[tuple[float, float], tuple[float, float]]] = []
    for trafo_idx in sorted(net.trafo.index):
        hb = idx_to_num[int(net.trafo.at[trafo_idx, "hv_bus"])]
        lb = idx_to_num[int(net.trafo.at[trafo_idx, "lv_bus"])]
        trafo_segments.append((coords[hb], coords[lb]))
        branch_rows.append(("transformer", int(trafo_idx), hb, lb, bool(net.trafo.at[trafo_idx, "in_service"])))

    lib.write_csv(
        ctx,
        "ieee39_diagram_branches.csv",
        ["kind", "pandapower_index", "from_bus", "to_bus", "in_service"],
        branch_rows,
    )

    # Bus records (1..39).
    bus_rows = []
    for bus_num in sorted(coords):
        idx = num_to_idx[bus_num]
        model_bus = conv.mapping["buses"][str(idx)]
        layout_dx = float(rails[f"bus:{model_bus}"]["dx"])
        x, y = coords[bus_num]
        is_gen = bus_num in gen_buses
        is_slack = bus_num in slack_buses
        is_ibr = bus_num == IBR_BUS
        is_load = bus_num in load_buses
        fault_kind = FAULT_BUSES.get(bus_num, "")
        if is_ibr:
            role = "ibr_bus"
        elif is_slack:
            role = "slack_generator_bus"
        elif is_gen:
            role = "generator_bus"
        elif is_load:
            role = "load_bus"
        else:
            role = "transmission_bus"
        bus_rows.append((
            bus_num,
            model_bus,
            role,
            float(net.bus.at[idx, "vn_kv"]),
            is_load,
            is_gen or is_slack,
            is_slack,
            is_ibr,
            fault_kind,
            x,
            y,
            layout_dx,
        ))

    lib.write_csv(
        ctx,
        "ieee39_diagram_buses.csv",
        [
            "bus",
            "model_bus",
            "role",
            "vn_kv",
            "has_load",
            "has_generator",
            "is_slack",
            "is_ibr",
            "fault_case",
            "x",
            "y",
            "layout_dx",
        ],
        bus_rows,
    )

    # Plot single-line diagram.
    fig, ax = style.figure(1, 1, width="double", height_in=5.1)

    for (x1, y1), (x2, y2), mult in line_segments:
        if mult == 0:
            ax.plot([x1, x2], [y1, y2], color="#444444", linewidth=0.9, zorder=1)
        else:
            # Offset parallel lines slightly perpendicular to the branch.
            dx, dy = x2 - x1, y2 - y1
            length = max(math.hypot(dx, dy), 1e-12)
            nx_vec, ny_vec = -dy / length * 0.08 * mult, dx / length * 0.08 * mult
            ax.plot([x1 + nx_vec, x2 + nx_vec], [y1 + ny_vec, y2 + ny_vec], color="#444444", linewidth=0.9, zorder=1)

    for p1, p2 in trafo_segments:
        ax.plot([p1[0], p2[0]], [p1[1], p2[1]], color="#222222", linestyle="--", linewidth=0.9, zorder=2)
        _draw_transformer_symbol(ax, p1, p2)

    # Group buses for CVD-safe markers & colors.
    tx_buses = [b for b in sorted(coords) if b not in all_gen_buses and b not in FAULT_BUSES]
    sync_buses = [b for b in all_gen_buses if b != IBR_BUS]
    ibr_buses = [IBR_BUS]
    fault_bus_list = sorted(FAULT_BUSES)

    def _scatter(buses: list[int], color_idx: int, label: str, size: float = 52.0, zorder: int = 4) -> None:
        xs = [coords[b][0] for b in buses]
        ys = [coords[b][1] for b in buses]
        ax.scatter(
            xs,
            ys,
            s=size,
            marker=style.MARKERS[color_idx],
            facecolor="white" if color_idx == 0 else style.COLORS[color_idx],
            edgecolor=style.COLORS[color_idx],
            linewidths=1.2,
            label=label,
            zorder=zorder,
        )

    _scatter(tx_buses, 0, "Transmission / load bus (1-29)", size=40.0, zorder=4)
    _scatter(sync_buses, 1, "Synchronous generator / slack bus (31-39)", size=58.0, zorder=5)
    _scatter(ibr_buses, 2, "IBR / PV plant location (bus 30)", size=76.0, zorder=6)
    _scatter(fault_bus_list, 3, "Verification fault bus (bus 2: 3LG, bus 16: SLG)", size=74.0, zorder=6)

    # Small legend proxies for lines & transformers.
    ax.plot([], [], color="#444444", linewidth=0.9, label=f"Transmission line ({len(line_segments)})")
    ax.plot([], [], color="#222222", linestyle="--", linewidth=0.9, label=f"Transformer ({len(trafo_segments)})")

    for bus_num, (x, y) in sorted(coords.items()):
        dx, dy = LABEL_OFFSETS.get(bus_num, (0.22, 0.16))
        if bus_num == IBR_BUS:
            text = f"{bus_num} (IBR)"
            weight = "bold"
        elif bus_num in FAULT_BUSES:
            text = f"{bus_num} ({FAULT_BUSES[bus_num]})"
            weight = "bold"
        elif bus_num == 31:
            text = f"{bus_num} (Slack)"
            weight = "bold"
        else:
            text = str(bus_num)
            weight = "normal"
        ax.annotate(
            text,
            xy=(x, y),
            xytext=(x + dx, y + dy),
            ha="center",
            va="center",
            fontsize=6.8 if bus_num not in FAULT_BUSES and bus_num != IBR_BUS else 7.2,
            fontweight=weight,
            color="#111111",
            zorder=7,
        )

    xs_all = [p[0] for p in coords.values()]
    ys_all = [p[1] for p in coords.values()]
    ax.set_xlim(min(xs_all) - 1.1, max(xs_all) + 1.0)
    ax.set_ylim(min(ys_all) - 0.85, max(ys_all) + 0.85)
    ax.set_aspect("equal")
    ax.set_axis_off()
    ax.legend(loc="upper center", bbox_to_anchor=(0.5, -0.02), ncols=3, columnspacing=1.2)

    lib.save_figure(ctx, fig, "ieee39_diagram")

    return {
        "buses": len(coords),
        "lines": len(line_segments),
        "transformers": len(trafo_segments),
        "loads": len(load_buses),
        "generator_buses": all_gen_buses,
        "slack_bus": sorted(slack_buses)[0],
        "ibr_bus": IBR_BUS,
        "fault_buses": {str(k): v for k, v in sorted(FAULT_BUSES.items())},
        "converted_components": len(conv.model["components"]),
        "layout_rails": len(rails),
    }
