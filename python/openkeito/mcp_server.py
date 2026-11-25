"""MCP server of OpenKeito (stdio): validate, run, query, convert, schedule.

Start it with ``openkeito-mcp`` (or ``python -m openkeito.mcp_server``) and
register that command in an MCP client. Every tool returns a JSON document.
Results are written to output directories that the client names; the
tools return their summaries and paths, and ``query_results`` reads the
recorded values back in pieces.

The documents and diagnostics are those of the ``openkeito`` command and the
Python package (docs/model.md): a run is good only when its summary's status is
"completed"; then check its events and warnings.

A mistake in the model comes back as the document with its diagnostics; a
mistake in the call (a file that cannot be read, an unknown name, a file that
exists already) is a tool error whose message says what to change. No tool
replaces an existing file or result unless ``force`` is given, and no answer
is larger than the limits below: what is left out is named in the answer,
with where to read it.

The tools read and write only under one folder, the root: the working
directory in which the server starts, or ``--root <folder>``, or the
environment variable ``OPENKEITO_MCP_ROOT``. Every path that a tool is given
is taken from the root when it is relative, resolved (``..``, symbolic links,
junctions) and refused when it then lies outside the root. ``--allow-any-path``
lifts the limit; the server says so on standard error when it starts. The
Python API and the ``openkeito`` command have no such limit.
"""

from __future__ import annotations

import argparse
import csv
import inspect
import json
import math
import os
import sys
from pathlib import Path
from typing import Any, Iterator

import openkeito

__all__ = ["server", "main"]

INSTRUCTIONS = """OpenKeito is an electromagnetic transient (EMT) simulator. Models are JSON documents:
read get_schema and list_components instead of guessing field names; units are part of the
field names. Validate before running and fix every diagnostic by its hint; get_topology returns
the connections of a model (components, terminals, nodes, buses, controls, signals and
diagnostics). Give every run a new
output directory. A run is good only when the summary's status is "completed"; then check its
events and warnings before reading values with query_results. Power-flow networks (pandapower
or PowSyBl/CGMES) convert into models that start from their power flow; schedules (values of
network elements over time) and disturbances run as time slices in parallel; run_transitions
follows a schedule from one time to the next with machines and converters under control, each
interval as a case of its own or (chain=true) all times in one run."""

#: The environment variable that names the root when --root is not given.
ROOT_VARIABLE = "OPENKEITO_MCP_ROOT"

#: Names that Windows gives to devices in every folder, whatever follows the first dot.
WINDOWS_DEVICES = frozenset(["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"]
                            + [f"{port}{n}" for port in ("COM", "LPT") for n in "0123456789¹²³"])

#: Values (rows times the axis and the columns) in one answer of query_results.
MAX_VALUES = 20_000
#: Columns in one answer of query_results.
MAX_COLUMNS = 500
#: Above this many columns a summary gives the column names instead of the per-column lists.
MAX_SUMMARY_COLUMNS = 50
#: Column names in a summary.
MAX_COLUMN_NAMES = 2000
#: Cases in the answer of run_time_slices.
MAX_CASES = 200
#: Diagnostics per case that did not complete.
MAX_CASE_ERRORS = 3

#: The modules and classes of the objects that pandapower writes into a network's JSON file.
PANDAPOWER_JSON_MODULES = ("builtins", "geopandas", "networkx", "numpy", "pandapower", "pandas", "shapely")
BUILTIN_JSON_CLASSES = frozenset({"complex", "tuple", "set", "frozenset"})
PANDAS_JSON_CLASSES = frozenset({
    "DataFrame", "Series", "Index", "RangeIndex", "MultiIndex", "DatetimeIndex",
    "TimedeltaIndex", "PeriodIndex", "CategoricalIndex", "IntervalIndex", "NumericIndex",
    "Int64Index", "UInt64Index", "Float64Index",
})
NUMPY_JSON_CLASSES = frozenset({
    "array", "ndarray", "dtype", "bool", "bool_", "bytes_", "str_", "object_",
    "int8", "int16", "int32", "int64", "intp", "int_",
    "uint8", "uint16", "uint32", "uint64", "uintp", "uint",
    "float16", "float32", "float64", "float96", "float128", "longdouble",
    "complex64", "complex128", "complex192", "complex256", "clongdouble",
    "datetime64", "timedelta64", "byte", "ubyte", "short", "ushort", "intc", "uintc",
    "longlong", "ulonglong", "half", "single", "double", "csingle", "cdouble",
})
MAX_WARNINGS = 50
PICKLE_SUFFIXES = (".p", ".pkl", ".pickle")
POWSYBL_FORMATS = "CGMES, XIIDM, UCTE, IEEE-CDF, MATPOWER, PSS/E or PowerFactory"


def _tool_error(message: str) -> Exception:
    from mcp.server.mcpserver.exceptions import ToolError

    return ToolError(message)


def _reason(e: BaseException) -> str:
    """Why a file operation failed, without the path (the message names it)."""
    if isinstance(e, OSError) and e.strerror:
        return e.strerror
    return str(e) or type(e).__name__


def _failure(what: str, e: Exception) -> Exception:
    """The tool error of a failed conversion or run."""
    problems = getattr(e, "problems", None)
    if problems:
        return _tool_error("cannot convert: " + "; ".join(problems))
    text = str(e.args[0]) if isinstance(e, KeyError) and e.args else str(e)
    return _tool_error(f"{what}: {text or type(e).__name__}")


