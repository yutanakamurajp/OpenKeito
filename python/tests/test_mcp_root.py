"""The MCP server reads and writes only under its root: every tool and every
argument that is a path, the ways around the limit one by one, the switch that
lifts it, and the command line over stdio.

Every test works in its temporary folder: ``root`` is the root of the server
and ``outside`` is a folder next to it that no tool may touch."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

pytest.importorskip("mcp")
pytest.importorskip("pandapower")
pytest.importorskip("pypowsybl")

from mcp.client.stdio import StdioServerParameters  # noqa: E402

from openkeito.mcp_server import main, server  # noqa: E402

from test_mcp import EXAMPLES, INPUTS, call  # noqa: E402

WINDOWS = os.name == "nt"


class Folders:
    """The root with what the tools read, and the same files outside it."""

    def __init__(self, tmp_path: Path):
        import pandapower
        import pandapower.networks
        import pypowsybl.network

        self.root = tmp_path / "root"
        self.outside = tmp_path / "outside"
        for folder in (self.root, self.outside):
            folder.mkdir()
            for name in ("rlc.json", "three_phase_fault.json"):
                shutil.copy(EXAMPLES / name, folder / name)
            (folder / "inputs.json").write_text(json.dumps(INPUTS), encoding="utf-8")
            pandapower.to_json(pandapower.networks.case9(), str(folder / "net.json"))
            pypowsybl.network.create_ieee14().save(str(folder / "net.xiidm"), format="XIIDM")
            (folder / "result").mkdir()
            (folder / "result" / "result.csv").write_text("time_s,V(a)\n0.0,1.0\n0.1,2.0\n", encoding="utf-8")
        self.server = server(root=self.root)
        self.kept = self.listing()

    def listing(self) -> dict[str, bytes | None]:
        """Every file and folder outside the root, with the bytes of the files."""
        return {str(p.relative_to(self.outside)): (p.read_bytes() if p.is_file() else None)
                for p in sorted(self.outside.rglob("*"))}

    def call(self, name: str, arguments: dict) -> tuple[bool, object]:
        return call(name, arguments, target=self.server)

    def refused(self, name: str, arguments: dict, argument: str) -> str:
        """The message of a call that is refused for the path of ``argument``;
        nothing outside the root has changed."""
        error, text = self.call(name, arguments)
        assert error, (name, arguments, text)
        # Which path, where it leads, the root, and the three ways to mend it.
        assert f": {argument} {arguments[argument]!r} is outside the root folder" in text, text
        assert f"not under {self.root}." in text, text
        assert "Put it under" in text and "--root" in text and "--allow-any-path" in text, text
        assert self.listing() == self.kept
        return text


@pytest.fixture
def folders(tmp_path) -> Folders:
    return Folders(tmp_path)


def symlink(link: Path, target: Path) -> None:
    try:
        link.symlink_to(target, target_is_directory=target.is_dir())
    except (OSError, NotImplementedError) as e:
        pytest.skip(f"symbolic links cannot be made here: {e}")


def junction(link: Path, target: Path) -> None:
    if not WINDOWS:
        pytest.skip("junctions are of Windows")
    made = subprocess.run(["cmd", "/c", "mklink", "/J", str(link), str(target)], capture_output=True)
    if made.returncode != 0:
        pytest.skip("junctions cannot be made here")


#: Every argument of every tool that is a path: (tool, the arguments with paths
#: under the root, the argument that then points outside the root). `{o}` is
#: the folder outside the root.
PATH_ARGUMENTS = [
    ("validate_model", {"path": "rlc.json"}, "path"),
    ("get_topology", {"path": "rlc.json"}, "path"),
    ("run_model", {"path": "rlc.json", "out": "o1"}, "path"),
    ("run_model", {"path": "rlc.json", "out": "o1"}, "out"),
    ("scan_frequency", {"path": "three_phase_fault.json", "out": "o2"}, "path"),
    ("scan_frequency", {"path": "three_phase_fault.json", "out": "o2"}, "out"),
    ("query_results", {"out": "result"}, "out"),
    ("convert_network", {"source": "pandapower", "network": "net.json", "model_path": "m1.json"}, "network"),
    ("convert_network", {"source": "pandapower", "network": "net.json", "model_path": "m1.json"}, "model_path"),
    ("convert_network", {"source": "powsybl", "network": "net.xiidm", "model_path": "m2.json"}, "network"),
    ("convert_network", {"source": "powsybl", "network": "net.xiidm", "model_path": "m2.json"}, "model_path"),
    ("make_schedule", {"network": "net.json", "inputs_path": "inputs.json", "schedule_path": "p.json"}, "network"),
    ("make_schedule", {"network": "net.json", "inputs_path": "inputs.json", "schedule_path": "p.json"},
     "inputs_path"),
    ("make_schedule", {"network": "net.json", "inputs_path": "inputs.json", "schedule_path": "p.json"},
     "schedule_path"),
    *[(tool, {"source": source, "network": network, "schedule_path": "inputs.json", "out": "o3"}, argument)
      for tool in ("run_time_slices", "run_transitions")
      for source, network in (("pandapower", "net.json"), ("powsybl", "net.xiidm"))
      for argument in ("network", "schedule_path", "out")],
]

#: A file or folder outside the root for each argument (the same names as under the root).
OUTSIDE_NAMES = {"out": "result", "model_path": "new.json", "schedule_path": "new.json"}


@pytest.mark.parametrize("tool,arguments,argument", PATH_ARGUMENTS,
                         ids=[f"{t}-{a.get('source', '')}-{k}" for t, a, k in PATH_ARGUMENTS])
def test_every_path_argument_outside_the_root_is_refused(folders, tool, arguments, argument):
    name = OUTSIDE_NAMES.get(argument, arguments[argument])
    if tool in ("run_time_slices", "run_transitions") and argument == "schedule_path":
        name = "inputs.json"
    # As an absolute path, and as a relative one that climbs out of the root.
    for path in (str(folders.outside / name), f"../outside/{name}"):
        text = folders.refused(tool, {**arguments, argument: path}, argument)
        assert str(folders.outside / name) in text
    # Outputs that do not exist yet, with parents that do not exist either.
    if argument in ("out", "model_path", "schedule_path"):
        folders.refused(tool, {**arguments, argument: "new/deeper/../../../outside/new/deeper/x.json"}, argument)
        assert not (folders.root / "new").exists()
    assert sorted(p.name for p in folders.root.iterdir()) == sorted(
        ["inputs.json", "net.json", "net.xiidm", "result", "rlc.json", "three_phase_fault.json"])


def test_every_tool_works_under_the_root_with_relative_paths(folders, tmp_path, monkeypatch):
    # Relative paths are taken from the root, not from the working directory.
    monkeypatch.chdir(folders.outside)
    root = folders.root
    error, report = folders.call("validate_model", {"path": "rlc.json"})
    assert not error and report["valid"] is True
    error, summary = folders.call("run_model", {"path": "rlc.json", "out": "runs/new/rlc"})
    assert not error and summary["status"] == "completed"
    assert Path(summary["output_directory"]) == root / "runs" / "new" / "rlc"
    assert (root / "runs" / "new" / "rlc" / "result.csv").exists()
    error, q = folders.call("query_results", {"out": "runs/new/rlc", "max_rows": 5})
    assert not error and Path(q["file"]) == root / "runs" / "new" / "rlc" / "result.csv"
    error, summary = folders.call("scan_frequency", {"path": "./three_phase_fault.json", "out": "runs/scan"})
    assert not error and summary["status"] == "completed" and (root / "runs" / "scan" / "scan.csv").exists()
    # Absolute paths under the root, and ".." that stays under it.
    error, report = folders.call("validate_model", {"path": str(root / "runs" / ".." / "rlc.json")})
    assert not error and report["valid"] is True

    for source, network in (("pandapower", "net.json"), ("powsybl", "net.xiidm")):
        error, conv = folders.call("convert_network", {"source": source, "network": network,
                                                       "model_path": f"models/{source}.json",
                                                       "options": {"end_time_s": 0.005}})
        assert not error, conv
        assert Path(conv["model_path"]) == root / "models" / f"{source}.json"
        assert Path(conv["mapping_path"]) == root / "models" / f"{source}.mapping.json"
        assert (root / "models" / f"{source}.mapping.json").exists()
    error, plan = folders.call("make_schedule", {"network": "net.json", "inputs_path": "inputs.json",
                                                 "schedule_path": "plans/plan.json"})
    assert not error and Path(plan["schedule_path"]) == root / "plans" / "plan.json"
    error, run = folders.call("run_time_slices", {"source": "pandapower", "network": "net.json",
                                                  "schedule_path": "plans/plan.json", "out": "slices",
                                                  "options": {"end_time_s": 0.005}})
    assert not error and run["status"] == "completed", run
    assert Path(run["output_directory"]) == root / "slices" and (root / "slices" / "t001" / "result.csv").exists()
    error, run = folders.call("run_transitions", {
        "source": "pandapower", "network": "net.json", "schedule_path": "plans/plan.json", "out": "moves",
        "options": {"start_time_s": 0.02, "ramp_time_s": 0.02, "settle_time_s": 0.02}})
    assert not error and run["status"] == "completed", run
    assert Path(run["output_directory"]) == root / "moves" and (root / "moves" / "i000" / "summary.json").exists()
    # The root itself is under the root.
    error, text = folders.call("validate_model", {"path": "."})
    assert error and "cannot read the model file" in text
    assert folders.listing() == folders.kept


# The ways around the limit, one by one -------------------------------------------

def test_dot_dot_is_resolved_before_the_check(folders):
    for path in ("../outside/rlc.json", "runs/../../outside/rlc.json", "./.././outside/rlc.json",
                 "..\\outside\\rlc.json" if WINDOWS else "..//outside//rlc.json"):
        folders.refused("validate_model", {"path": path}, "path")
    # Far above the root, and back into it by another name than its own.
    folders.refused("validate_model", {"path": "../" * 40 + "rlc.json"}, "path")
    error, report = folders.call("validate_model", {"path": "../root/rlc.json"})
    assert not error and report["valid"] is True


def test_absolute_paths_outside_the_root_are_refused(folders):
    folders.refused("validate_model", {"path": str(folders.outside / "rlc.json")}, "path")
    folders.refused("validate_model", {"path": (folders.outside / "rlc.json").as_posix()}, "path")
    folders.refused("run_model", {"path": "rlc.json", "out": str(folders.outside / "new")}, "out")
    folders.refused("query_results", {"out": str(folders.outside / "result")}, "out")
    # The parent of the root, and a sibling whose name begins with the root's.
    folders.refused("run_model", {"path": "rlc.json", "out": str(folders.root.parent)}, "out")
    sibling = folders.root.parent / "root2"
    folders.refused("run_model", {"path": "rlc.json", "out": str(sibling)}, "out")
    assert not sibling.exists()


@pytest.mark.skipif(not WINDOWS, reason="drive letters and UNC paths are of Windows")
def test_drive_letters_unc_and_device_paths_are_refused(folders):
    outside = folders.outside / "rlc.json"
    drive, rest = os.path.splitdrive(str(outside))
    assert len(drive) == 2 and drive[1] == ":"
    paths = [
        f"\\\\?\\{outside}",  # the extended-length form
        f"//?/{outside}",
        f"\\\\.\\{outside}",  # the device form
        f"\\\\localhost\\{drive[0]}$" + rest,  # UNC: the administrative share of the same drive
        f"\\\\127.0.0.1\\{drive[0]}$" + rest,
        rest,  # rooted, without the drive: from the root of the current drive
        f"{drive}..\\outside\\rlc.json",  # relative to the working directory of the drive
        f"{drive.swapcase()}{rest}",
    ]
    other = next((f"{letter}:" for letter in "ZYXWVUTSRQ" if not os.path.exists(f"{letter}:\\")), None)
    if other is not None:
        paths += [f"{other}\\rlc.json", f"{other}rlc.json"]  # another drive, absolute and relative
    for path in paths:
        error, text = folders.call("validate_model", {"path": path})
        assert error and ("outside the root folder" in text or "not a path that can be used" in text), (path, text)
        error, text = folders.call("run_model", {"path": "rlc.json", "out": path + ".out"})
        assert error and ("outside the root folder" in text or "not a path that can be used" in text), (path, text)
    # Device names are not files under the root.
    for name in ("NUL", "CON", "nul.json", "COM1", "sub/Lpt1.txt", "aux /x.json", "con.a.b", "conin$"):
        for tool, arguments in (
                ("convert_network", {"source": "pandapower", "network": "case9", "model_path": name, "force": True}),
                ("run_model", {"path": "rlc.json", "out": name, "force": True}),
                ("validate_model", {"path": name})):
            error, text = folders.call(tool, arguments)
            assert error and "Windows device" in text and "--allow-any-path" in text, (name, tool, text)
    assert not (folders.root / "sub").exists()
    assert folders.listing() == folders.kept
    # The same forms of a path under the root are that path.
    inside = folders.root / "rlc.json"
    for path in (f"\\\\?\\{inside}", f"{drive.swapcase()}{os.path.splitdrive(str(inside))[1]}"):
        error, report = folders.call("validate_model", {"path": path})
        assert not error and report["valid"] is True, path


def test_upper_and_lower_case_do_not_get_around_the_root(folders):
    # Another spelling of the folder outside is still outside.
    for path in ("../OUTSIDE/rlc.json", str(folders.outside).upper() + "/rlc.json",
                 str(folders.outside).swapcase() + "/RLC.JSON"):
        error, text = folders.call("validate_model", {"path": path})
        assert error and ("outside the root folder" in text or "cannot read" in text), (path, text)
        assert "outside the root folder" in text or not WINDOWS, (path, text)
    # A folder whose name differs from the root's only in case is not the root, unless the
    # file system says that it is.
    other = folders.root.parent / "ROOT"
    same = other.exists()
    error, result = folders.call("validate_model", {"path": str(other / "rlc.json")})
    if same:
        assert os.path.samefile(other, folders.root)
        assert not error and result["valid"] is True
        error, result = folders.call("validate_model", {"path": str(folders.root).swapcase() + "/RLC.JSON"})
        assert not error and result["valid"] is True
        error, summary = folders.call("run_model", {"path": "RLC.json", "out": str(other / "New" / "Out")})
        assert not error and summary["status"] == "completed"
        assert (folders.root / "New" / "Out" / "result.csv").exists()
    else:
        assert error and "outside the root folder" in result
        other.mkdir()
        shutil.copy(folders.root / "rlc.json", other / "rlc.json")
        folders.refused("validate_model", {"path": str(other / "rlc.json")}, "path")
        shutil.rmtree(other)
    assert folders.listing() == folders.kept


def test_symbolic_links_to_outside_the_root_are_refused(folders):
    root, outside = folders.root, folders.outside
    symlink(root / "file_link.json", outside / "rlc.json")
    symlink(root / "folder_link", outside)
    symlink(root / "dangling", outside / "not_there")
    (root / "sub").mkdir()
    symlink(root / "sub" / "up", root.parent)
    folders.kept = folders.listing()

    text = folders.refused("validate_model", {"path": "file_link.json"}, "path")
    assert str(outside / "rlc.json") in text  # where the link leads
    folders.refused("validate_model", {"path": "folder_link/rlc.json"}, "path")
    folders.refused("validate_model", {"path": "sub/up/outside/rlc.json"}, "path")
    folders.refused("query_results", {"out": "folder_link/result"}, "out")
    folders.refused("convert_network", {"source": "pandapower", "network": "folder_link/net.json",
                                        "model_path": "m.json"}, "network")
    folders.refused("convert_network", {"source": "powsybl", "network": "folder_link/net.xiidm",
                                        "model_path": "m.json"}, "network")
    folders.refused("make_schedule", {"network": "case9", "inputs_path": "folder_link/inputs.json",
                                      "schedule_path": "p.json"}, "inputs_path")
    # Writing: through the link, into a folder below it that does not exist yet, and
    # through a link whose target does not exist yet.
    for out in ("folder_link", "folder_link/new/deeper", "dangling", "dangling/new", "sub/up/outside/new"):
        folders.refused("run_model", {"path": "rlc.json", "out": out}, "out")
        folders.refused("run_time_slices", {"source": "pandapower", "network": "case9",
                                            "schedule_path": "inputs.json", "out": out}, "out")
    for target in ("folder_link/m.json", "dangling", "file_link.json"):
        folders.refused("convert_network", {"source": "pandapower", "network": "case9", "model_path": target,
                                            "force": True}, "model_path")
        folders.refused("make_schedule", {"network": "case9", "inputs_path": "inputs.json",
                                          "schedule_path": target, "force": True}, "schedule_path")
    # A network that is a folder of files, one of which is a link.
    (root / "cgmes").mkdir()
    symlink(root / "cgmes" / "eq.xml", outside / "net.xiidm")
    error, text = folders.call("convert_network", {"source": "powsybl", "network": "cgmes", "model_path": "m.json"})
    assert error and "a link to" in text and str(outside / "net.xiidm") in text
    # The mapping next to the model is a link, the model is not.
    symlink(root / "m.mapping.json", outside / "new.mapping.json")
    error, text = folders.call("convert_network", {"source": "pandapower", "network": "case9",
                                                   "model_path": "m.json", "force": True})
    assert error and "outside the root folder" in text and "m.mapping.json" in text
    assert not (root / "m.json").exists() and folders.listing() == folders.kept

    # A link that stays under the root is followed.
    symlink(root / "inner.json", root / "rlc.json")
    symlink(root / "inner", root / "result")
    error, report = folders.call("validate_model", {"path": "inner.json"})
    assert not error and report["valid"] is True
    error, q = folders.call("query_results", {"out": "inner"})
    assert not error and Path(q["file"]) == root / "result" / "result.csv"


def test_junctions_to_outside_the_root_are_refused(folders):
    root, outside = folders.root, folders.outside
    junction(root / "junction", outside)
    junction(root / "inner", root / "result")
    folders.kept = folders.listing()
    text = folders.refused("validate_model", {"path": "junction/rlc.json"}, "path")
    assert str(outside / "rlc.json") in text
    folders.refused("query_results", {"out": "junction/result"}, "out")
    for out in ("junction", "junction/new/deeper", "junction\\result"):
        folders.refused("run_model", {"path": "rlc.json", "out": out, "force": True}, "out")
    folders.refused("convert_network", {"source": "pandapower", "network": "case9",
                                        "model_path": "junction/new.json"}, "model_path")
    error, q = folders.call("query_results", {"out": "inner"})
    assert not error and Path(q["file"]) == root / "result" / "result.csv"


def test_links_inside_an_output_directory_are_refused(folders):
    root, outside = folders.root, folders.outside
    victim = outside / "result" / "result.csv"
    # A result file that is a link: a run with force would write through it.
    (root / "out").mkdir()
    symlink(root / "out" / "result.csv", victim)
    folders.kept = folders.listing()
    for force in (False, True):
        error, text = folders.call("run_model", {"path": "rlc.json", "out": "out", "force": force})
        assert error and "a link to" in text and str(victim) in text and "--allow-any-path" in text, text
        assert str(root / "out" / "result.csv") in text and "a new directory" in text
        assert folders.listing() == folders.kept
    # Reading it back is refused too.
    folders.refused("query_results", {"out": "out"}, "out")
    # The directory of a case of a batch that is a link, and a file in one.
    (root / "batch").mkdir()
    symlink(root / "batch" / "t000", outside / "result")
    (root / "batch2" / "t000").mkdir(parents=True)
    symlink(root / "batch2" / "t000" / "summary.json", outside / "inputs.json")
    folders.kept = folders.listing()
    for out in ("batch", "batch2"):
        for tool in ("run_time_slices", "run_transitions"):
            error, text = folders.call(tool, {"source": "pandapower", "network": "case9",
                                              "schedule_path": "inputs.json", "out": out, "force": True,
                                              "options": {"end_time_s": 0.005}})
            assert error and "a link to" in text and str(outside) in text, text
            assert folders.listing() == folders.kept


def test_case_names_cannot_leave_the_output_directory(folders):
    # The name of a disturbance is part of the name of a case directory.
    for name in ("../../outside/x", "..", "a/b", "a\\b", str(folders.outside / "x")):
        error, text = folders.call("run_time_slices", {
            "source": "pandapower", "network": "case9", "schedule_path": "inputs.json", "out": "slices",
            "options": {"end_time_s": 0.005},
            "disturbances": [{"name": name, "components": []}]})
        assert error and "E402" in text, (name, text)
        assert folders.listing() == folders.kept
    assert not (folders.root / "x").exists() and not (folders.root.parent / "x").exists()


def test_paths_that_cannot_be_resolved_are_usage_mistakes(folders):
    error, text = folders.call("validate_model", {"path": "a\x00b.json"})
    assert error and "path" in text
    error, text = folders.call("run_model", {"path": "rlc.json", "out": "a\x00b"})
    assert error and "out" in text
    assert folders.listing() == folders.kept


# The root and the switch ---------------------------------------------------------------

def test_the_root_is_the_argument_the_variable_or_the_working_directory(folders, tmp_path, monkeypatch):
    def root_of(target) -> str:
        error, text = call("validate_model", {"path": str(tmp_path.parent / "nowhere" / "x.json")}, target=target)
        assert error and "outside the root folder" in text
        return text

    # conftest.py sets the variable to the temporary folder; the argument wins over it.
    assert f"not under {tmp_path}." in root_of(server())
    assert f"not under {folders.root}." in root_of(server(root=folders.root))
    assert f"not under {folders.root}." in root_of(server(root=str(folders.root / "result" / "..")))
    monkeypatch.setenv("OPENKEITO_MCP_ROOT", str(folders.outside))
    assert f"not under {folders.outside}." in root_of(server())
    for empty in (True, False):
        monkeypatch.setenv("OPENKEITO_MCP_ROOT", "") if empty else monkeypatch.delenv("OPENKEITO_MCP_ROOT")
        monkeypatch.chdir(folders.root / "result")
        assert f"not under {folders.root / 'result'}." in root_of(server())
    # The root is fixed when the server is made.
    made = server()
    monkeypatch.chdir(folders.outside)
    assert f"not under {folders.root / 'result'}." in root_of(made)
    # A root that is not a folder.
    for bad in (tmp_path / "nowhere", folders.root / "rlc.json"):
        with pytest.raises(ValueError, match="--root"):
            server(root=bad)
        monkeypatch.setenv("OPENKEITO_MCP_ROOT", str(bad))
        with pytest.raises(ValueError, match="OPENKEITO_MCP_ROOT"):
            server()


def test_a_root_that_is_a_link_is_the_folder_it_leads_to(folders, tmp_path):
    symlink(tmp_path / "root_link", folders.root)
    linked = server(root=tmp_path / "root_link")
    for path in ("rlc.json", str(folders.root / "rlc.json"), str(tmp_path / "root_link" / "rlc.json")):
        error, report = call("validate_model", {"path": path}, target=linked)
        assert not error and report["valid"] is True, path
    error, text = call("validate_model", {"path": "../outside/rlc.json"}, target=linked)
    assert error and "outside the root folder" in text and str(folders.root) in text


def test_the_instructions_name_the_root(folders):
    import asyncio

    from mcp.client import Client

    async def instructions(target) -> str:
        async with Client(target) as c:
            return c.instructions

    text = asyncio.run(instructions(folders.server))
    assert str(folders.root) in text and "refused" in text
    text = asyncio.run(instructions(server(root=folders.root, allow_any_path=True)))
    assert str(folders.root) in text and "--allow-any-path" in text


def test_allow_any_path_lifts_the_limit(folders):
    root, outside = folders.root, folders.outside
    free = server(root=root, allow_any_path=True)
    error, report = call("validate_model", {"path": "../outside/rlc.json"}, target=free)
    assert not error and report["valid"] is True
    error, summary = call("run_model", {"path": str(outside / "rlc.json"), "out": str(outside / "new" / "run")},
                          target=free)
    assert not error and summary["status"] == "completed" and (outside / "new" / "run" / "result.csv").exists()
    error, q = call("query_results", {"out": str(outside / "result")}, target=free)
    assert not error and q["rows_returned"] == 2
    error, conv = call("convert_network", {"source": "powsybl", "network": str(outside / "net.xiidm"),
                                           "model_path": str(outside / "new" / "m.json"),
                                           "options": {"end_time_s": 0.005}}, target=free)
    assert not error and (outside / "new" / "m.mapping.json").exists()
    error, plan = call("make_schedule", {"network": str(outside / "net.json"),
                                         "inputs_path": str(outside / "inputs.json"),
                                         "schedule_path": str(outside / "new" / "plan.json")}, target=free)
    assert not error and plan["times"] == 2
    # Relative paths are still taken from the root.
    error, summary = call("run_model", {"path": "rlc.json", "out": "rel"}, target=free)
    assert not error and (root / "rel" / "result.csv").exists()
    # The same calls without the switch are refused.
    folders.kept = folders.listing()
    folders.refused("run_model", {"path": "rlc.json", "out": str(outside / "new" / "run2")}, "out")


# The command -----------------------------------------------------------------------------

def test_the_command_takes_the_root_over_stdio(folders, tmp_path):
    def params(*args: str, **more) -> StdioServerParameters:
        return StdioServerParameters(command=sys.executable, args=["-m", "openkeito.mcp_server", *args], **more)

    environment = {k: v for k, v in os.environ.items() if k != "OPENKEITO_MCP_ROOT"}
    outside = str(folders.outside / "rlc.json")
    # --root: relative paths from it, and nothing outside it.
    target = params("--root", str(folders.root), cwd=str(folders.outside), env=environment)
    error, summary = call("run_model", {"path": "rlc.json", "out": "結果/stdio"}, target=target)
    assert not error and summary["status"] == "completed"
    assert Path(summary["output_directory"]) == folders.root / "結果" / "stdio"
    for arguments in ({"path": outside, "out": "x"}, {"path": "rlc.json", "out": str(folders.outside / "x")},
                      {"path": "../outside/rlc.json", "out": "x"}):
        error, text = call("run_model", arguments, target=target)
        assert error and "outside the root folder" in text and str(folders.root) in text
    assert not (folders.root / "x").exists() and folders.listing() == folders.kept
    # The variable; --root wins over it.
    target = params(cwd=str(folders.outside), env={**environment, "OPENKEITO_MCP_ROOT": str(folders.root)})
    error, text = call("validate_model", {"path": outside}, target=target)
    assert error and f"not under {folders.root}." in text
    error, report = call("validate_model", {"path": "rlc.json"}, target=target)
    assert not error and report["valid"] is True
    target = params("--root", str(folders.root), cwd=str(folders.outside),
                    env={**environment, "OPENKEITO_MCP_ROOT": str(folders.outside)})
    error, text = call("validate_model", {"path": outside}, target=target)
    assert error and f"not under {folders.root}." in text
    # Neither: the working directory in which the server starts.
    target = params(cwd=str(folders.root), env=environment)
    error, text = call("validate_model", {"path": outside}, target=target)
    assert error and f"not under {folders.root}." in text
    error, report = call("validate_model", {"path": "rlc.json"}, target=target)
    assert not error and report["valid"] is True
    # --allow-any-path.
    target = params("--root", str(folders.root), "--allow-any-path", cwd=str(folders.outside), env=environment)
    error, report = call("validate_model", {"path": outside}, target=target)
    assert not error and report["valid"] is True
    assert folders.listing() == folders.kept


def test_the_command_says_on_standard_error_that_the_limit_is_lifted(folders, tmp_path):
    environment = {k: v for k, v in os.environ.items() if k != "OPENKEITO_MCP_ROOT"}

    def start(*args: str) -> subprocess.CompletedProcess:
        # The input ends at once, so the server stops after starting.
        return subprocess.run([sys.executable, "-m", "openkeito.mcp_server", *args], stdin=subprocess.DEVNULL,
                              capture_output=True, timeout=120, cwd=str(folders.root), env=environment)

    free = start("--allow-any-path")
    lines = [line for line in free.stderr.decode("utf-8", "replace").splitlines() if "--allow-any-path" in line]
    assert len(lines) == 1 and lines[0].startswith("openkeito-mcp: warning:") and free.stdout == b""
    limited = start("--root", str(folders.root))
    assert b"allow-any-path" not in limited.stderr and limited.stdout == b""
    # A root that is not a folder stops the command with what to change.
    bad = start("--root", str(tmp_path / "nowhere"))
    assert bad.returncode == 2 and b"--root" in bad.stderr and b"nowhere" in bad.stderr and bad.stdout == b""
    helped = start("--help")
    assert helped.returncode == 0 and b"--root" in helped.stdout and b"--allow-any-path" in helped.stdout


def test_main_refuses_a_bad_root_before_serving(tmp_path, capsys):
    with pytest.raises(SystemExit) as stopped:
        main(["--root", str(tmp_path / "nowhere")])
    assert stopped.value.code == 2 and "nowhere" in capsys.readouterr().err


def test_query_results_refusal_reports_caller_argument_name(folders, monkeypatch):
    # Exercises the same refusal message path as test_links_inside_an_output_directory_are_refused
    # even on systems where symbolic links cannot be created.
    out = folders.root / "out"
    out.mkdir()
    result_file = out / "result.csv"
    result_file.write_text("time_s,V(a)\n0.0,1.0\n", encoding="utf-8")
    outside_target = folders.outside / "result" / "result.csv"
    orig_resolve = Path.resolve

    def fake_resolve(self, strict=False):
        if self == result_file:
            return outside_target
        return orig_resolve(self, strict=strict)

    monkeypatch.setattr(Path, "resolve", fake_resolve)
    folders.refused("query_results", {"out": "out"}, "out")
