"""paper/make.py runs to its end with the small settings (--quick) and the
main numbers of its CSV files are where the full run and the Rust tests put
them. The bounds are wide: --quick uses fewer and larger time steps than the
full run, and the test is about the scripts, not about the solver."""

from __future__ import annotations

import csv
import importlib.util
import json
import math
import os
import subprocess
import sys
from pathlib import Path

import numpy as np
import pytest

pytest.importorskip("matplotlib")

MAKE = Path(__file__).resolve().parents[2] / "paper" / "make.py"
LIB = MAKE.parent / "lib.py"


def rows(path: Path) -> list[dict[str, str]]:
    with open(path, encoding="utf-8", newline="") as f:
        return list(csv.DictReader(f))


@pytest.fixture(scope="module")
def out(tmp_path_factory) -> Path:
    directory = tmp_path_factory.mktemp("paper")
    done = subprocess.run([sys.executable, str(MAKE), "--quick", "--out", str(directory)], capture_output=True,
                          text=True, encoding="utf-8", errors="replace",
                          env={**os.environ, "PYTHONIOENCODING": "utf-8"})
    assert done.returncode == 0, done.stdout[-2000:] + done.stderr[-4000:]
    return directory


def test_every_item_wrote_its_files(out):
    numbers = json.loads((out / "numbers.json").read_text(encoding="utf-8"))
    listed = subprocess.run([sys.executable, str(MAKE), "--list"], capture_output=True,
                            text=True, encoding="utf-8", check=True)
    items = [line.split(":", 1)[0] for line in listed.stdout.splitlines() if line.strip()]
    assert numbers.get("_settings") == {"quick": True}
    assert [k for k in numbers if k != "_settings"] == items
    for name in ("integration_order", "numerical_oscillation", "buck_switching", "diode_bridge", "fault_currents",
                 "steady_state", "line_models", "machine_short_circuit", "converter_response", "controls_v07",
                 "wecc_ibr", "performance"):
        for suffix in ("pdf", "png"):
            assert (out / f"{name}.{suffix}").stat().st_size > 1000, name
    for name in ("fault_currents_table", "controls_v07_table", "diagnostics_table"):
        assert (out / f"{name}.tex").read_text(encoding="utf-8").count("\\\\") > 3
    # Run times are only written on request.
    assert not list(out.glob("*_measured.csv"))


def test_the_methods_are_of_second_order(out):
    for r in rows(out / "integration_order.csv"):
        if r["order_v_c"] != "nan":
            assert 1.9 < float(r["order_v_c"]) < 2.1, r
    by_step = {(r["method"], r["time_step_s"]): float(r["max_error_v_c_v"]) for r in rows(out / "integration_order.csv")}
    assert by_step[("2s-dirk", "1e-05")] == pytest.approx(2.98e-3, rel=0.02)


def test_the_trapezoidal_rule_oscillates_and_2s_dirk_does_not(out):
    table = rows(out / "numerical_oscillation.csv")
    after = [r for r in table if float(r["time_s"]) > 2e-3]
    assert max(abs(float(r["v_l_2s_dirk_v"])) for r in after) < 1e-6
    assert min(abs(float(r["v_l_trapezoidal_v"])) for r in after) > 1.0
    assert max(abs(float(r["v_l_exact_v"])) for r in after) < 1e-12


def test_switching_within_the_step_removes_the_first_order_error(out):
    table = {(r["switching"], r["method"], r["time_step_s"]): float(r["max_error_i_l_a"])
             for r in rows(out / "buck_switching.csv")}
    # 0.60 A at grid points and 6.0e-6 A within the step at 2 us.
    assert 0.3 < table[("grid", "2s-dirk", "2e-06")] < 1.0
    assert table[("interpolated", "2s-dirk", "2e-06")] < 1e-5
    ratio = table[("interpolated", "2s-dirk", "1e-05")] / table[("interpolated", "2s-dirk", "5e-06")]
    assert 3.8 < ratio < 4.2


