"""What the figure modules share: the context of a run of make.py, CSV and
table files, runs of OpenKeito models, phasors and small exact solutions.

Nothing here depends on the clock or on random numbers: the same inputs
give the same files.
"""

from __future__ import annotations

import cmath
import json
import math
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Iterable, Sequence

import numpy as np

import openkeito

SQRT2 = math.sqrt(2.0)
METHODS = [("2s-dirk", "2S-DIRK"), ("trapezoidal", "Trapezoidal")]
EXAMPLES_DIR = Path(__file__).resolve().parent.parent / "examples"


@dataclass
class Context:
    """One run of make.py."""

    out: Path
    #: Small settings, for the test: fewer step sizes, shorter runs,
    #: smaller networks. The numbers then differ from the full run.
    quick: bool = False
    #: Measure run times on this machine too (written to *_measured.csv).
    measure: bool = False
    files: list[str] = field(default_factory=list)

    def path(self, name: str) -> Path:
        self.files.append(name)
        return self.out / name


def fmt(value: Any) -> str:
    """Text of a CSV cell. Floats are written with their shortest exact
    representation."""
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (float, np.floating)):
        return repr(float(value))
    if isinstance(value, (int, np.integer)):
        return str(int(value))
    text = "" if value is None else str(value)
    if any(c in text for c in ',"\n'):
        text = '"' + text.replace('"', '""') + '"'
    return text


def write_csv(ctx: Context, name: str, header: Sequence[str], rows: Iterable[Sequence[Any]]) -> Path:
    path = ctx.path(name)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(",".join(header) + "\n")
        for row in rows:
            f.write(",".join(fmt(v) for v in row) + "\n")
    return path


def tex_escape(text: str) -> str:
    for a, b in (("\\", r"\textbackslash{}"), ("&", r"\&"), ("%", r"\%"), ("#", r"\#"), ("_", r"\_"),
                 ("{", r"\{"), ("}", r"\}"), ("^", r"\^{}"), ("~", r"\~{}"), ("$", r"\$")):
        if a == "\\":
            text = text.replace(a, "\0")
        else:
            text = text.replace(a, b)
    return text.replace("\0", r"\textbackslash{}")


def sci(value: float, digits: int = 1) -> str:
    """A number as 1.5e-06 -> '1.5e-6' for tables."""
    if value == 0:
        return "0"
    mantissa, exponent = f"{value:.{digits}e}".split("e")
    return f"{mantissa}e{int(exponent)}"


def sig(value: float, digits: int = 3) -> float:
    """Rounds `value` to `digits` significant figures."""
    return float(f"{value:.{digits}g}")


def save_figure(ctx: Context, fig: Any, name: str) -> None:
    """Saves `fig` as `<name>.pdf` and `<name>.png` and registers them in `ctx.files`."""
    import style

    ctx.files += [p.name for p in style.save(fig, ctx.out, name)]


def example(name: str) -> dict[str, Any]:
    """Loads an example model JSON from `examples/`."""
    return json.loads((EXAMPLES_DIR / name).read_text(encoding="utf-8"))


def write_table(ctx: Context, name: str, header: Sequence[str], rows: Sequence[Sequence[Any]],
                caption: str) -> None:
    """A table as <name>.tex (booktabs tabular) and <name>.md, from cells
    that are already text."""
    cells = [[str(c) for c in row] for row in rows]
    with open(ctx.path(f"{name}.tex"), "w", encoding="utf-8", newline="\n") as f:
        f.write(f"% {caption}\n")
        f.write("\\begin{tabular}{" + "l" * len(header) + "}\n\\toprule\n")
        f.write(" & ".join(tex_escape(h) for h in header) + " \\\\\n\\midrule\n")
        for row in cells:
            f.write(" & ".join(tex_escape(c) for c in row) + " \\\\\n")
        f.write("\\bottomrule\n\\end{tabular}\n")
    with open(ctx.path(f"{name}.md"), "w", encoding="utf-8", newline="\n") as f:
        f.write(f"{caption}\n\n")
        f.write("| " + " | ".join(header) + " |\n")
        f.write("|" + "---|" * len(header) + "\n")
        for row in cells:
            f.write("| " + " | ".join(c.replace("|", "\\|") for c in row) + " |\n")


