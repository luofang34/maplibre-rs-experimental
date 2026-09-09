# Immersive rendering validation — 8 September 2026

## Latest termination

`MapLibreVision-2026-09-08-185216.ips` records a SIGKILL with termination namespace `<0x28>`, flags 6 and code 0. Apple’s [XNU reason definitions](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/sys/reason.h) identify namespace 40 as Wakeboard. The report does not expose its precise trigger. The contemporaneous `JetsamEvent-2026-09-08-185215.ips` names `intelligenceplatformd` as its victim and does not list MapLibreVision. This evidence does not establish an app OOM termination.

The application log shows repeated tile processing and many late frames before the quit. Its maximum reported footprint was 2527 MB; the final report was 1805 MB with 1267 MB available under the host budget. The main thread was idle in its application run loop. No Rust panic or failed stereo frame appeared in this log.

## Changes

- Clearance samples each physical eye’s displaced geographic position, using the finest cached DEM even outside the rendered region. Tilt, navigation and head movement keep the eyes at least 150 terrain metres above loaded ground; table-scale clearance also reserves 5 cm in the room. Flight corrections persist without accumulating an identical correction on successive frames.
- Foreground DEM requests precede background texture coverage, with one coarse ancestor for initial elevation bounds. Mesh detail remains independent of the texture budget. The default 4000 m immersive viewpoint requests fine terrain both looking toward the horizon and looking down.
- Terrain vector requests and uploads follow the bounded texture covering instead of also loading unused fine tiles and all intermediate ancestors. Geometry staging has 16 MiB vector and 8 MiB symbol allowances per frame; a single larger layer may proceed to avoid starvation. Two named tile workers limit simultaneous CPU work. The host uses a 2.5 GiB internal footprint envelope, leaving earlier headroom for compositor and queued work; this is an application policy, not a claimed OS termination limit.
- Symbol coverage follows glyph and GPU readiness independently of terrain textures. Ready fine labels remain while neighbouring tiles need parent labels. Collision priority resolves overlapping parent/child anchors. New labels start hidden until placement supplies their elevation. Labels sample the rendered terrain rather than the coarser DEM implied by their source tile.
- Collision metadata is tracked by each allocation’s identity. An unrelated upload no longer invalidates all placed labels. Residency diagnostics report both vector and symbol revisions and the finest loaded DEM level.

## Executed checks

| Check | Result |
| --- | --- |
| Swift interaction and camera suite | 35 passed |
| Core Rust, `headless,thread-safe-futures`, Metal enabled | 461 passed; 1 existing ignored |
| Bundled style regressions | 4 passed |
| Pixel stability | Identical terrain/text pixels across 24 stationary frames |
| Terrain label regression | Parent text and sprite stay visible above a newly loaded finer DEM |
| Default immersive DEM regression | Fine DEM requested without head movement at both tested pitches |
| Workspace format and changed-module guard | Passed; no changed `mod.rs` |
| Rust release, visionOS device and simulator | Passed |
| Xcode signed device and simulator builds | Passed |
| Physical Vision Pro installation | Confirmed |

The final simulator app, launched directly into immersive mode at 4000 m with `--memory-cap 1000`, loaded DEM level 13. A stationary observation passed the allocation checker: 22 residency reports spanning 31.6 seconds, zero pending requests, unchanged vector/symbol revisions, and no renderer error. Continued observation held vector revision 369 and symbol revision 89. Representative settled frame reports were 4.4–4.8 ms. Simulator memory counters are not physical-device measurements.

A table-to-immersive flight was also exercised during this round. It completed and settled with zero pending requests and stable vector allocations. The final app’s remote device launch failed with CoreDevice error 4000: a required XPC connection to `remoteService` was unavailable. Installation succeeded independently; physical-device rendering and the termination’s precise trigger remain unverified.

All six requested full workspace CI commands were run. Format passed. Strict Clippy stops at the existing `maplibre-build-tools` `manual_ok_err` lint. Native all-target tests, documentation and release builds include Android/web crates that reject macOS; the benchmark target also has an existing `ProcessedLayers::clone` error. Strict documentation additionally encounters existing undocumented items and broken links. Full workspace CI is not green; the targeted renderer tests and both visionOS release targets pass.

## Reproducing stability

Launch a Debug simulator app with `--enter --mode immersive --height 4000 --memory-cap 1000`. Keep its viewpoint fixed while loading finishes, then collect `Library/Caches/maplibre-tiles/maplibre.log` from the app container and run:

```sh
python3 scripts/check-immersive-stability.py /path/to/maplibre.log
```

The checker requires a stationary camera, at least 30 seconds of observations, zero pending requests, fine terrain, unchanged vector and symbol allocation revisions, and no renderer failures. Inspect a capture as well; allocation stability alone does not establish visual correctness.

For the independent presentation-recovery guard, launch with `--simulate-render-failure-once` and run:

```sh
python3 apple/visionos/MapLibreVision/Tests/check-frame-recovery.py /path/to/maplibre.log
```

That Debug-only injection exercises reuse of the last complete stereo frame. The presentation ordering follows [Apple’s Compositor Services rendering guide](https://developer.apple.com/documentation/CompositorServices/drawing-fully-immersive-content-using-metal).

Local artifacts for this round use `/private/tmp/map-clearance-*`: crash and Jetsam reports, device and simulator logs, Rust/Swift tests, CI results, signed builds, installation/launch logs and screenshots. Interaction and terrain contracts are in [INTERACTION_DESIGN.md](INTERACTION_DESIGN.md) and [TERRAIN_RENDERING.md](TERRAIN_RENDERING.md).