def test_three_phase_numbers(out):
    for r in rows(out / "fault_currents.csv"):
        w_h = 2.0 * math.pi * 50.0 * float(r["time_step_s"])
        bound = (w_h ** 2) / 12.0
        limit = 1.05 * bound if r["method"] == "trapezoidal" else 0.5 * bound
        assert float(r["relative_error"]) <= limit, r
    for r in rows(out / "steady_state.csv"):
        assert float(r["relative_voltage_deviation"]) < float(r["omega_h_squared"]), r
    lattice = rows(out / "line_lattice.csv")
    assert max(abs(float(r["v_load_end_2s_dirk_v"]) - float(r["v_load_end_lattice_v"])) for r in lattice) < 1e-10
    sections = rows(out / "line_pi_sections.csv")
    assert 3.9 < float(sections[-1]["ratio"]) < 4.1


def test_machine_and_converter(out):
    machine = {r["time_step_s"]: float(r["max_error_phase_current_pu"]) for r in rows(out / "machine_short_circuit.csv")
               if r["method"] == "2s-dirk"}
    # 2.0e-4 pu at 100 us in the full run (0.21 s); the quick run is shorter.
    assert machine["0.0001"] < 3e-4 and 3.0 < machine["0.0001"] / machine["5e-05"] < 5.0
    step = rows(out / "converter_step.csv")
    assert float(step[0]["max_error_relative_to_the_step"]) < 0.08
    fault = rows(out / "converter_fault.csv")
    during = [float(r["reference_magnitude_pu"]) for r in fault if 0.15 < float(r["time_s"]) < 0.25]
    assert max(abs(x - 1.1) for x in during) < 1e-9


def test_controls_v07_and_wecc_ibr(out):
    pwl = rows(out / "controls_v07_pwl.csv")
    assert max(abs(float(r["pwl"]) - float(r["pwl_exact"])) for r in pwl) < 1e-12
    assert max(abs(float(r["limiter"]) - float(r["limiter_exact"])) for r in pwl) < 1e-12

    conv = rows(out / "controls_v07_convergence.csv")
    for col in ("lag_order", "pi_order", "integrator_order"):
        orders = [float(r[col]) for r in conv if r[col] != "nan"]
        assert orders and all(1.9 < o < 2.1 for o in orders), (col, orders)

    aw = rows(out / "controls_v07_anti_windup.csv")
    assert max(abs(float(r["pi_state"]) - float(r["block_decomposition"])) for r in aw) < 1e-12
    after = [r for r in aw if float(r["time_s"]) > 0.015]
    assert max(abs(float(r["pi_state"]) - float(r["pi_conditional"])) for r in after) > 0.5

    ex = rows(out / "controls_v07_exclude.csv")
    assert abs(float(ex[0]["i_shunt_excluded_a"])) < 1e-12
    assert abs(float(ex[0]["i_shunt_included_a"])) > 1.0

    ibr = rows(out / "wecc_ibr.csv")
    vt0 = float(ibr[0]["vt_pu"])
    pre = [r for r in ibr if float(r["time_s"]) < 0.2 - 1e-9]
    fault = [r for r in ibr if 0.2 <= float(r["time_s"]) <= 0.3]
    post = [r for r in ibr if float(r["time_s"]) > 0.32]
    assert max(abs(float(r["vt_pu"]) - vt0) for r in pre) < 1e-3
    assert max(float(r["iq_gen"]) for r in fault) > 0.4
    assert min(float(r["ip_gen"]) for r in fault) < 0.6
    assert max(float(r["vt_pu"]) for r in post) > 0.9

    numbers = json.loads((out / "numbers.json").read_text(encoding="utf-8"))
    assert numbers["controls_v07"]["piecewise_linear max error vs np.interp"] < 1e-12
    assert numbers["controls_v07"]["initialization exclude"]["excluded_i_t0_a"] < 1e-12
    assert numbers["wecc_ibr"]["fault peak iq_gen (pu)"] > 0.4


def test_power_flow_and_schedule(out):
    pytest.importorskip("pandapower")
    for r in rows(out / "power_flow_start.csv"):
        assert float(r["phasor_max_dv_pu"]) < 1e-6 and float(r["waveform_max_dv_pu"]) < 1e-3, r
    numbers = json.loads((out / "numbers.json").read_text(encoding="utf-8"))["schedule_runs"]
    assert numbers["time slices: 1 thread and 4 threads identical bit for bit"] is True
    assert numbers["transitions: 1 thread and 2 threads identical bit for bit"] is True
    for r in rows(out / "schedule_transitions.csv"):
        assert r["status"] == "completed" and float(r["max_abs_dv_pu"]) < 1e-8


