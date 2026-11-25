"""Fixtures of the tests of the MCP server (the modules test_mcp*)."""

from __future__ import annotations

import pytest


@pytest.fixture(autouse=True)
def mcp_root(request, tmp_path, monkeypatch):
    """The MCP server reads and writes only under its root. In the tests of the
    server, the root of ``server()`` is the test's temporary folder."""
    if request.module.__name__.startswith("test_mcp"):
        monkeypatch.setenv("OPENKEITO_MCP_ROOT", str(tmp_path))