class _Paths:
    """The folder under which the tools read and write (the root), and the
    check of the paths that they are given."""

    def __init__(self, root: str | os.PathLike | None = None, allow_any_path: bool = False):
        given = root if root is not None else (os.environ.get(ROOT_VARIABLE) or os.getcwd())
        try:
            self.root = Path(given).resolve(strict=True)
            self._id = os.stat(self.root).st_ino
        except (OSError, ValueError, RuntimeError) as e:
            raise ValueError(f"the root folder {os.fspath(given)!r} cannot be used: {_reason(e)}; give an "
                             f"existing folder with --root or {ROOT_VARIABLE}") from e
        if not self.root.is_dir():
            raise ValueError(f"the root {os.fspath(given)!r} is not a folder; give an existing folder with "
                             f"--root or {ROOT_VARIABLE}")
        self.allow_any_path = allow_any_path
        self.how = (f"Put it under {self.root} and give that path (a relative path is taken from this root), "
                    f"or start openkeito-mcp with --root <folder> (or {ROOT_VARIABLE}) naming a folder that "
                    "holds it, or with --allow-any-path to lift the limit")

    def inside(self, resolved: Path) -> bool:
        """Whether a resolved path is the root or below it. The folders are
        compared as the file system knows them (so that upper and lower case,
        short names and other spellings of the root count as the root), and
        by their names only where the file system gives no identities."""
        if not self._id:
            return resolved == self.root or self.root in resolved.parents
        for folder in (resolved, *resolved.parents):
            try:
                if os.path.samefile(folder, self.root):
                    return True
            except (OSError, ValueError):
                continue
        return False

    def path(self, given: str, argument: str, *, display: str | None = None) -> Path:
        """The resolved path of an argument: taken from the root when it is
        relative, with ``..`` and links followed (also the links above a file
        or folder that does not exist yet). Outside the root it is refused.
        ``display`` names the path as the caller gave it when checking a file
        inside an argument directory."""
        shown = given if display is None else display
        try:
            resolved = (self.root / given).resolve()
        except (OSError, ValueError, RuntimeError) as e:
            raise _tool_error(f"{argument} {shown!r} is not a path that can be used: {_reason(e)}; give the "
                              f"path of a file or folder under {self.root}") from e
        if self.allow_any_path:
            return resolved
        if os.name == "nt":
            # A device name is a device in every folder: it is not a file under the root.
            device = next((part for part in resolved.parts[1:]
                           if part.split(".")[0].split(":")[0].rstrip(" ").upper() in WINDOWS_DEVICES), None)
            if device is not None:
                raise _tool_error(f"{argument} {shown!r} names the Windows device {device!r}, not a file or "
                                  f"folder under the root folder {self.root}; give another name (or start "
                                  "openkeito-mcp with --allow-any-path to lift the limit)")
        if self.inside(resolved):
            return resolved
        raise _tool_error(f"{argument} {shown!r} is outside the root folder of this server: it resolves to "
                          f"{resolved}, which is not under {self.root}. {self.how}")

    def folder(self, given: str, argument: str, depth: int = 1) -> Path:
        """The resolved path of a folder that a tool reads or writes as a
        whole: an output directory, or a network given as a folder of files.
        What it holds already (down to ``depth`` levels: the files of a run,
        or the case directories of a batch and their files) must not be links
        to outside the root, since the tool would read them, or with ``force``
        write through them."""
        directory = self.path(given, argument)
        if not self.allow_any_path:
            self._no_links_out(directory, given, argument, depth)
        return directory

    def _no_links_out(self, folder: Path, given: str, argument: str, depth: int) -> None:
        try:
            names = sorted(os.listdir(folder))
        except OSError:
            return  # not there yet, or not a directory: the run says so
        for name in names:
            entry = folder / name
            try:
                real = entry.resolve()
            except (OSError, ValueError, RuntimeError):
                real = None
            if real is None or not self.inside(real):
                raise _tool_error(f"{argument} {given!r} holds {entry}, a link to "
                                  f"{real if real is not None else 'a place that cannot be resolved'}, which is "
                                  f"not under the root folder {self.root}: the tool would read or write "
                                  f"through it. Give a folder under {self.root} without such a link (for "
                                  "results, a new directory), or start openkeito-mcp with --root <folder> "
                                  "naming a folder that holds both, or with --allow-any-path to lift the limit")
            # A link to a place under the root is not followed further.
            if depth > 1 and real == entry and entry.is_dir():
                self._no_links_out(entry, given, argument, depth - 1)


def _model(model: dict[str, Any] | None, path: str | None, paths: _Paths) -> Any:
    if (model is None) == (path is None):
        raise _tool_error("give either model (the JSON document) or path (a model file), not both")
    if model is not None:
        return model
    file = paths.path(path, "path")
    try:
        return file.read_text(encoding="utf-8")
    except UnicodeDecodeError as e:
        raise _tool_error(f"the model file {file} is not UTF-8 text; give the path of a model (a JSON "
                          "document, see get_schema)") from e
    except (OSError, ValueError) as e:
        raise _tool_error(f"cannot read the model file {file}: {_reason(e)}; give the path of a model, or "
                          "the document itself as model") from e


def _short_summary(summary: dict[str, Any], out: Path | None) -> dict[str, Any]:
    """The summary without the per-row parts, plus where the files are. For a
    model with many columns, the lists with an entry per column are left out
    (they are in summary.json) and the column names are given instead."""
    s = dict(summary)
    s.pop("all_events", None)
    omitted, more = [], ""
    for section, key in (("result", "columns"), ("result", "statistics"), ("initialization", "phasors")):
        part = s.get(section)
        items = part.get(key) if isinstance(part, dict) else None
        if not isinstance(items, list) or len(items) <= MAX_SUMMARY_COLUMNS:
            continue
        part = s[section] = {k: v for k, v in part.items() if k != key}
        omitted.append(f"{section}.{key}")
        if key == "columns":
            names = [c.get("name") if isinstance(c, dict) else c for c in items]
            part["column_count"] = len(names)
            part["column_names"] = names[:MAX_COLUMN_NAMES]
            if len(names) > MAX_COLUMN_NAMES:
                more = f" column_names holds the first {MAX_COLUMN_NAMES} of {len(names)} names."
        elif key == "phasors":
            part["phasor_count"] = len(items)
    if out is not None:
        s["output_directory"] = str(out)
    if omitted:
        s["omitted"] = omitted
        where = "summary.json"
        if out is not None:
            s["summary_path"] = where = str(out / "summary.json")
        s["note"] = (f"{', '.join(omitted)} are left out because there are more than {MAX_SUMMARY_COLUMNS} "
                     f"columns; they are in {where}. query_results gives the statistics and the values "
                     f"of chosen columns.{more}")
    return s


