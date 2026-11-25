"""Tests for third-party license notice generation and checks."""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
GENERATE_LICENSES = REPO_ROOT / "scripts" / "generate_licenses.py"


def test_generate_licenses_check():
    assert GENERATE_LICENSES.exists(), "scripts/generate_licenses.py must be present"
    if shutil.which("cargo") is None:
        pytest.skip("cargo executable not found on PATH")
    proc = subprocess.run(
        [sys.executable, str(GENERATE_LICENSES), "--check"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    assert proc.returncode == 0, f"stdout:\n{proc.stdout}\nstderr:\n{proc.stderr}"
    assert "up to date" in proc.stdout
