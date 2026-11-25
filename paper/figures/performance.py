"""Run times: figures from the recorded numbers, and a measurement on this
machine on request.

Run times depend on the machine and on what else runs on it, so they are
kept apart from everything else:

- By default the figure is drawn from paper/data/*_recorded.csv, which are
  the tables of docs/performance.md (one pinned fast core of the machine
  named there, 2026-11-22). The same files give the same figure.
- With `--measure`, the ring systems are also run here and written to
  performance_measured.csv, and drawn into a second figure next to the
  recorded numbers. Pin the process to one fast core first, on Windows

      cmd /c start /b /wait /affinity 400 .venv\\Scripts\\python.exe paper\\make.py --only performance --measure

  (the mask is a hexadecimal set of logical processors), or give the mask in
  the environment variable OPENKEITO_PAPER_AFFINITY and let this module set
  it. The runs with several threads need the cores they may use in the mask.
  The WebAssembly numbers are not measured here: they need Node and the
  built module (crates/openkeito-wasm/README.md).

The ring system (crates/openkeito-core/tests/benchmark.rs): N buses at 66 kV
joined by 10 km lines, a grounded star load of 10 MW + 3 Mvar on every bus
and, on every tenth bus, an 11 kV source behind a 100 MVA YNd11 transformer;
a three-phase fault on bus N/2 from 0.1 to 0.2 s. A time per step is the
time of the run without the output, divided by the steps.
"""

from __future__ import annotations

import csv
import math
import os
import sys
from pathlib import Path

import numpy as np

import lib
import style

TITLE = "Time per step against the unknowns, with threads, and WebAssembly against native (recorded numbers)"
DATA = Path(__file__).resolve().parents[1] / "data"
FREQ = 50.0


def read(name: str) -> list[dict[str, str]]:
    with open(DATA / name, encoding="utf-8", newline="") as f:
        return list(csv.DictReader(line for line in f if not line.startswith("#")))


def number(text: str) -> float:
    return float(text) if text else float("nan")


def ring(n: int, h: float, end: float, solver: str = "auto", line_type: str = "pi_line",
         partitioning: str | None = None, threads: int | None = None) -> dict:
    w = 2 * math.pi * FREQ
    p, q, v = 10e6, 3e6, 66e3
    s2 = p * p + q * q
    r, x = v * v * p / s2, v * v * q / s2
    c = []
    for k in range(n):
        c.append({"type": line_type, "name": f"L{k}", "buses": [f"b{k}", f"b{(k + 1) % n}"], "length_km": 10.0,
                  "per_km": {"type": "sequence", "r1_ohm_per_km": 0.03, "l1_h_per_km": 0.9e-3,
                             "c1_f_per_km": 12.7e-9, "r0_ohm_per_km": 0.2, "l0_h_per_km": 3e-3,
                             "c0_f_per_km": 7e-9}})
        c.append({"type": "coupled_rl", "name": f"LD{k}", "buses": [f"b{k}", "gnd"],
                  "impedance": {"type": "sequence", "r1_ohm": r, "l1_h": x / w, "r0_ohm": r, "l0_h": x / w}})
        if k % 10 == 0:
            c.append({"type": "three_phase_source", "name": f"G{k}", "bus": f"g{k}", "line_voltage_rms_v": 11e3,
                      "frequency_hz": FREQ,
                      "impedance": {"type": "sequence", "r1_ohm": 0.01, "l1_h": 0.1 / w, "r0_ohm": 0.01,
                                    "l0_h": 0.1 / w}})
            c.append({"type": "three_phase_transformer", "name": f"T{k}", "buses": [f"b{k}", f"g{k}"],
                      "vector_group": "YNd11", "rated_voltage_kv": [66.0, 11.0], "rated_power_mva": 100.0,
                      "rated_frequency_hz": FREQ, "impedance_percent": 12.0, "resistance_percent": 0.3,
                      "no_load_current_percent": 0.2, "no_load_loss_kw": 50.0})
    faulted = n // 2
    c.append({"type": "fault", "name": "F1", "bus": f"b{faulted}", "phases": ["a", "b", "c"], "ground": True,
              "start_time_s": 0.1, "clear_time_s": 0.2})
    simulation = {"time_step_s": h, "end_time_s": end, "initialization": "steady_state", "linear_solver": solver,
                  "output_interval_steps": 10}
    if partitioning:
        simulation["partitioning"] = partitioning
    if threads:
        simulation["threads"] = threads
    return lib.model(simulation, c, [{"type": "bus_voltage", "bus": f"b{faulted}"},
                                     {"type": "branch_current", "component": "F1"}])