def model(simulation: dict[str, Any], components: list[dict[str, Any]], outputs: list[dict[str, Any]],
          controls: list[dict[str, Any]] | None = None, version: str = "0.5") -> dict[str, Any]:
    m: dict[str, Any] = {"format": "openkeito-model", "version": version, "simulation": simulation,
                         "components": components, "outputs": outputs}
    if controls:
        m["controls"] = controls
    return m


def run(m: dict[str, Any]) -> openkeito.Result:
    """Runs a model and requires that it completed (AGENTS.md: the summary
    decides, not the exit code)."""
    result = openkeito.run(m)
    if result.status != "completed":
        raise RuntimeError(f"run failed with status {result.status!r}: {result.summary}")
    return result


def order(errors: Sequence[float]) -> list[float]:
    """Observed orders log2(e_k / e_k+1) of errors at steps h, h/2, ..."""
    return [math.log2(a / b) if a > 0 and b > 0 else float("nan") for a, b in zip(errors, errors[1:])]


def phasor(t: np.ndarray, x: np.ndarray, frequency_hz: float, periods: int = 1) -> complex:
    """Rms phasor (cosine reference) of the last whole `periods` of x(t),
    by a discrete Fourier transform over the rows of that window."""
    h = float(t[1] - t[0])
    n = int(round(periods / (frequency_hz * h)))
    tw, xw = t[-n:], x[-n:]
    return complex(np.sum(xw * np.exp(-2j * math.pi * frequency_hz * tw)) * 2.0 / n / SQRT2)


def fitted_phasor(t: np.ndarray, x: np.ndarray, frequency_hz: float, periods: float = 1.0) -> complex:
    """Rms phasor (cosine reference) of x(t) by a least-squares fit of
    a cos(wt) + b sin(wt) to the rows of the last `periods`. Unlike the
    Fourier sum, it is exact for a sinusoid whose period is not a whole
    number of time steps (60 Hz at 50 us)."""
    keep = t >= t[-1] - periods / frequency_hz - 1e-12
    w = 2.0 * math.pi * frequency_hz
    basis = np.column_stack([np.cos(w * t[keep]), np.sin(w * t[keep])])
    (a, b), *_ = np.linalg.lstsq(basis, x[keep], rcond=None)
    return complex(a, -b) / SQRT2


def polar(magnitude: float, degrees: float) -> complex:
    return cmath.rect(magnitude, math.radians(degrees))


# Exact solution of x' = A x + b with constant A and b -------------------------


def propagate(a: np.ndarray, b: np.ndarray, x0: np.ndarray, t: float) -> np.ndarray:
    """x(t) of x' = A x + b from x(0) = x0, for a 2x2 matrix A with
    non-zero determinant, through the eigenvalues of A (which stays finite
    for stiff systems, where a power series would not)."""
    xss = -np.linalg.solve(a, b)
    return xss + expm2(a, t) @ (x0 - xss)


def expm2(a: np.ndarray, t: float) -> np.ndarray:
    """e^{A t} of a 2x2 matrix: p I + q (A - s I) with s the mean of the
    eigenvalues (Sylvester's formula)."""
    s = 0.5 * (a[0, 0] + a[1, 1])
    det = a[0, 0] * a[1, 1] - a[0, 1] * a[1, 0]
    disc = s * s - det
    shifted = a - s * np.eye(2)
    if disc > 1e-12 * s * s:
        r = math.sqrt(disc)
        e1, e2 = math.exp((s + r) * t), math.exp((s - r) * t)
        p, q = 0.5 * (e1 + e2), (e1 - e2) / (2.0 * r)
    elif disc < -1e-12 * s * s:
        w = math.sqrt(-disc)
        e = math.exp(s * t)
        p, q = e * math.cos(w * t), e * math.sin(w * t) / w
    else:
        e = math.exp(s * t)
        p, q = e, e * t
    return p * np.eye(2) + q * shifted