def _example(module: Any, name: str, package: str) -> Any:
    """The function ``name`` of ``module`` when it is a public function of
    ``package`` that needs no arguments (as the example networks are)."""
    f = None if name.startswith("_") else getattr(module, name, None)
    if not inspect.isfunction(f) or not (getattr(f, "__module__", None) or "").startswith(package):
        return None
    try:
        parameters = inspect.signature(f).parameters.values()
    except (TypeError, ValueError):
        return None
    if any(p.default is p.empty and p.kind not in (p.VAR_POSITIONAL, p.VAR_KEYWORD) for p in parameters):
        return None
    return f


def _allowed_pandapower_object(module: Any, cls: Any) -> bool:
    if not isinstance(module, str) or not isinstance(cls, str) or not cls:
        return False
    top = module.split(".")[0]
    if top not in PANDAPOWER_JSON_MODULES:
        return False
    if top == "builtins":
        return cls in BUILTIN_JSON_CLASSES
    if top == "numpy":
        return cls in NUMPY_JSON_CLASSES
    if top == "pandas":
        return cls in PANDAS_JSON_CLASSES
    return True


def _foreign_modules(value: Any, found: set[str]) -> None:
    """Collects the modules/classes that a pandapower JSON document names for
    its objects ("_module", "_class") other than the safe types pandapower
    writes itself. The objects can be inside strings that hold JSON again."""
    if isinstance(value, dict):
        if "_module" in value:
            module = value.get("_module")
            cls = value.get("_class")
            if not _allowed_pandapower_object(module, cls):
                if isinstance(module, str) and module.split(".")[0] in PANDAPOWER_JSON_MODULES:
                    found.add(f"{module}.{cls}" if isinstance(cls, str) else f"{module}.<missing>")
                else:
                    found.add(str(module))
        for v in value.values():
            _foreign_modules(v, found)
    elif isinstance(value, list):
        for v in value:
            _foreign_modules(v, found)
    elif isinstance(value, str) and value.lstrip()[:1] in ("{", "[", '"'):
        try:
            inner = json.loads(value)
        except ValueError:
            return
        _foreign_modules(inner, found)


def _pandapower_file(file: Path) -> Any:
    """A network from a JSON file written by pandapower.to_json."""
    import pandapower as pp

    how = "give a file written by pandapower.to_json(net, path)"
    try:
        document = json.loads(file.read_text(encoding="utf-8"))
    except OSError as e:
        raise _tool_error(f"cannot read the network file {file}: {_reason(e)}; {how}") from e
    except (ValueError, RecursionError) as e:
        raise _tool_error(f"the network file {file} is not JSON ({_reason(e)}); {how}") from e
    # pandapower imports the module that an object in the file names. Only
    # the safe classes of what pandapower itself writes are let through.
    foreign: set[str] = set()
    try:
        _foreign_modules(document, foreign)
    except RecursionError as e:
        raise _tool_error(f"the network file {file} is nested too deeply; {how}") from e
    if foreign:
        raise _tool_error(f"the network file {file} names objects ({', '.join(sorted(foreign))}), which are "
                          f"not loaded (only safe types of {', '.join(PANDAPOWER_JSON_MODULES)}); remove these "
                          "objects (such as controllers) from the net and save it again with pandapower.to_json")
    try:
        net = pp.from_json(str(file))
    except Exception as e:
        raise _tool_error(f"cannot read the pandapower network {file}: {_reason(e)}; {how}") from e
    if not isinstance(net, pp.pandapowerNet):
        raise _tool_error(f"the file {file} does not hold a pandapower network; {how}")
    return net


def _network(source: str, network: str, paths: _Paths) -> tuple[Any, Path | None]:
    """(a pandapower or PowSyBl network: a built-in example by name or a
    file; the file, if it is one). A name that is a path to outside the root
    is refused before anything is looked up."""
    if source == "pandapower":
        import pandapower as pp
        import pandapower.networks as examples

        suffix = Path(network).suffix.lower()
        if suffix in PICKLE_SUFFIXES:
            raise _tool_error(f"pickle files are not loaded, because loading one can run code: {network}; save "
                              "the net with pandapower.to_json(net, 'net.json') and give that file")
        if suffix == ".json":
            file = paths.path(network, "network")
            return _pandapower_file(file), file
        factory = _example(examples, network, "pandapower.networks")
        if factory is None:
            raise _tool_error(f"pandapower.networks has no example network {network!r}; give a name such as "
                              "'case9', 'case39' or 'case118', or a .json file written by pandapower.to_json")
        try:
            net = factory()
        except Exception as e:
            raise _tool_error(f"pandapower.networks.{network}() failed: {_reason(e)}; give another example such "
                              "as 'case9' or 'case39', or a .json file written by pandapower.to_json") from e
        if not isinstance(net, pp.pandapowerNet):
            raise _tool_error(f"pandapower.networks has no example network {network!r} (it does not make a "
                              "network); give a name such as 'case9', 'case39' or 'case118'")
        return net, None
    if source == "powsybl":
        import pypowsybl.network as pn

        file = paths.path(network, "network")
        if file.is_dir():
            file = paths.folder(network, "network")  # a folder of files, such as those of CGMES
        if file.exists():
            try:
                return pn.load(str(file)), file
            except Exception as e:
                raise _tool_error(f"cannot read the network file {file}: {_reason(e)}; give a file that "
                                  f"pypowsybl imports ({POWSYBL_FORMATS})") from e
        base = network[len("create_"):] if network.startswith("create_") else network
        factory = next((f for f in (_example(pn, n, "pypowsybl") for n in (f"create_{base}", f"create_{base}_network"))
                        if f is not None), None)
        unknown = (f"pypowsybl has no example network {network!r}, and there is no such file; give a name such "
                   f"as 'ieee14' or 'micro_grid_be', or a file that pypowsybl imports ({POWSYBL_FORMATS})")
        if factory is None:
            raise _tool_error(unknown)
        try:
            net = factory()
        except Exception as e:
            raise _tool_error(f"pypowsybl.network.{factory.__name__}() failed: {_reason(e)}; give another "
                              "example such as 'ieee14' or 'micro_grid_be'") from e
        if not isinstance(net, pn.Network):
            raise _tool_error(unknown)
        return net, None
    raise _tool_error("source is 'pandapower' or 'powsybl'")


