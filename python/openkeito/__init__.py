"""OpenKeito: an open electromagnetic transient (EMT) simulator.

The models, summaries and diagnostics are those of the `openkeito` command
(see docs/model.md); this package runs the same Rust core in process.

    import openkeito
    result = openkeito.run(model)          # a dict, a JSON string or a path
    result.status                          # "completed"
    result["V(n3)"]                        # numpy array, one value per row
    result.time                            # the time_s column

Failures raise OpenKeitoError, which carries the diagnostics (code, path,
message, hint) of the summary.
"""

from __future__ import annotations

import json
import math
import os
from collections.abc import Callable, Iterable, Mapping
from pathlib import Path
from typing import Any, Union

import numpy as np

from . import _native

__all__ = [
    "BatchResult",
    "OpenKeitoError",
    "Result",
    "components",
    "run",
    "run_batch",
    "scan",
    "schema",
    "topology",
    "validate",
]

__version__: str = _native.version()

ModelLike = Union[Mapping[str, Any], str, os.PathLike]


class OpenKeitoError(Exception):
    """A model was invalid or a run failed. `summary` is the whole document,
    `diagnostics` its errors and `warnings` the warnings found beside them,
    each with code, path, message and hint."""

    def __init__(self, summary: Mapping[str, Any]):
        self.summary = dict(summary)
        self.diagnostics: list[dict[str, Any]] = list(summary.get("errors", []))
        self.warnings: list[dict[str, Any]] = list(summary.get("warnings", []))
        lines = [
            f"{d.get('code')} at {d.get('path')}: {d.get('message')} (hint: {d.get('hint')})"
            for d in self.diagnostics
        ]
        super().__init__("\n".join(lines) or "the run failed")


def _model_text(model: ModelLike) -> str:
    """JSON text of a model given as a dict, as JSON text or as a file path."""
    if isinstance(model, Mapping):
        return json.dumps(model)
    if isinstance(model, os.PathLike):
        return Path(model).read_text(encoding="utf-8")
    text = str(model)
    if text.lstrip().startswith("{"):
        return text
    return Path(text).read_text(encoding="utf-8")


def _out(out: str | os.PathLike | None) -> str | None:
    return None if out is None else os.fspath(out)


class Result:
    """The result of a run or a scan: its summary and its recorded values."""

    def __init__(self, raw: Mapping[str, Any]):
        self.summary: dict[str, Any] = json.loads(raw["summary"])
        self.code: int = int(raw["code"])
        names = list(raw.get("names", []))
        values = raw.get("values")
        self.axis: np.ndarray = raw.get("axis", np.zeros(0))
        self.columns: list[str] = names
        self._values = {n: values[k] for k, n in enumerate(names)} if values is not None else {}

    @property
    def status(self) -> str:
        return self.summary.get("status", "failed")

    @property
    def ok(self) -> bool:
        return self.status == "completed"

    @property
    def warnings(self) -> list[dict[str, Any]]:
        return list(self.summary.get("warnings", []))

    @property
    def time(self) -> np.ndarray:
        """The time_s column of a run (frequency_hz of a scan)."""
        return self.axis

    @property
    def frequency(self) -> np.ndarray:
        return self.axis

    def __getitem__(self, column: str) -> np.ndarray:
        try:
            return self._values[column]
        except KeyError:
            raise KeyError(f"no column {column!r}; columns: {', '.join(self.columns)}") from None

    def __contains__(self, column: str) -> bool:
        return column in self._values

    def to_pandas(self):
        """The values as a pandas DataFrame indexed by time (or frequency)."""
        import pandas as pd

        index_name = self.summary.get("result", {}).get("time_column") or self.summary.get(
            "result", {}
        ).get("frequency_column", "time_s")
        return pd.DataFrame(self._values, index=pd.Index(self.axis, name=index_name))

    def raise_for_status(self) -> "Result":
        if not self.ok:
            raise OpenKeitoError(self.summary)
        return self

    def __repr__(self) -> str:
        return f"<openkeito.Result {self.status}, {len(self.axis)} rows, columns {self.columns}>"


class BatchResult:
    """The result of `run_batch`: the batch summary and one Result per case."""

    def __init__(self, raw: Mapping[str, Any], names: list[str]):
        self.summary: dict[str, Any] = json.loads(raw["summary"])
        self.code: int = int(raw["code"])
        self.cases: dict[str, Result] = {
            name: Result(case) for name, case in zip(names, raw["cases"])
        }

    @property
    def status(self) -> str:
        return self.summary.get("status", "failed")

    @property
    def ok(self) -> bool:
        return self.status == "completed"

    def __getitem__(self, name: str) -> Result:
        return self.cases[name]

    def __repr__(self) -> str:
        return f"<openkeito.BatchResult {self.status}, {self.summary.get('counts')}>"


def validate(model: ModelLike) -> dict[str, Any]:
    """The document of `openkeito validate`: valid, errors, warnings."""
    return json.loads(_native.validate(_model_text(model)))