def test_diagnostics_have_a_code_a_place_and_a_hint(out):
    table = rows(out / "diagnostics.csv")
    assert {"E001", "E111", "E130", "E121", "E150", "E142", "E230", "E280", "W290"} <= {r["code"] for r in table}
    assert all(r["path"] and r["message"] and r["hint"] for r in table)


def test_lib_helpers():
    spec = importlib.util.spec_from_file_location("_paper_lib", LIB)
    assert spec is not None and spec.loader is not None
    lib = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = lib
    try:
        spec.loader.exec_module(lib)
    finally:
        sys.modules.pop(spec.name, None)

    # expm2: distinct real eigenvalues, complex conjugate, and repeated roots
    t = 0.2
    a_real = np.array([[-1.0, 0.5], [0.0, -3.0]])
    expected_real = np.array([[math.exp(-t), 0.25 * (math.exp(-t) - math.exp(-3 * t))], [0.0, math.exp(-3 * t)]])
    assert np.allclose(lib.expm2(a_real, t), expected_real, rtol=1e-12, atol=1e-12)

    w = 3.0
    a_osc = np.array([[0.0, 1.0], [-w * w, 0.0]])
    expected_osc = np.array([[math.cos(w * t), math.sin(w * t) / w], [-w * math.sin(w * t), math.cos(w * t)]])
    assert np.allclose(lib.expm2(a_osc, t), expected_osc, rtol=1e-12, atol=1e-12)

    a_rep = np.array([[-2.0, 1.0], [0.0, -2.0]])
    expected_rep = math.exp(-2 * t) * np.array([[1.0, t], [0.0, 1.0]])
    assert np.allclose(lib.expm2(a_rep, t), expected_rep, rtol=1e-12, atol=1e-12)

    # phasor & fitted_phasor
    f = 50.0
    h = 1e-4
    ts = np.arange(0.0, 0.04 + h / 2, h)
    amp, phi = 120.0, math.radians(35.0)
    xs = amp * np.cos(2 * math.pi * f * ts + phi)
    ref = lib.polar(amp / math.sqrt(2.0), 35.0)
    assert abs(lib.phasor(ts, xs, f) - ref) < 1e-10
    assert abs(lib.fitted_phasor(ts, xs, f) - ref) < 1e-10

    # tex_escape & fmt
    assert lib.tex_escape(r"\a&b%c#d_e{f}g^h~i$j") == (
        r"\textbackslash{}a\&b\%c\#d\_e\{f\}g\^{}h\~{}i\$j"
    )
    assert lib.fmt(True) == "true"
    assert lib.fmt("a,b") == '"a,b"'
    assert lib.fmt('a"b') == '"a""b"'
    assert lib.fmt(None) == ""


def test_quick_repeatability_and_settings_guard(tmp_path):
    env = {**os.environ, "PYTHONIOENCODING": "utf-8"}
    d1, d2 = tmp_path / "run1", tmp_path / "run2"
    for d in (d1, d2):
        done = subprocess.run(
            [sys.executable, str(MAKE), "--quick", "--only", "diagnostics", "--only", "performance",
             "--only", "controls_v07", "--only", "wecc_ibr", "--out", str(d)],
            capture_output=True, text=True, encoding="utf-8", errors="replace", env=env,
        )
        assert done.returncode == 0, done.stdout[-2000:] + done.stderr[-4000:]
    files1 = sorted(p.name for p in d1.iterdir())
    files2 = sorted(p.name for p in d2.iterdir())
    assert files1 == files2
    for name in files1:
        assert (d1 / name).read_bytes() == (d2 / name).read_bytes(), name

    mismatch = subprocess.run(
        [sys.executable, str(MAKE), "--only", "diagnostics", "--out", str(d1)],
        capture_output=True, text=True, encoding="utf-8", errors="replace", env=env,
    )
    assert mismatch.returncode != 0
