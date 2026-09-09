# New York zoom and globe carry — 2026-09-09

## Device evidence

`MapLibreVision-2026-09-08-232303.ips` reports SIGKILL (`EXC_CRASH`), termination namespace `0x28`, code 0. The main thread was idle. The `maplibre render` thread was inside `projection::tile_covering::coarsening_parent`, called by `coarsen`, globe coverage, and the vector request system.

This establishes the killed thread's stack, not the exact system policy that killed it. It does not establish an OOM kill. The nearby Jetsam report predates this session and identifies another process as its victim. The app log reported approximately 1.8–2.0 GiB footprint near the New York zoom, with frequent late frames.

## Tile selection

External globe and Mercator eyes refine a bounded frontier, prioritizing detail demand and then distance. A parent is replaced only when all visible children fit. Both the frontier size and the number of attempted splits are bounded, including conservative parents whose children all cull away. Nearest-first ordering, source zoom limits, LOD history and padding remain inputs to selection.

Terrain drape coarsening builds one ancestor index and uses the same refinement mechanism. It does not rebuild a histogram of all remaining tiles for every individual merge.

An isolated optimized Rust harness calling the actual coarsener produced:

| Input tiles → budget 512 | Repeated histogram scans | Ancestor index and bounded refinement |
| --- | ---: | ---: |
| 1,024 | 31.79 ms | 0.484 ms |
| 4,096 | 599.48 ms | 1.249 ms |
| 16,384 | 9,457.88 ms | 4.294 ms |

These are algorithm timings with a coordinate stand-in, not GPU or complete frame timings. The unrestricted implementation's multi-second scaling is a plausible stall mechanism consistent with the device stack; the report alone cannot prove that it triggered this particular SIGKILL.

Regression tests exercise a full visible world requesting zoom 31, single-child chains, unserved source levels, empty budgets and propagated errors. New York camera cases span 40,000 km down to 150 m, above and below the horizon. Budget-reduction tests verify that every reference leaf remains covered exactly once.

## Globe carry

Common two-hand motion carries the whole globe. Sideways and vertical travel scale with the globe's apparent distance, using the same 0.6 m minimum hand-reference depth as indirect surface dragging. Gain is limited to 1–4 and latched at gesture start. A globe 1.2 m away moves sideways twice as far as a 0.6 m hand-reference displacement. Push/pull stays at room scale. Tests verify constant gain during a carry, unchanged orientation/zoom, and no motion from moving the head with stationary hands.

One-hand surface turning retains its pointer-following rule. Physical gesture feel still needs headset use.

## Validation

- Core renderer: **475 passed**, one existing ignored test; includes Metal pixel, stereo, terrain, symbol and coverage tests.
- Default style integration: **5 passed**.
- Swift interaction: **44 passed**.
- Changed module layout: passed; no touched `mod.rs` files. Touched Rust files/functions satisfy the 500/80-line limits.
- Rust release builds: device and simulator targets passed.
- Xcode simulator and signed device builds: passed.
- Installed bundle `com.sokolysystems.maplibre.vision` on the connected Vision Pro. Installation returned database sequence 1724.

The debug-only renderer launch option `--zoom-stress-nyc` places the globe over New York, waits eight seconds, and performs two continuous zoom round trips between table height and 4 km. It then settles at table height. The sweep uses normal placement, zoom, clearance, rendering and loading paths. Its timing logic has a unit test.

```sh
xcrun simctl launch --terminate-running-process booted \
  com.sokolysystems.maplibre.vision \
  --enter --mode tableGlobe --zoom-stress-nyc --memory-cap 1000
```

The simulator completed the sweep and remained running, with no renderer errors. Logged map zoom reached 15.56 and DEM detail reached zoom 13. Pending tiles peaked at four, drapes at 32, and retained geometry at 444 MiB. The captured 67 one-second frame summaries averaged 16.07 ms; the highest interval average was 60.63 ms and the longest individual frame was 180.22 ms. Loading hitches remain. Simulator memory counters do not establish physical headset memory behavior.

Full workspace CI was executed in order: format, Clippy, all-target tests, missing-docs rustdoc, broken-link rustdoc, release build. Formatting passed. The other gates remain blocked by existing workspace issues: `maplibre-build-tools` Clippy/documentation errors and Android/web targets compiled on macOS, with existing API and documentation failures. The scoped renderer tests and both visionOS builds passed independently. No unrelated workspace failures were suppressed.
