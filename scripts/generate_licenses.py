"""Generate and verify THIRD-PARTY-LICENSES from Cargo.lock metadata."""

from __future__ import annotations

import argparse
import difflib
import json
import subprocess
import sys
from collections import Counter
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_OUTPUT = REPO_ROOT / "THIRD-PARTY-LICENSES"


def _escape_cell(text: str) -> str:
    return text.replace("|", "\\|").replace("\r", " ").replace("\n", " ").strip()


def load_external_crates(repo_root: Path = REPO_ROOT) -> list[dict[str, Any]]:
    """Return external Rust crates from ``cargo metadata --format-version 1 --locked``."""
    proc = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        cwd=repo_root,
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=True,
    )
    metadata = json.loads(proc.stdout)
    workspace_ids = set(metadata.get("workspace_members", []))
    external = [pkg for pkg in metadata.get("packages", []) if pkg["id"] not in workspace_ids]
    external.sort(key=lambda p: (p["name"].lower(), p["name"], p["version"], p["id"]))
    return external


def render_licenses(external: list[dict[str, Any]]) -> str:
    """Render deterministic Markdown content for THIRD-PARTY-LICENSES."""
    license_counts = Counter(
        pkg.get("license") or pkg.get("license_file") or "UNKNOWN" for pkg in external
    )

    lines: list[str] = [
        "# Third-Party Licenses",
        "",
        "This file is generated deterministically from `Cargo.lock` via",
        "`cargo metadata --format-version 1 --locked` by",
        "`scripts/generate_licenses.py`. Do not edit it by hand.",
        "",
        "## Scope",
        "",
        f"- **Rust crates (`Cargo.lock`)**: {len(external)} external crates resolved by the",
        "  workspace, checked against `deny.toml` (`cargo deny check licenses sources bans`).",
        "- **Web browser page (`web/`)**: **0 external JavaScript dependencies**.",
        "  The browser interface uses plain HTML, CSS, and ES modules with no runtime or",
        "  build-time package dependencies (`web/package.json` declares `\"type\": \"module\"`",
        "  for Node.js tooling only).",
        "- **Python package (`python/`, `pyproject.toml`)**: declares `numpy>=1.24` at",
        "  runtime and optional extras (`pandas`, `pandapower`, `powsybl`, `mcp`, `test`,",
        "  `lint`, `paper`). None of those packages are vendored in this repository; they are",
        "  installed separately by the user under their own licenses when selected.",
        "",
        "## License summary (Rust external crates)",
        "",
        "| SPDX License | Crates |",
        "|---|---|",
    ]
    for spdx, count in sorted(license_counts.items()):
        lines.append(f"| `{_escape_cell(spdx)}` | {count} |")

    lines.extend(
        [
            "",
            f"## Rust external crates ({len(external)})",
            "",
            "| Crate | Version | License | Repository | Authors |",
            "|---|---|---|---|---|",
        ]
    )

    for pkg in external:
        name = _escape_cell(pkg["name"])
        version = _escape_cell(pkg["version"])
        spdx = _escape_cell(pkg.get("license") or pkg.get("license_file") or "UNKNOWN")
        repo = _escape_cell(pkg.get("repository") or pkg.get("homepage") or "(none)")
        authors_list = pkg.get("authors") or []
        authors = _escape_cell(", ".join(authors_list)) if authors_list else "(not specified)"
        lines.append(f"| `{name}` | `{version}` | `{spdx}` | {repo} | {authors} |")

    lines.append("")
    return "\n".join(lines)


def generate_text(repo_root: Path = REPO_ROOT) -> str:
    """Generate the full THIRD-PARTY-LICENSES text from the repository."""
    return render_licenses(load_external_crates(repo_root))


def check_licenses(
    repo_root: Path = REPO_ROOT, output_path: Path | None = None
) -> tuple[bool, str]:
    """Verify that the committed THIRD-PARTY-LICENSES matches cargo metadata."""
    target = output_path or (repo_root / "THIRD-PARTY-LICENSES")
    if not target.exists():
        return False, f"missing {target.relative_to(repo_root).as_posix()}; run scripts/generate_licenses.py"
    expected = generate_text(repo_root)
    actual = target.read_text(encoding="utf-8").replace("\r\n", "\n")
    if actual == expected:
        count = len(load_external_crates(repo_root))
        return True, f"THIRD-PARTY-LICENSES is up to date ({count} external Rust crates)."
    diff = "".join(
        difflib.unified_diff(
            actual.splitlines(keepends=True),
            expected.splitlines(keepends=True),
            fromfile="THIRD-PARTY-LICENSES (current)",
            tofile="THIRD-PARTY-LICENSES (expected)",
            n=2,
        )
    )
    return False, f"THIRD-PARTY-LICENSES differs from cargo metadata:\n{diff}"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="Exit non-zero if THIRD-PARTY-LICENSES is missing or out of date.",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=DEFAULT_OUTPUT,
        help="Output path (default: THIRD-PARTY-LICENSES at repository root).",
    )
    args = parser.parse_args(argv)

    try:
        if args.check:
            ok, msg = check_licenses(REPO_ROOT, args.output)
            print(msg, file=sys.stdout if ok else sys.stderr)
            return 0 if ok else 1

        content = generate_text(REPO_ROOT)
    except FileNotFoundError:
        print("error: `cargo` executable not found on PATH", file=sys.stderr)
        return 1
    except subprocess.CalledProcessError as exc:
        print(f"error: cargo metadata failed ({exc.returncode}): {exc.stderr}", file=sys.stderr)
        return 1

    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("w", encoding="utf-8", newline="\n") as fh:
        fh.write(content)
    print(f"Wrote {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