def _convert(source: str, net: Any, options: dict[str, Any]) -> Any:
    if source == "pandapower":
        from .pandapower import from_pandapower

        return from_pandapower(net, **options)
    from .powsybl import from_powsybl

    return from_powsybl(net, **options)


def _short_circuit(source: str, file: Path | None, net: Any, options: dict[str, Any] | None,
                   wanted: bool) -> tuple[dict[str, Any], dict[str, Any], list[str]]:
    """(the options with the short-circuit attributes of a CGMES file, which
    pypowsybl's import leaves out, as from_cgmes has them; what was read; the
    warning about attributes left out). Other networks keep their options."""
    options = dict(options or {})
    if not (wanted and source == "powsybl" and file is not None):
        return options, {}, []
    from .powsybl import with_cgmes_short_circuit_data

    try:
        return with_cgmes_short_circuit_data(str(file), net, options)
    except Exception as e:
        raise _tool_error(f"cannot read the short-circuit attributes of {file}: {_reason(e)}; correct the "
                          "file, or give short_circuit_profile=false to convert without them") from e


def _schedule(file: Path, argument: str) -> Any:
    """The checked schedule of a file."""
    from .schedule import Schedule

    how = "give a schedule file (format openkeito-schedule, see get_schema with kind 'schedule')"
    try:
        document = json.loads(file.read_text(encoding="utf-8"))
    except UnicodeDecodeError as e:
        raise _tool_error(f"{argument} {file} is not UTF-8 text; {how}") from e
    except OSError as e:
        raise _tool_error(f"cannot read {argument} {file}: {_reason(e)}; {how}") from e
    except (ValueError, RecursionError) as e:
        raise _tool_error(f"{argument} {file} is not JSON ({_reason(e)}); {how}") from e
    try:
        return Schedule(document)
    except (openkeito.OpenKeitoError, TypeError, ValueError, KeyError) as e:
        raise _tool_error(f"{argument} {file} is not a valid schedule: {_reason(e)}") from e


def _target(file: Path, force: bool, argument: str) -> Path:
    """The file that a tool is to write: not a directory, and not an existing
    file unless ``force``."""
    if file.is_dir():
        raise _tool_error(f"{argument} {file} is a directory; give the path of the file to write")
    if file.exists() and not force:
        raise _tool_error(f"{file} already exists and is not overwritten; give another {argument}, or "
                          "force=true to replace the file")
    return file


def _write_json(file: Path, value: Any, force: bool) -> str:
    try:
        file.parent.mkdir(parents=True, exist_ok=True)
        with file.open("w" if force else "x", encoding="utf-8") as f:
            f.write(json.dumps(value, indent=2) + "\n")
    except FileExistsError as e:
        raise _tool_error(f"{file} already exists and is not overwritten; give another path, or force=true "
                          "to replace the file") from e
    except OSError as e:
        raise _tool_error(f"cannot write {file}: {_reason(e)}; give a path that can be written") from e
    return str(file)


def _rows(file: Path, start: float | None, stop: float | None) -> Iterator[list[str]]:
    """The rows of a result file whose axis value is between start and stop."""
    with file.open(encoding="utf-8", newline="") as f:
        reader = csv.reader(f)
        next(reader, None)
        for row in reader:
            if not row:
                continue
            axis = float(row[0])
            if (start is None or axis >= start) and (stop is None or axis <= stop):
                yield row


def _number(text: str) -> float | None:
    return float(text) if text != "" else None