def per_step_us(m: dict, rounds: int) -> tuple[float, dict]:
    """Best time per step of `rounds` runs, without the output."""
    best, summary = math.inf, {}
    for _ in range(rounds):
        summary = lib.run(m).summary
        t = summary["timing"]
        best = min(best, (t["run_s"] - t["output_s"]) / summary["solver"]["steps"])
    return best * 1e6, summary


def pin() -> str:
    """Pins the process to OPENKEITO_PAPER_AFFINITY (a hexadecimal mask of
    logical processors), when given. Returns what was done."""
    text = os.environ.get("OPENKEITO_PAPER_AFFINITY")
    if not text:
        return "not pinned by this script"
    mask = int(text, 16)
    if sys.platform == "win32":
        import ctypes

        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.GetCurrentProcess.restype = ctypes.c_void_p
        kernel.SetProcessAffinityMask.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
        if not kernel.SetProcessAffinityMask(kernel.GetCurrentProcess(), mask):
            return f"could not set the affinity mask {text}"
    elif hasattr(os, "sched_setaffinity"):
        os.sched_setaffinity(0, {k for k in range(mask.bit_length()) if mask >> k & 1})
    else:
        return "this system has no call to set the affinity"
    return f"pinned to the mask {text}"


def measure(ctx: lib.Context) -> dict:
    pinned = pin()
    rounds = 2 if ctx.quick else 5
    sizes = [10, 30] if ctx.quick else [10, 30, 100, 300, 1000]
    rows = []
    for n in sizes:
        end = 0.01 if ctx.quick else 0.2
        us, s = per_step_us(ring(n, 10e-6, end, "sparse"), rounds)
        rows.append(("ring", n, s["solver"]["unknowns"], "pi_line", "sparse", "off", 1, us))
        if n <= 100:
            # The dense LU grows with the square of the unknowns: a shorter run.
            us, s = per_step_us(ring(n, 10e-6, min(end, 0.02 if n == 100 else end), "dense"), rounds)
            rows.append(("ring", n, s["solver"]["unknowns"], "pi_line", "dense", "off", 1, us))
    for n in ([30] if ctx.quick else [300, 1000]):
        end = 0.01 if ctx.quick else 0.2
        us, s = per_step_us(ring(n, 10e-6, end, "auto", "bergeron_line", "off"), rounds)
        rows.append(("ring", n, s["solver"]["unknowns"], "bergeron_line", "auto", "off", 1, us))
        for threads in (1, 2, 4, 8):
            us, s = per_step_us(ring(n, 10e-6, end, "auto", "bergeron_line", "auto", threads), rounds)
            rows.append(("ring", n, s["solver"]["unknowns"], "bergeron_line", "auto", "auto",
                         s["solver"]["threads"], us))
    lib.write_csv(ctx, "performance_measured.csv",
                  ["system", "buses", "unknowns", "line", "linear_solver", "partitioning", "threads_used",
                   "us_per_step"], rows)

    ring_rows = [r for r in read("ring_recorded.csv") if r["time_step_us"] == "10"]
    recorded = {(int(r["buses"]), solver): number(r[f"{solver}_us_per_step"])
                for r in ring_rows for solver in ("sparse", "dense")}
    fig, ax = style.figure()
    for k, solver in enumerate(("sparse", "dense")):
        mine = [(r[2], r[7]) for r in rows if r[3] == "pi_line" and r[4] == solver]
        ax.loglog(*zip(*mine), label=f"{solver.capitalize()} LU, measured here", **style.series(k))
        theirs = [(r[2], recorded[(r[1], solver)]) for r in rows if r[3] == "pi_line" and r[4] == solver
                  and not math.isnan(recorded.get((r[1], solver), math.nan))]
        if theirs:
            ax.loglog(*zip(*theirs), label=f"{solver.capitalize()} LU, recorded", color=style.REFERENCE,
                      linestyle="none", marker="x" if k == 0 else "+")
    ax.set_xlabel("Unknowns")
    ax.set_ylabel("Time per step (µs)")
    ax.legend()
    lib.save_figure(ctx, fig, "performance_measured")
    ratios = [r[7] / recorded[(r[1], r[4])] for r in rows if r[3] == "pi_line"
              and not math.isnan(recorded.get((r[1], r[4]), math.nan))]
    return {"measured here": pinned,
            "measured here: sparse LU, us per step by buses": {str(r[1]): round(r[7], 2) for r in rows
                                                               if r[3] == "pi_line" and r[4] == "sparse"},
            "measured here / recorded, range": [round(min(ratios), 2), round(max(ratios), 2)]}


