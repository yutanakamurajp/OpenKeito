# Contributing to OpenKeito

Bug reports, models that do not behave as the documentation says, corrections of the
documents and code are all welcome. This page says what a change needs before it can be
merged. For a larger change, open an issue first and describe what you want to do.

## Where the code comes from

OpenKeito is an independent implementation, and it has to stay one. Read
[docs/clean-room.md](docs/clean-room.md) before you contribute. In short:

- Use published papers and textbooks, public standards, the documentation of open-source
  projects whose licenses permit it, closed-form solutions and hand calculations. Name the
  source in the code or in [docs/numerical-method.md](docs/numerical-method.md).
- Do not use the code, component definitions, file formats, documentation text, input
  files or output files of a proprietary simulator, and do not describe how one behaves
  from having inspected it.
- Do not commit names of client companies or persons, or material received from them.
- Reference data in the repository is either analytic or produced by OpenKeito itself.

By opening a pull request you confirm that your contribution follows these rules, and that
you have the right to submit it under the licenses of the project (MIT OR Apache-2.0).

## Reporting a problem

Give the version (`openkeito --version`), the operating system, the smallest model that
shows the problem, the command or call, and the `summary.json` (or the document that the
command printed). Say what you expected. Do not attach models or data that you may not
publish. Security problems are reported privately: see [SECURITY.md](SECURITY.md).

## Building and checking

You need a Rust toolchain (1.85 or later; `rustup` installs it), Python 3.10 or later for
the Python package, and Node 18 or later for the WebAssembly module and the browser page.

Every change:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

When Python code or the bindings (`crates/openkeito-py`) change, build the package into a
virtual environment kept in `.venv` and run the Python checks:

```sh
python -m venv .venv                     # then activate it
python -m pip install ".[test,lint,pandapower,powsybl,mcp,paper]"
python -m pyflakes python paper
python -m pytest
```

(`maturin develop --release` rebuilds the native module in place after a change.)

When `openkeito-wasm`, the library of `openkeito-cli`, the solver or an example changes,
build the module and compare its results with those of the command; when `web/` or an
example changes, also test and assemble the page (both `web/tests/modules.test.mjs` and
`web/build.mjs` load the built WebAssembly module in `crates/openkeito-wasm/pkg/`, so run the
`openkeito-wasm` build and `wasm-bindgen` commands below first):

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked     # the version of wasm-bindgen in Cargo.lock
cargo build --release -p openkeito-wasm --target wasm32-unknown-unknown
cargo build --release -p openkeito-cli
wasm-bindgen --target web --out-dir crates/openkeito-wasm/pkg target/wasm32-unknown-unknown/release/openkeito_wasm.wasm
node crates/openkeito-wasm/tests/compare.mjs
node --test web/tests/modules.test.mjs
node web/build.mjs
```

When a dependency changes, check the licenses (`cargo install cargo-deny --locked`):

```sh
cargo deny check licenses sources bans
```

Every dependency must carry a license that MIT OR Apache-2.0 allows; a new license
identifier fails the check until it is added to `deny.toml`, with the reason.

These checks, run on Windows by the maintainers, decide a merge. The workflow
`.github/workflows/ci.yml` runs the same on Windows, macOS and Linux, but only when it is
started by hand; say so in the pull request when a change is likely to behave differently
on another system. Times are not compared there; when a change can affect the speed,
measure as [docs/performance.md](docs/performance.md) says.

## What a change needs

- **A test against something independent.** A new component, block or method is compared
  with a closed-form solution, a hand calculation or the same equations integrated
  separately, and the comment of the test says where its tolerance comes from. A fix comes
  with a test that failed before it.
- **A new component** needs: the model structure with units in the field names and doc
  comments, validation, a catalog entry with an example (`openkeito components`), the
  element in `openkeito-core`, and the test above.
- **A new diagnostic** needs a new code and a hint that says what to change, and its range
  in the table of [docs/model.md](docs/model.md#diagnostics).
- **Documents that stay true.** A change of behaviour changes the page that describes it in
  the same pull request. Numbers in the documents come from tests or from recorded
  measurements.
- **Results that do not change by accident.** If a change moves the results of existing
  models, say by how much and why.
- Do not commit build artifacts (`target/`, `.venv`, `_native.pyd`, `crates/openkeito-wasm/pkg`,
  `web/dist`, `paper/out`); `.gitignore` lists them. Files are stored with LF line endings.

## Commit messages

Commits follow [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/):
`<type>(<scope>): <description>`.

- Types: `feat`, `fix`, `perf`, `refactor`, `docs`, `style`, `test`, `build`, `ci`, `chore`,
  `revert`. Scopes: `model`, `core`, `cli`, `py`, `mcp`, `wasm`, `web`.
- The type, the scope and the footer tokens are English; the description and the body are
  written in Japanese, as in the history of the repository.
- A breaking change has `!` after the type or scope and a `BREAKING CHANGE:` footer.

The version follows SemVer 2.0.0 and is chosen from these commits at a release; do not
change `version` or `CHANGELOG.md` in a pull request. Releases are made by the maintainers.

## License

OpenKeito is licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT](LICENSE-MIT) at the user's option, and contributions are accepted under the same
terms.
