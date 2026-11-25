**What this changes, and why**

**How it was checked**

- [ ] `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
- [ ] Python or bindings changed: `python -m pyflakes python paper`, `python -m pytest`
- [ ] wasm, solver, examples or `web/` changed: `node crates/openkeito-wasm/tests/compare.mjs`, `node --test web/tests/modules.test.mjs`, `node web/build.mjs`
- [ ] Dependencies changed: `cargo deny check licenses sources bans`
- [ ] New behaviour has a test against an independent solution, with the source of its tolerance
- [ ] The documents that describe what changed are updated

**Results of existing models**

Unchanged, or changed by how much and why:

**Origin**

- [ ] This contribution follows docs/clean-room.md: published sources only, nothing taken
      from a proprietary simulator, no names of clients or persons, and I may submit it
      under MIT OR Apache-2.0.
