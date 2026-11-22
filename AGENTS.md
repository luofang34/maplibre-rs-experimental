# AGENTS.md — Progressive Rules & Verification Entrypoint

This repository uses `cargo-evidence` (`Tier 2: verify` in [`evidence.toml`](evidence.toml)) and `evidence-mcp` to keep Rust, WASM, and Web/TS checks fast and progressive.

## 1. Progressive Workflow (Before Editing -> On Demand -> Finish)

1. **Discover scope first**: Before broad file searches, query `cargo evidence context <path> --json` (or the `evidence_context` MCP tool) for your target file to get its `[feature_map]` slice, owning crates, ceilings, and `verify_targets`.
2. **Load deep rules on demand**: Consult [`docs/src/development-documents/agent-rules.md`](docs/src/development-documents/agent-rules.md) and the specific section of [`evidence.toml`](evidence.toml) surfaced by `context` or any diagnostic — do not preload every rule doc upfront.
3. **Iterate & verify scoped, then full Tier 2**:
   - Scoped iteration: `cargo test -p maplibre`, `cargo test -p maplibre-coords`, `cargo test -p maplibre-tcs`, or `(cd web/lib && npx tsc --noEmit && node --test test/*.test.mjs)`.
   - Before finishing: `RUSTUP_TOOLCHAIN=nightly cargo fmt`, `cargo evidence gate --format=jsonl`, and `cargo evidence check --tier=verify --format=jsonl`.

## 2. Hard Invariants (Never Bypass)

- **Layering & WASM**: Respect `foundation -> core -> platforms -> apps_and_tests` layers; keep native OS/runtime/SQLite/TLS crates out of `wasm32` graphs (`[[architecture.dep_closures]]`).
- **Structure Limits**: Max `500` lines per `.rs` file (`lib.rs <= 100` with `//!` docs) and `80` lines per function. Never introduce `mod.rs`, `utils.rs`, `helpers.rs`, `common.rs`, or stray `frame_*.png`/backup files; files in `file_size_allowlist` may only shrink.
- **No Panics or Bare `#[allow]`**: Zero `.unwrap()`, `.expect()`, `panic!()`, `todo!()`, `unimplemented!()` in clean crates; never use bare `#[allow(...)]` outside tests (`#[expect(..., reason = "...")]` when justified, typed structs instead of `too_many_arguments`). Lower `[floors]` ceilings when removing `.expect(` sites.
- **Comments & Conformance**: No temporal/archaeology markers, bare issue numbers, excuse comments (`TODO`/`FIXME`/`HACK`/`WORKAROUND`), apology hedges, inline blocks `> 6` lines, or inline comment ratio `> 0.15`. Never raise `render-tests/ignores.json` above `70`.
- **Web Boundaries & Timers**: Keep `web/lib/src/{singlethreaded,multithreaded}/**` isolated from main-thread DOM modules; never add `setTimeout` or ad-hoc `requestAnimationFrame` workarounds in `web/**`.
- **Disk Discipline**: Use ONLY default `target/` (native) and `target/verify-wasm` (`wasm32`); never run concurrent Cargo builds sharing a target directory.
- **Hooks**: Ensure `git config core.hooksPath .githooks` is active; `.claude/settings.json` also runs `cargo evidence gate --format=jsonl` on file edits.
