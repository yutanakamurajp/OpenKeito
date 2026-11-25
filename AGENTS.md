# OpenKeito agent guide

OpenKeito is an open EMT simulator. Read [README.md](README.md) for an overview,
[docs/model.md](docs/model.md) for the model format and conventions,
[docs/python.md](docs/python.md) and [docs/mcp.md](docs/mcp.md) for the Python package and
the MCP server, and [docs/numerical-method.md](docs/numerical-method.md) for the solver.
[CONTRIBUTING.md](CONTRIBUTING.md) says the same as "Changing the code" below for human
contributors.

## Using the tool

- `openkeito schema` and `openkeito components` describe the model format; read them instead
  of guessing field names.
- `openkeito topology <model>` prints a model's connections (components, terminals, nodes,
  buses, control signals and diagnostics) as JSON.
- Run `openkeito validate <model>` before `run`. Fix every diagnostic by its `hint`.
- Use a new `--out` directory for every run. Read `summary.json`: require
  `status == "completed"`, then check `events` and `warnings` before interpreting the CSV.
- Exit code 0 is necessary but not sufficient; the summary decides.
- For power electronics (PWM, rectifiers) set `"switching": "interpolated"` in `simulation`;
  the default `grid` moves every switching instant to a grid point.
- Before running a binary from `target/`, check `openkeito --version` against `Cargo.toml`:
  `target/release` keeps an old build until `cargo build --release` is run again.
- The MCP server (`openkeito-mcp`) reads and writes only under its root folder; give paths
  relative to it.

## Changing the code

- Crates: `openkeito-model` (format, validation, catalog), `openkeito-core` (solver),
  `openkeito-cli` (the command and the library behind it: topology, run, scan, batch; its feature
  `native`, on by default, holds the command and `batch`), `openkeito-py` (the native
  module `openkeito._native`) and `openkeito-wasm` (the WebAssembly module, which uses
  `openkeito-cli` without `native`). The Python package is `python/openkeito`: the run API,
  `schedule`, the converters `pandapower` and `powsybl` (with `convert` in common),
  `transition` (from one schedule time to the next, or through several) and the MCP server
  `mcp_server` (command `openkeito-mcp`).
- `web/` is the browser page on `openkeito-wasm` (plain HTML, CSS and ES modules, no
  dependencies; [web/README.md](web/README.md)). `paper/` holds the scripts that reproduce
  figures and tables (`python paper/make.py`; [paper/README.md](paper/README.md)).
- Before committing: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`. When Python code or the bindings change, also build the package
  into a virtual environment (`maturin develop --release`, or
  `pip install ".[test,lint,pandapower,powsybl,mcp,paper]"`) and run
  `python -m pyflakes python paper` and `python -m pytest`. Keep the virtual environment in
  `.venv` (not in Git); build artifacts such as `_native.pyd` and `.pdb` stay out of Git
  (`.gitignore`).
- When `openkeito-wasm`, the library of `openkeito-cli`, the solver or an example changes,
  also build the module and compare it with the command
  ([crates/openkeito-wasm/README.md](crates/openkeito-wasm/README.md), "Build" and "Test"):

  ```sh
  cargo build --release -p openkeito-wasm --target wasm32-unknown-unknown
  cargo build --release -p openkeito-cli
  wasm-bindgen --target web --out-dir crates/openkeito-wasm/pkg target/wasm32-unknown-unknown/release/openkeito_wasm.wasm
  node crates/openkeito-wasm/tests/compare.mjs
  ```

  When `web/` or an example changes, also run `node --test web/tests/modules.test.mjs` and
  `node web/build.mjs` (a new example must be listed in `web/examples.json`). Both load the
  built WebAssembly module in `crates/openkeito-wasm/pkg/`, so run the `openkeito-wasm`
  build and `wasm-bindgen` steps above first when that directory is missing or out of date.
- When a dependency changes: `cargo deny check licenses sources bans` (`deny.toml`). Every dependency must carry a license that MIT OR Apache-2.0 allows.
- The workflow `.github/workflows/ci.yml` runs on pushes and pull requests (Windows, macOS,
  Linux, wasm, Python and licenses), and can also be started by hand
  (`gh workflow run CI --ref <branch>`).
- A new component needs: the model structure with units in the field names and doc
  comments, validation, a catalog entry with an example, the element in `openkeito-core`, and a
  test against a closed-form solution.
- A new diagnostic needs a new code and a hint that says what to change; add its range to
  the table in [docs/model.md](docs/model.md#diagnostics).
- Keep the documents true: numbers in them come from tests or recorded measurements, and a
  change of behaviour changes the page that describes it in the same commit.
- Follow [docs/clean-room.md](docs/clean-room.md): published literature only; no code,
  formats or output of proprietary simulators; no client or person names.
- Commit messages follow Conventional Commits 1.0.0: type, scope and footer tokens in
  English, description and body in Japanese. Scopes: `model`, `core`, `cli`, `py`, `mcp`,
  `wasm`, `web`.

## Version control

- Files are stored with LF line endings (`.gitattributes`).
- Never put files of a proprietary simulator under Git
  ([docs/clean-room.md](docs/clean-room.md)).

## Versions and releases

- The software follows SemVer 2.0.0. The single source of the version is `version` in
  `[workspace.package]` of `Cargo.toml`; every crate inherits it, and `openkeito --version`
  and `summary.json` report it.
- The `version` field of the model format (now `"0.7"`) is separate from the software
  version. Change it only when the format changes, and say so in the CHANGELOG.
- To release: take the commits since the last tag and choose the next version (while
  0.y.z, a breaking change or `feat` raises y, `fix` or `perf` raises z). Update `version`
  in `Cargo.toml` and, with it, the `version` of the three workspace crates in
  `[workspace.dependencies]`; refresh `Cargo.lock` with `cargo check`; add a section at the
  top of `CHANGELOG.md`; set `version` and `date-released` in `CITATION.cff`; commit
  `chore(release): X.Y.Z` and add the annotated tag `vX.Y.Z`.
- Pushing tags, creating GitHub releases and publishing packages need the maintainer's
  approval.