def _query(file: Path, columns: list[str] | None, start: float | None, stop: float | None,
           max_rows: int) -> dict[str, Any]:
    with file.open(encoding="utf-8", newline="") as f:
        header = next(csv.reader(f), None)
    if not header:
        raise _tool_error(f"{file} is empty; run run_model or scan_frequency again into a new directory")
    names = header[1:]
    shown = f"{names[:20]}" + (f" and {len(names) - 20} more (the header of {file})" if len(names) > 20 else "")
    if columns:
        missing = [c for c in columns if c not in names]
        if missing:
            raise _tool_error(f"no columns {missing[:20]}; the columns are {shown}")
        if len(columns) > MAX_COLUMNS:
            raise _tool_error(f"{len(columns)} columns are chosen; choose at most {MAX_COLUMNS} in one call")
    wanted = list(columns) if columns else names[:MAX_COLUMNS]
    idx = [header.index(c) for c in wanted]

    # First pass: the statistics and the number of rows; second pass: the rows
    # to return. Nothing but the answer is kept in memory.
    count = 0
    low: list[float | None] = [None] * len(idx)
    high: list[float | None] = [None] * len(idx)
    last: list[float | None] = [None] * len(idx)
    for row in _rows(file, start, stop):
        count += 1
        for j, k in enumerate(idx):
            if row[k] == "":
                continue
            v = float(row[k])
            if low[j] is None or v < low[j]:
                low[j] = v
            if high[j] is None or v > high[j]:
                high[j] = v
            last[j] = v
    stats = {c: {"min": low[j], "max": high[j], "max_abs": max(abs(low[j]), abs(high[j])), "last": last[j]}
             for j, c in enumerate(wanted) if last[j] is not None}

    limit = min(max_rows, max(1, MAX_VALUES // (len(wanted) + 1)))
    if count <= limit:
        step = 1
    elif limit == 1:
        step = count
    else:
        step = math.ceil((count - 1) / (limit - 1))
    rows = []
    for n, row in enumerate(_rows(file, start, stop)):
        # Every step-th row and the last one; the last one alone when only one fits.
        if n == count - 1 or (n % step == 0 and limit > 1):
            rows.append([_number(row[0])] + [_number(row[k]) for k in idx])

    notes = []
    if step > 1:
        notes.append(f"{len(rows)} of the {count} rows in the range are returned (every {step}th and the last): "
                     f"an answer holds at most max_rows rows and {MAX_VALUES} values. For more detail choose "
                     "fewer columns or a narrower range (start, stop)")
    if len(wanted) < len(names) and not columns:
        notes.append(f"only the first {MAX_COLUMNS} of the {len(names)} columns are returned; choose others "
                     "with columns")
    answer = {
        "file": str(file), "axis": header[0], "columns": wanted, "columns_total": len(names),
        "rows_in_range": count, "rows_returned": len(rows), "every": step, "truncated": bool(notes),
    }
    if notes:
        answer["note"] = "; ".join(notes) + f"; or read the file {file} (CSV, one row per step)."
    answer["statistics"] = stats
    answer["rows"] = rows
    return answer


def _with_warnings(seen: list[str]) -> dict[str, Any]:
    if not seen:
        return {}
    out: dict[str, Any] = {"warnings": seen[:MAX_WARNINGS]}
    if len(seen) > MAX_WARNINGS:
        out["warnings_total"] = len(seen)
        out["warnings_truncated"] = True
    return out


def _prepare_schedule_run(
    source: str,
    network: str,
    schedule_path: str,
    out: str,
    jobs: int | None,
    options: dict[str, Any] | None,
    short_circuit_profile: bool,
    paths: _Paths,
) -> tuple[Path, Any, Any, dict[str, Any], list[str]]:
    """Shared argument checks and loading for run_time_slices and run_transitions."""
    if source not in ("pandapower", "powsybl"):
        raise _tool_error("source is 'pandapower' or 'powsybl'")
    if jobs is not None and jobs < 1:
        raise _tool_error("jobs is the number of cases to run at once: 1 or more, or leave it out")
    directory = paths.folder(out, "out", depth=2)
    schedule_file = paths.path(schedule_path, "schedule_path")
    net, file = _network(source, network, paths)
    options, _, notes = _short_circuit(source, file, net, options, short_circuit_profile)
    plan = _schedule(schedule_file, "schedule_path")
    return directory, net, plan, options, notes


def _cases(result: Any) -> dict[str, Any]:
    """The rows of the time slices, with the diagnostics of the cases that did
    not complete; at most MAX_CASES of them, those that did not complete first."""
    rows = []
    for row in result.rows:
        row = dict(row)
        if row.get("status") != "completed":
            case = result.batch.cases.get(row.get("case"))
            errors = case.summary.get("errors") if case is not None else None
            if errors:
                row["errors"] = errors[:MAX_CASE_ERRORS]
        rows.append(row)
    answer: dict[str, Any] = {"cases_total": len(rows), "cases_truncated": len(rows) > MAX_CASES}
    if len(rows) > MAX_CASES:
        rows.sort(key=lambda r: r.get("status") == "completed")
        rows = rows[:MAX_CASES]
        answer["note"] = (f"{MAX_CASES} of the {answer['cases_total']} cases are listed, those that did not "
                          "complete first; all are in batch-summary.json and batch-metrics.csv in the output "
                          "directory, and each case's summary.json is in its own directory")
    answer["cases"] = rows
    return answer


def server(root: str | os.PathLike | None = None, allow_any_path: bool = False) -> Any:
    """The MCP server with every tool registered.

    root: the folder under which the tools read and write (default: the
    environment variable OPENKEITO_MCP_ROOT, or else the working directory).
    Relative paths are taken from it. A root that is not an existing folder
    raises ValueError.
    allow_any_path: do not refuse paths outside the root."""
    from mcp.server.mcpserver import MCPServer

    paths = _Paths(root, allow_any_path)
    limit = (f"\nPaths: relative paths are taken from the root folder {paths.root}. "
             + ("Paths outside it are allowed (the server was started with --allow-any-path)."
                if allow_any_path else
                "The tools read and write only under it; a path that resolves to outside it is refused."))
    s = MCPServer("openkeito", instructions=INSTRUCTIONS + limit, version=openkeito.__version__)

    @s.tool()
    def get_schema(kind: str = "model") -> dict[str, Any]:
        """JSON Schema of a file format: kind "model" (default), "batch" or "schedule"."""
        if kind not in ("model", "batch", "schedule"):
            raise _tool_error("kind is 'model', 'batch' or 'schedule'")
        return openkeito.schema(batch=kind == "batch", schedule=kind == "schedule")

    @s.tool()
    def list_components() -> dict[str, Any]:
        """Component types with parameters (units in the names), sign conventions and an example each."""
        return {"components": openkeito.components()}

    @s.tool()
    def validate_model(model: dict[str, Any] | None = None, path: str | None = None) -> dict[str, Any]:
        """Checks a model (the JSON document, or a file path) without running it. Returns valid,
        errors and warnings; each diagnostic has code, path, message and a hint saying what to change."""
        source = _model(model, path, paths)
        if isinstance(source, str) and not source.lstrip().startswith("{"):
            return json.loads(openkeito._native.validate(source))
        return openkeito.validate(source)

    @s.tool()
    def get_topology(model: dict[str, Any] | None = None, path: str | None = None) -> dict[str, Any]:
        """Returns the connections of a model (the JSON document, or a file path) without running
        it: components with their terminals and signals, nodes, buses, control blocks, signals and
        diagnostics, plus valid, errors and warnings."""
        source = _model(model, path, paths)
        if isinstance(source, str) and not source.lstrip().startswith("{"):
            return json.loads(openkeito._native.topology(source))
        return openkeito.topology(source)

    @s.tool()
    def run_model(out: str, model: dict[str, Any] | None = None, path: str | None = None,
                  force: bool = False) -> dict[str, Any]:
        """Runs a model and writes result.csv, events.csv and summary.json into `out` (a new
        directory; a directory that holds a result is refused unless force=true). Returns the summary:
        status, events, warnings and per-column statistics. For a model with more than 50 columns the
        per-column lists are left out (`omitted`; they are in the file `summary_path`) and
        result.column_names lists the columns. Read values with query_results."""
        directory = paths.folder(out, "out")
        source = _model(model, path, paths)
        try:
            if isinstance(source, str) and not source.lstrip().startswith("{"):
                result = openkeito.Result(openkeito._native.run(source, str(directory), force))
            else:
                result = openkeito.run(source, directory, force=force, check=False)
        except OSError as e:
            raise _tool_error(f"cannot run into {directory}: {_reason(e)}") from e
        return _short_summary(result.summary, directory)

    @s.tool()
    def scan_frequency(out: str, model: dict[str, Any] | None = None, path: str | None = None,
                       force: bool = False) -> dict[str, Any]:
        """Computes the model's frequency_scan (impedances against frequency) into `out`/scan.csv
        (a new directory; a directory that holds a result is refused unless force=true). Returns the
        summary, shortened as that of run_model."""
        directory = paths.folder(out, "out")
        source = _model(model, path, paths)
        try:
            if isinstance(source, str) and not source.lstrip().startswith("{"):
                result = openkeito.Result(openkeito._native.scan(source, str(directory), force))
            else:
                result = openkeito.scan(source, directory, force=force, check=False)
        except OSError as e:
            raise _tool_error(f"cannot scan into {directory}: {_reason(e)}") from e
        return _short_summary(result.summary, directory)

    @s.tool()
    def query_results(out: str, columns: list[str] | None = None, start: float | None = None,
                      stop: float | None = None, max_rows: int = 500) -> dict[str, Any]:
        """Reads recorded values from the directory of a run (result.csv) or a scan (scan.csv):
        the chosen columns (default: all, up to 500), rows with the axis (time_s or frequency_hz)
        between start and stop, and per column min, max, max_abs and last over the whole range.
        An answer holds at most max_rows rows and 20000 values (rows times the axis and the columns):
        beyond that the rows are thinned evenly (every `every`-th row and the last), `truncated` is
        true and `note` says how to get more: fewer columns, a narrower range, or the file itself."""
        if max_rows < 1:
            raise _tool_error("max_rows is the largest number of rows to return: 1 or more")
        d = paths.path(out, "out")
        file = d / "result.csv" if (d / "result.csv").exists() else d / "scan.csv"
        if not file.exists():
            raise _tool_error(f"{d} has no result.csv or scan.csv; run run_model or scan_frequency first")
        file = paths.path(str(file), "out", display=out)  # the file itself can be a link
        try:
            return _query(file, columns, start, stop, max_rows)
        except (OSError, ValueError, IndexError, csv.Error) as e:
            raise _tool_error(f"cannot read {file}: {_reason(e)}; it is not the result.csv or scan.csv of a "
                              "run, so run the model again into a new directory") from e

    @s.tool()
    def convert_network(source: str, network: str, model_path: str, options: dict[str, Any] | None = None,
                        force: bool = False, short_circuit_profile: bool = True) -> dict[str, Any]:
        """Converts a power-flow network into a model that starts from its power flow, and writes
        the model to model_path and the element mapping next to it (<model>.mapping.json); existing
        files are refused unless force=true.
        source: "pandapower" (network: the name of an example in pandapower.networks such as "case39",
        or a .json file written by pandapower.to_json; pickle files are not loaded) or "powsybl"
        (network: a pypowsybl example such as "micro_grid_be" or "ieee14", or a file that pypowsybl
        imports: CGMES, XIIDM, UCTE, IEEE-CDF, MATPOWER, PSS/E or PowerFactory).
        options go to from_pandapower / from_powsybl, such as end_time_s, time_step_s or outputs
        ("buses" or "all"). For "powsybl", {"phase_angle_clocks": "rotate"} turns the model by the
        phase-angle clocks of the transformers (Yd and Dy shift by 30 degrees; the default leaves
        them out, as the load flow does).
        short_circuit_profile: for a CGMES file of source "powsybl", also read the short-circuit
        attributes of its equipment profile (subtransient reactances, zero-sequence impedances,
        winding connections, grounding), which replace the converter's assumptions; attributes that
        could not be used are named in `warnings`. false keeps the assumptions."""
        if source not in ("pandapower", "powsybl"):
            raise _tool_error("source is 'pandapower' or 'powsybl'")
        model_file = _target(paths.path(model_path, "model_path"), force, "model_path")
        mapping_file = _target(paths.path(str(model_file.with_suffix("")) + ".mapping.json", "model_path"),
                               force, "model_path")
        net, file = _network(source, network, paths)
        given = (options or {}).get("short_circuit_data")
        options, read, notes = _short_circuit(source, file, net, options, short_circuit_profile)
        try:
            conv = _convert(source, net, options)
        except Exception as e:
            raise _failure("cannot convert", e) from e
        if read:
            from .powsybl import label_cgmes_short_circuit_data

            label_cgmes_short_circuit_data(conv, read, given)
        conv.warnings.extend(notes)
        return {
            "model_path": _write_json(model_file, conv.model, force),
            "mapping_path": _write_json(mapping_file, conv.mapping, force),
            "components": len(conv.model["components"]), "buses": len(conv.mapping["buses"]),
            "warnings": conv.warnings,
        }

    @s.tool()
    def make_schedule(network: str, inputs_path: str, schedule_path: str, opf: str = "ac",
                      force: bool = False) -> dict[str, Any]:
        """Generator set points from pandapower's optimal power flow ("ac" or "dc") at every time of
        an input schedule (loads and other inputs, format openkeito-schedule; see
        get_schema("schedule")). network: the name of an example in pandapower.networks, or a .json
        file written by pandapower.to_json. Writes the schedule (inputs plus generator outputs and
        voltages) to schedule_path; an existing file is refused unless force=true."""
        if opf not in ("ac", "dc"):
            raise _tool_error("opf is 'ac' or 'dc'")
        schedule_file = _target(paths.path(schedule_path, "schedule_path"), force, "schedule_path")
        inputs_file = paths.path(inputs_path, "inputs_path")
        net, _ = _network("pandapower", network, paths)
        inputs = _schedule(inputs_file, "inputs_path")
        from .pandapower import schedule_from_opf

        try:
            plan, notes = schedule_from_opf(net, inputs, opf=opf)
        except Exception as e:
            raise _failure("cannot make the schedule", e) from e
        return {"schedule_path": _write_json(schedule_file, plan.data, force), "times": len(plan.times),
                "series": len(plan.series), "warnings": notes}

    @s.tool()
    def run_time_slices(source: str, network: str, schedule_path: str, out: str,
                        disturbances: list[dict[str, Any]] | None = None, times: list[str] | None = None,
                        jobs: int | None = None, force: bool = False,
                        options: dict[str, Any] | None = None,
                        short_circuit_profile: bool = True) -> dict[str, Any]:
        """Applies a schedule to a network at each of its times (or the given times), solves the power
        flow, converts it, adds each disturbance and runs every case in parallel into out/<case>
        (a case directory that holds a result is refused unless force=true).
        source, network and short_circuit_profile are those of convert_network (`warnings`: the
        short-circuit attributes of a CGMES file that could not be used). A disturbance is {"name", "components":
        [...], "outputs": [...]}; a component's "bus" may be a bus of the source network (pandapower
        index, IIDM bus id). Returns one row per case: status, the largest steady-state difference
        from the power flow, the departure from the steady state before the first switching, the
        number of warnings, and the diagnostics (`errors`) of a case that did not complete. Above
        200 cases, 200 are listed (those that did not complete first; `cases_truncated`, `note`)."""
        directory, net, plan, options, notes = _prepare_schedule_run(
            source, network, schedule_path, out, jobs, options, short_circuit_profile, paths)
        if source == "pandapower":
            from .pandapower import run_time_slices as run
        else:
            from .powsybl import run_time_slices as run
        try:
            result = run(net, plan, disturbances, times=times, jobs=jobs, out=directory, force=force, **options)
        except Exception as e:
            raise _failure("cannot run the time slices", e) from e
        seen: list[str] = list(notes)
        for s in result.slices:
            seen += [w for w in s.conversion.warnings if w not in seen]
        return {"output_directory": str(directory), "status": result.batch.status,
                "counts": result.batch.summary.get("counts"), **_with_warnings(seen), **_cases(result)}

    @s.tool()
    def run_transitions(source: str, network: str, schedule_path: str, out: str,
                        intervals: list[list[str]] | None = None, jobs: int | None = None,
                        force: bool = False, options: dict[str, Any] | None = None,
                        short_circuit_profile: bool = True, chain: bool = False,
                        times: list[str] | None = None, estimate_only: bool = False,
                        max_wall_s: float | None = None) -> dict[str, Any]:
        """Follows a schedule from one of its times to the next in an EMT run: for every pair of
        neighbouring times (or the given intervals, [[from_time, to_time], ...]) the model starts from
        the power flow of from_time, and the set points of machines (governor and voltage regulator),
        converters, loads and breakers move to the values of to_time. Runs the cases in parallel into
        out/<case> (i000, i001, ...; refused when it holds a result unless force=true).
        chain=true (or times, a list of two or more times of the schedule) makes one run instead, the
        case `chain`, through all times of the schedule (or the times given, or the intervals when
        each begins where the one before ends): steady state, then for each following time its
        changes and its settling time, one after the other; the hours between the times are not
        run. Choose it to see what one change leaves for the next; the cases of their own start
        each from an exact steady state and use several cores. A machine may start at any time of
        a chain and stop once, loads and breakers go in and out as often as the schedule says; a
        machine that is to come back after stopping is refused with the time at which to split.
        source, network and short_circuit_profile are those of convert_network. options:
        start_time_s (steady state before
        the change, default 1), ramp_time_s (seconds, or {element: seconds}; 0 is a step; "schedule"
        is the real time between the two times of a linear schedule; default 5),
        ramp_rate_mw_per_min, settle_time_s (default 10), load_steps, exciter, governor, pss (the
        machines get a power system stabilizer unless pss is false; a dict changes its values),
        machine_controls ({element: {"exciter": ..., "governor": ..., "pss": ...}} for single
        machines), and the options of the converter (time_step_s, ...). In a chain, ramp_time_s,
        ramp_rate_mw_per_min, settle_time_s and switch_delay_s (seconds after the beginning of the
        changes at which breakers act) also take a list with one entry per interval. One simulated
        second costs up to about (number of buses / 40) seconds on one core at 50 us
        (docs/performance.md): keep the times short. A chain lasts start_time_s plus, for every
        interval, its ramp and settling time (end_time_s of its row), on one core.
        Returns one row per case: status; max_abs_dv_pu (start against the power flow of from_time);
        drift_before_change_pu; end_max_abs_dv_pu, end_max_abs_dva_degree and end_max_abs_dp_mw (end
        against the power flow of to_time); the smallest and largest voltage and frequency and the
        largest machine and converter currents on the way; end_voltage_swing_pu and
        end_frequency_swing_hz (what still moves at the end); and `changes` and `warnings`: what was
        changed and what could not be followed (tap changes, sources), and what was assumed: "no
        stabilizer for machines that deliver less than 0.1 of their rating ..." names the machines
        that got none, "no rating; rated for all times of the transition ..." those without a rating
        in the data. The row of a chain also has `times`: for every time after the first, time_s
        (the end of its settling time in the run) and the end_... values there against the power
        flow of that time."""
        if source not in ("pandapower", "powsybl"):
            raise _tool_error("source is 'pandapower' or 'powsybl'")
        if max_wall_s is not None and max_wall_s <= 0:
            raise _tool_error("max_wall_s must be > 0")
        directory, net, plan, options, notes = _prepare_schedule_run(
            source, network, schedule_path, out, jobs, options, short_circuit_profile, paths)
        if source == "pandapower":
            from .pandapower import apply_schedule, check_schedule, from_pandapower, transition as make_case
            rename_buses = False
        else:
            from .powsybl import apply_schedule, check_schedule, from_powsybl as from_pandapower, transition as make_case
            rename_buses = True
        from .transition import estimate as estimate_transition, make_transitions, run as run_cases

        try:
            cases = make_transitions(
                net, plan, intervals, make=make_case, options=options, times=times, chain=chain,
                check=check_schedule, apply=apply_schedule, convert=from_pandapower, rename_buses=rename_buses)
        except Exception as e:
            raise _failure("cannot build the transitions", e) from e
        if estimate_only or max_wall_s is not None:
            try:
                estimates = [{"case": t.name, "from_time": t.from_time, "to_time": t.to_time,
                              **estimate_transition(t)} for t in cases]
            except Exception as e:
                raise _failure("cannot estimate the transitions", e) from e
            total_steps = sum(e["steps"] for e in estimates)
            total_wall = sum(e["estimated_wall_s"] for e in estimates)
            if estimate_only:
                seen_est: list[str] = list(notes)
                for t in cases:
                    seen_est += [w for w in t.warnings if w not in seen_est]
                est_extra: dict[str, Any] = _with_warnings(seen_est)
                if len(estimates) == 1:
                    est_extra.update({
                        k: estimates[0][k] for k in (
                            "time_step_s", "end_time_s", "stages", "components", "controls", "switches", "step_wall_us"
                        )
                    })
                return {
                    "estimate_only": True,
                    "steps": total_steps,
                    "estimated_wall_s": total_wall,
                    "cases": estimates,
                    **est_extra,
                }
            if max_wall_s is not None and total_wall > max_wall_s:
                raise _tool_error(
                    f"estimated wall time {total_wall:.2f} s exceeds max_wall_s={max_wall_s:g} s "
                    f"({total_steps} steps); shorten settle_time_s, increase time_step_s, or raise max_wall_s"
                )
        try:
            result = run_cases(cases, jobs=jobs, out=directory, force=force)
        except Exception as e:
            raise _failure("cannot run the transitions", e) from e
        answer = _cases(result)
        made = {t.name: t for t in result.transitions}
        for row in answer["cases"]:
            t = made.get(row.get("case"))
            if t is not None:
                row["changes"] = t.changes
                row["end_time_s"] = t.end_time_s
                if len(t.stages) > 1:
                    row["times"] = [{key: value for key, value in at.items()
                                     if key in ("time", "time_s", "settled") or key.startswith("end_")}
                                    for at in result.time_rows if at.get("case") == t.name]
        seen: list[str] = list(notes)
        for w in result.warnings:
            if w not in seen:
                seen.append(w)
        return {"output_directory": str(directory), "status": result.batch.status,
                "counts": result.batch.summary.get("counts"), "warnings": seen[:MAX_WARNINGS],
                **{k: v for k, v in _with_warnings(seen).items() if k != "warnings"}, **answer}

    return s


def main(argv: list[str] | None = None) -> None:
    """Runs the server on stdio. The transport reads and writes UTF-8 whatever
    the encoding of the console is."""
    parser = argparse.ArgumentParser(
        prog="openkeito-mcp",
        description="MCP server of OpenKeito on stdio. Its tools read and write only under the root folder.")
    parser.add_argument("--root", metavar="DIR",
                        help="the folder under which the tools read and write; relative paths are taken from "
                             f"it (default: the environment variable {ROOT_VARIABLE}, or else the working "
                             "directory)")
    parser.add_argument("--allow-any-path", action="store_true",
                        help="let the tools read and write outside the root too (anywhere the user who starts "
                             "the server can)")
    parser.add_argument("--version", action="version", version=f"openkeito-mcp {openkeito.__version__}")
    arguments = parser.parse_args(argv)
    try:
        s = server(root=arguments.root, allow_any_path=arguments.allow_any_path)
    except ValueError as e:
        parser.error(str(e))
    if arguments.allow_any_path:
        print("openkeito-mcp: warning: started with --allow-any-path: the tools read and write any path that "
              "this user can, not only those under the root", file=sys.stderr, flush=True)
    s.run("stdio")


if __name__ == "__main__":
    main()
