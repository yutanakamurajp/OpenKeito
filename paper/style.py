"""The look of every figure, in one place.

Change STYLE (fonts, sizes, widths) when the venue is known; the figure
modules take everything from here and set nothing of their own.

Series are told apart twice: by colour and by line style and marker, so the
figures can be read in black and white. The four hues pass a check of
colour-vision-deficiency separation (adjacent pairs, deutan/protan/tritan)
on a white surface; black is kept for references (closed forms, exact
solutions) and is not one of the series hues.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import matplotlib

matplotlib.use("Agg")

import matplotlib.pyplot as plt  # noqa: E402

STYLE: dict[str, Any] = {
    # Fonts. "family" is a matplotlib font family or a list of fallbacks.
    "font_family": ["DejaVu Sans"],
    "math_fontset": "dejavusans",
    "font_size_pt": 8.0,
    "legend_font_size_pt": 7.0,
    # Widths of a figure in inches: one column and two columns of a
    # two-column journal page.
    "width_single_in": 3.5,
    "width_double_in": 7.16,
    "line_width_pt": 1.2,
    "marker_size_pt": 4.5,
    "png_dpi": 300,
}

#: Ink of references (closed forms, exact solutions, recorded bounds).
REFERENCE = "#000000"
#: Series hues, in the fixed order in which they are assigned.
COLORS = ["#0072B2", "#D55E00", "#009E73", "#AA4499"]
#: Line style and marker that go with each hue.
LINES = ["-", "--", "-.", ":"]
MARKERS = ["o", "s", "^", "D"]
GRID = "#d9d9d9"


def series(k: int, *, marker: bool = True, line: bool = True) -> dict[str, Any]:
    """Keyword arguments of the k-th series (0 to 3) for `Axes.plot`."""
    style: dict[str, Any] = {"color": COLORS[k], "linestyle": LINES[k] if line else "none"}
    if marker:
        style.update(marker=MARKERS[k], markerfacecolor="white", markeredgecolor=COLORS[k],
                     markeredgewidth=1.0)
    return style


def reference(*, dotted: bool = False) -> dict[str, Any]:
    """Keyword arguments of a reference curve."""
    return {"color": REFERENCE, "linestyle": ":" if dotted else "-", "linewidth": 0.8}


def apply() -> None:
    """Sets matplotlib's defaults from STYLE."""
    s = STYLE
    plt.rcParams.update({
        "font.family": s["font_family"],
        "mathtext.fontset": s["math_fontset"],
        "font.size": s["font_size_pt"],
        "axes.labelsize": s["font_size_pt"],
        "axes.titlesize": s["font_size_pt"],
        "xtick.labelsize": s["font_size_pt"],
        "ytick.labelsize": s["font_size_pt"],
        "legend.fontsize": s["legend_font_size_pt"],
        "legend.frameon": False,
        "legend.handlelength": 2.6,
        "lines.linewidth": s["line_width_pt"],
        "lines.markersize": s["marker_size_pt"],
        "axes.linewidth": 0.6,
        "axes.grid": True,
        "axes.axisbelow": True,
        "grid.color": GRID,
        "grid.linewidth": 0.4,
        "xtick.direction": "in",
        "ytick.direction": "in",
        "xtick.major.width": 0.6,
        "ytick.major.width": 0.6,
        "xtick.minor.width": 0.4,
        "ytick.minor.width": 0.4,
        "figure.dpi": 100,
        "savefig.dpi": s["png_dpi"],
        "pdf.fonttype": 42,
        "ps.fonttype": 42,
        "figure.constrained_layout.use": True,
    })


def figure(columns: int = 1, rows: int = 1, *, width: str = "single", height_in: float | None = None,
           **kwargs: Any):
    """A figure of the single or double column width with a grid of axes.
    Returns (fig, axes) as `plt.subplots`."""
    w = STYLE["width_double_in" if width == "double" else "width_single_in"]
    h = height_in if height_in is not None else rows * w / columns * 0.72
    return plt.subplots(rows, columns, figsize=(w, h), **kwargs)


def legend_below(ax, ncols: int = 2) -> None:
    """A legend under the axis label, for panels whose curves leave no
    empty corner."""
    ax.legend(loc="upper center", bbox_to_anchor=(0.5, -0.24), ncols=ncols, columnspacing=1.2)


def plain_log_ticks(ax, values, axis: str = "x") -> None:
    """Ticks of a logarithmic axis at the given values, written as plain
    numbers (time steps, numbers of sections)."""
    from matplotlib.ticker import NullFormatter, NullLocator

    target = ax.xaxis if axis == "x" else ax.yaxis
    target.set_ticks(list(values))
    target.set_ticklabels([f"{v:g}" for v in values])
    target.set_minor_locator(NullLocator())
    target.set_minor_formatter(NullFormatter())


def step_axis(ax, steps_us, *, ylabel: str, ticks=None) -> None:
    """Labels a logarithmic time-step axis in microseconds with plain tick labels."""
    ax.set_xlabel("Time step (µs)")
    ax.set_ylabel(ylabel)
    plain_log_ticks(ax, steps_us if ticks is None else ticks)


def panel(ax, letter: str) -> None:
    """Marks a panel "(a)", "(b)" above its upper left corner."""
    ax.set_title(f"({letter})", loc="left", pad=3)


def save(fig, out: Path, name: str) -> list[Path]:
    """Writes <name>.pdf and <name>.png without dates, so that the same
    data give the same files."""
    pdf, png = out / f"{name}.pdf", out / f"{name}.png"
    fig.savefig(pdf, metadata={"CreationDate": None, "ModDate": None, "Producer": None, "Creator": None})
    fig.savefig(png, metadata={"Software": None})
    plt.close(fig)
    return [pdf, png]
