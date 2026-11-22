# CLAUDE.md — Project Rules & Evidence Gates

See [`AGENTS.md`](AGENTS.md) for the short checklist and [`docs/src/development-documents/agent-rules.md`](docs/src/development-documents/agent-rules.md) for full details enforced by [`evidence.toml`](evidence.toml) (`cargo-evidence` `Tier 2: verify`) and `evidence-mcp` (`.claude/settings.json`).

## Three-Step Progressive Workflow

1. **Query context first**: Run `cargo evidence context <path> --json` (or MCP `evidence_context`) before reading large subtrees to load only the owning `[feature_map]` slice, active `[floors]`, `[[web_boundaries]]`, and `verify_targets`.
2. **Read deep rules on demand**: Open [`agent-rules.md`](docs/src/development-documents/agent-rules.md) or the relevant `[structure]`/`[architecture]`/`[verify]` block in [`evidence.toml`](evidence.toml) only when working in or diagnosed by that gate.
3. **Format, Gate & Verify**:
   - Fast crate loop: `cargo test -p maplibre`, `cargo test -p maplibre-coords`, `cargo test -p render-tests`
   - Tier 1 static gate: `RUSTUP_TOOLCHAIN=nightly cargo fmt -- --check && cargo evidence gate --format=jsonl`
   - Tier 2 full verification (`wasm32-clippy`, `web-lib`, `native-nextest`, `proptest >= 16`, `render-tests` artifacts): `cargo evidence check --tier=verify --format=jsonl`

## Non-Negotiable Constraints

- **Architecture**: Strict `foundation -> core -> platforms -> apps_and_tests` DAG; `wasm32` closures forbid `tokio`, `mio`, `hyper`, `rusqlite`, `openssl*`, `rustls`, `socket2`, `curl*`, `reqwest` (in `maplibre`/`web-tile-worker`), and `winit` (in `web-tile-worker`).
- **Structure & Comments**: Max `500` lines/file (`lib.rs <= 100` with `//!` doc), `<= 80` lines/fn; no `mod.rs`, `utils.rs`/`helpers.rs`/`common.rs`, or stray PNG/backup files. No `TODO`/`FIXME`/`HACK`, rot markers, apology comments, inline comment blocks `> 6` lines, or ratio `> 15%`.
- **Rust Lints & Ratchets**: Workspace denies `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`, `await_holding_lock`, `let_underscore_must_use`, `let_underscore_future`. Zero bare `#[allow]` outside tests; use parameter structs instead of `too_many_arguments`; keep `new_library_panics = 0`, `new_unwraps = 0`, and `render-tests/ignores.json <= 70`.
- **Web/TS**: Workers cannot import DOM/canvas/offscreen-client modules; DOM helpers cannot import worker internals; no `setTimeout` or ad-hoc `requestAnimationFrame` in `web/**`.
- **Build Hygiene**: Keep `.githooks` active (`git config core.hooksPath .githooks`) and restrict Cargo builds to default `target/` and `target/verify-wasm` sequentially.