def make(ctx: lib.Context) -> dict:
    rings = [r for r in read("ring_recorded.csv") if r["time_step_us"] == "10"]
    parts = read("partition_recorded.csv")
    wasm = read("wasm_recorded.csv")
    converted = read("converted_recorded.csv")

    unknowns = np.array([number(r["unknowns"]) for r in rings])
    sparse = np.array([number(r["sparse_us_per_step"]) for r in rings])
    dense = np.array([number(r["dense_us_per_step"]) for r in rings])
    threads = [1, 2, 4, 8]
    chosen = ["ring of 300 buses", "ring of 1000 buses", "16 blocks of 30 buses", "8 blocks of 100 buses"]
    speedup = {}
    for r in parts:
        if r["system"] in chosen:
            # Against the same circuit not divided, on one thread.
            speedup[r["system"]] = [number(r["off"]) / number(r[f"auto_{n}"]) for n in threads]
    ratio = [number(r["wasm_us_per_step"]) / number(r["native_us_per_step"]) for r in wasm]

    lib.write_csv(ctx, "performance_ring.csv",
                  ["buses", "unknowns", "sparse_us_per_step", "dense_us_per_step", "sparse_ns_per_unknown"],
                  [(int(r["buses"]), int(r["unknowns"]), s, d, 1e3 * s / int(r["unknowns"]))
                   for r, s, d in zip(rings, sparse, dense)])
    lib.write_csv(ctx, "performance_threads.csv", ["system", "threads", "us_per_step", "speedup_against_undivided"],
                  [(r["system"], n, number(r[f"auto_{n}"]), number(r["off"]) / number(r[f"auto_{n}"]))
                   for r in parts for n in threads if r[f"auto_{n}"]])
    lib.write_csv(ctx, "performance_wasm.csv",
                  ["example", "unknowns", "native_us_per_step", "wasm_us_per_step", "ratio"],
                  [(r["example"], int(r["unknowns"]), number(r["native_us_per_step"]),
                    number(r["wasm_us_per_step"]), x) for r, x in zip(wasm, ratio)])
    lib.write_csv(ctx, "performance_converted.csv",
                  ["network", "model", "machines", "converters", "unknowns", "steady_s_per_simulated_s",
                   "during_change_s_per_simulated_s"],
                  [(r["network"], r["model"], int(r["machines"]), int(r["converters"]), int(r["unknowns"]),
                    number(r["steady_s"]), number(r["during_change_s"])) for r in converted])

    fig, (ax1, ax2, ax3) = style.figure(3, 1, width="double", height_in=3.0, gridspec_kw={"width_ratios": [1, 1, 0.8]})
    ax1.loglog(unknowns, sparse, label="Sparse LU", **style.series(0))
    keep = ~np.isnan(dense)
    ax1.loglog(unknowns[keep], dense[keep], label="Dense LU", **style.series(1))
    ax1.loglog(unknowns, sparse[0] * 0.6 * unknowns / unknowns[0], label="Slope 1", **style.reference(dotted=True))
    ax1.loglog(unknowns[keep], dense[0] * 1.8 * (unknowns[keep] / unknowns[0]) ** 2, label="Slope 2",
               **style.reference())
    ax1.set_xlabel("Unknowns")
    ax1.set_ylabel("Time per step (µs)")
    style.legend_below(ax1)
    style.panel(ax1, "a")
    labels = {"ring of 300 buses": "Ring, 300 buses", "ring of 1000 buses": "Ring, 1000 buses",
              "16 blocks of 30 buses": "16 blocks of 30", "8 blocks of 100 buses": "8 blocks of 100"}
    for k, name in enumerate(chosen):
        ax2.plot(threads, speedup[name], label=labels[name], **style.series(k))
    ax2.set_xscale("log", base=2)
    ax2.set_xticks(threads)
    ax2.set_xticklabels([str(n) for n in threads])
    ax2.minorticks_off()
    ax2.set_xlabel("Threads")
    ax2.set_ylabel("Speed-up against undivided")
    style.legend_below(ax2, ncols=2)
    style.panel(ax2, "b")
    y = np.arange(len(wasm))[::-1]
    ax3.barh(y, ratio, height=0.55, color=style.COLORS[0])
    for yy, x in zip(y, ratio):
        ax3.text(x + 0.03, yy, f"{x:.2f}", va="center", fontsize=style.STYLE["legend_font_size_pt"])
    ax3.axvline(1.0, **style.reference(dotted=True))
    ax3.set_yticks(y)
    ax3.set_yticklabels([r["example"].replace("_", " ") for r in wasm])
    ax3.set_xlim(0, 2.4)
    ax3.grid(axis="y", visible=False)
    ax3.set_xlabel("WebAssembly / native")
    style.panel(ax3, "c")
    lib.save_figure(ctx, fig, "performance")

    slope = math.log(sparse[-1] / sparse[0]) / math.log(unknowns[-1] / unknowns[0])
    k_dense = [k for k in range(len(dense)) if not math.isnan(dense[k])]
    slope_dense = math.log(dense[k_dense[-1]] / dense[k_dense[0]]) / math.log(
        unknowns[k_dense[-1]] / unknowns[k_dense[0]])
    by_unknowns = {int(r["unknowns"]): float(s) for r, s in zip(rings, sparse)}
    out = {
        "source": "docs/performance.md, recorded 2026-11-22 on one pinned fast core (machine named there)",
        "sparse LU: us per step at 51, 483, 4803 unknowns": [by_unknowns[u] for u in (51, 483, 4803)],
        "sparse LU: ns per unknown and step, smallest and largest ring": [
            round(float(1e3 * sparse[0] / unknowns[0]), 1), round(float(1e3 * sparse[-1] / unknowns[-1]), 1)],
        "sparse LU: exponent of the growth from 51 to 4803 unknowns": round(slope, 2),
        "dense LU: exponent of the growth from 51 to 1443 unknowns": round(slope_dense, 2),
        "speed-up with 8 threads against the undivided circuit": {k: round(v[3], 2) for k, v in speedup.items()},
        "speed-up with 1 thread (divided against undivided)": {k: round(v[0], 2) for k, v in speedup.items()},
        "WebAssembly / native: range": [round(min(ratio), 2), round(max(ratio), 2)],
        "converted networks, during a change, s per simulated s (machines and converters)": {
            r["network"]: number(r["during_change_s"]) for r in converted
            if r["model"] == "machines and converters"},
    }
    if ctx.measure:
        out.update(measure(ctx))
    return out