def topology(model: ModelLike) -> dict[str, Any]:
    """The document of `openkeito topology`: the connections of a model.

    `components` (each with its `terminals`: the field that names it, its
    place in the model and its nodes), `nodes`, `buses`, `controls` (the
    blocks with their inputs and outputs), `signals` (which block gives a
    signal and which blocks, components and outputs read it) and
    `diagnostics` (the components, blocks, nodes and buses that each error
    and warning is about), with `valid`, `errors` and `warnings` as
    `validate` reports them. A model that can be read has its connections
    also when it is not valid; a text that cannot be read has only `errors`.
    """
    return json.loads(_native.topology(_model_text(model)))


def run(
    model: ModelLike,
    out: str | os.PathLike | None = None,
    *,
    force: bool = False,
    threads: int | None = None,
    check: bool = True,
    controller: Callable[[float, Mapping[str, float]], Mapping[str, float] | None] | None = None,
    control_period_s: float | None = None,
    control_interval_steps: int | None = None,
) -> Result:
    """Runs a model. With `out`, also writes result.csv, events.csv and
    summary.json there, as `openkeito run`. `threads` overrides
    simulation.threads. Raises OpenKeitoError unless the run completed (with
    check=False, returns the failed Result instead).

    When `controller` is given, it is called as `controller(t, state)` at
    t = 0 and every `control_interval_steps` steps (or `control_period_s`,
    defaulting to 1 step), receiving a mapping of control signal names and
    output column names to floats, and returning a mapping of signal overrides
    (or None / {} to hold the current zero-order-held values)."""
    if controller is None:
        if control_period_s is not None or control_interval_steps is not None:
            raise ValueError("control_period_s and control_interval_steps require a controller")
        result = Result(_native.run(_model_text(model), _out(out), force, threads))
        return result.raise_for_status() if check else result

    if not callable(controller):
        raise ValueError("controller must be callable")
    if control_period_s is not None and control_interval_steps is not None:
        raise ValueError("pass at most one of control_period_s and control_interval_steps")

    interval_steps: int | None = None
    period_s: float | None = None
    if control_interval_steps is not None:
        if isinstance(control_interval_steps, bool) or not isinstance(
            control_interval_steps, (int, np.integer)
        ):
            raise ValueError("control_interval_steps must be an integer >= 1")
        interval_steps = int(control_interval_steps)
        if interval_steps < 1:
            raise ValueError("control_interval_steps must be >= 1")
    elif control_period_s is not None:
        if isinstance(control_period_s, bool) or not isinstance(
            control_period_s, (int, float, np.integer, np.floating)
        ):
            raise ValueError("control_period_s must be a finite positive number")
        period_s = float(control_period_s)
        if not math.isfinite(period_s) or period_s <= 0.0:
            raise ValueError("control_period_s must be a finite positive number")

    result = Result(
        _native.run_with_controller(
            _model_text(model),
            controller,
            _out(out),
            force,
            threads,
            period_s,
            interval_steps,
        )
    )
    return result.raise_for_status() if check else result


def scan(
    model: ModelLike,
    out: str | os.PathLike | None = None,
    *,
    force: bool = False,
    check: bool = True,
) -> Result:
    """Computes the model's frequency_scan. Columns are those of scan.csv."""
    result = Result(_native.scan(_model_text(model), _out(out), force))
    return result.raise_for_status() if check else result


def run_batch(
    cases: Mapping[str, ModelLike] | Iterable[tuple[str, ModelLike]],
    out: str | os.PathLike | None = None,
    *,
    jobs: int | None = None,
    force: bool = False,
    keep_rows: bool = False,
) -> BatchResult:
    """Runs many models in parallel (`jobs` threads; default: the available
    parallelism). Every case is independent, so the results are the same as
    in a sequential run, bit for bit. With `out`, each valid case writes its
    result files and `model.json` into out/<name>, plus batch-summary.json and
    batch-metrics.csv. With keep_rows, every case's values are kept in memory.

    A case name is also a directory name: letters, digits, '_', '-' and '.',
    unique also when upper and lower case are not told apart. Names that are
    not, and an `out` that already holds a batch result (without force),
    raise OpenKeitoError before anything runs."""
    items = list(cases.items()) if isinstance(cases, Mapping) else list(cases)
    names = [name for name, _ in items]
    raw = _native.run_batch(
        [(name, _model_text(m)) for name, m in items], _out(out), jobs, force, keep_rows
    )
    result = BatchResult(raw, names)
    if "cases" not in result.summary:
        # The request itself was refused: nothing ran.
        raise OpenKeitoError(result.summary)
    return result


def schema(batch: bool = False, schedule: bool = False) -> dict[str, Any]:
    """JSON Schema of the model format (or of the batch or the schedule
    format)."""
    return json.loads(_native.schema(batch, schedule))


def components() -> list[dict[str, Any]]:
    """Component types with parameters, units, sign conventions, examples."""
    return json.loads(_native.components())
