"""Makes the tables and figures of the paper material from OpenKeito itself.

    python paper/make.py                      # everything, into paper/out
    python paper/make.py --only buck_switching --only line_models
    python paper/make.py --out some/dir
    python paper/make.py --quick              # small settings (the test)
    python paper/make.py --measure            # also measure run times here
    python paper/make.py --list

Every item writes its numbers as CSV (the data behind the figure) and the
figure as PDF and PNG, or a table as .tex and .md, into the output directory,
and its key numbers into numbers.json. Nothing depends on the clock or on
random numbers: the same inputs give the same CSV files. Run times are the
exception; they are only written with --measure, into files named
*_measured.csv (see figures/performance.py and figures/schedule_runs.py).
"""

from __future__ import annotations

import argparse
import importlib
import json
import logging
import sys
import warnings
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import lib  # noqa: E402
import style  # noqa: E402

#: Module names under figures/, in the order of paper/README.md.
ITEMS = [
    "integration_order",
    "numerical_oscillation",
    "buck_switching",
    "diode_bridge",
    "fault_currents",
    "steady_state",
    "line_models",
    "machine_short_circuit",
    "converter_response",
    "controls_v07",
    "expression_control",
    "ai_control",
    "wecc_ibr",
    "power_flow_start",
    "schedule_runs",
    "ieee39_diagram",
    "performance",
    "diagnostics",
]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--only", action="append", metavar="NAME", help="make only this item (may be repeated)")
    parser.add_argument("--out", type=Path, default=HERE / "out", help="output directory (default: paper/out)")
    parser.add_argument("--quick", action="store_true", help="small settings: the numbers differ from the full run")
    parser.add_argument("--measure", action="store_true", help="also measure run times on this machine")
    parser.add_argument("--list", action="store_true", help="list the items and stop")
    args = parser.parse_args(argv)

    if args.list:
        for name in ITEMS:
            module = importlib.import_module(f"figures.{name}")
            print(f"{name}: {module.TITLE}")
        return 0
    names = args.only or ITEMS
    unknown = [n for n in names if n not in ITEMS]
    if unknown:
        parser.error(f"unknown item(s) {', '.join(unknown)}; items: {', '.join(ITEMS)}")

    args.out.mkdir(parents=True, exist_ok=True)
    style.apply()
    # The notes of pandapower about its own bundled cases are not ours to fix.
    warnings.filterwarnings("ignore", module="pandapower")
    logging.getLogger("pandapower").setLevel(logging.ERROR)
    ctx = lib.Context(out=args.out, quick=args.quick, measure=args.measure)
    settings = {"quick": bool(ctx.quick)}
    numbers_path = args.out / "numbers.json"
    numbers: dict = {}
    if args.only and numbers_path.exists():
        numbers = json.loads(numbers_path.read_text(encoding="utf-8"))
        existing = numbers.get("_settings")
        if existing is not None and existing != settings:
            parser.error(f"{numbers_path} has settings {existing}, which differs from {settings}; "
                         "run without --only or into another --out")
    for name in names:
        module = importlib.import_module(f"figures.{name}")
        print(f"[{name}] {module.TITLE}", flush=True)
        numbers[name] = module.make(ctx)
        for key, value in numbers[name].items():
            print(f"    {key}: {value}")
    ordered = {"_settings": settings, **{name: numbers[name] for name in ITEMS if name in numbers}}
    with open(numbers_path, "w", encoding="utf-8", newline="\n") as f:
        json.dump(ordered, f, indent=2, ensure_ascii=False)
        f.write("\n")
    print(f"wrote {len(ctx.files)} files and numbers.json to {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
