# Immersive rendering validation — 8 September 2026

## Diagnosed failure

The retrieved `MapLibreVision-2026-09-08-145256.ips` reports SIGABRT and a LIBSYSTEM application-triggered fault at `cp_frame_end_submission`. Symbolication identifies the host's renderer-failure return. The application log immediately before termination reports a globe-camera construction error and an undrawn eye. The host ended submission without encoding presentation. This captured quit was not a Jetsam OOM termination. Its last reported footprint was 2094 MB with 988 MB available; the retrieved Jetsam report names `managedappdistributiond` as its victim.

Every drawable with an available command buffer now reaches presentation. A failed stereo render reuses the last complete colour/depth pair; initial failure clears valid sky/passthrough and depth. A bounded pair of stereo targets prevents one failed eye from publishing alongside a successful eye. At 1888 × 1792, the two complete colour/depth sets total approximately 103 MiB. Immersion changes are applied after frame submission.

Zoom preserves the captured target and scene orientation and solves distance using positive radial clearance. Regression tests cover repeated table-to-ground zooms and both eye offsets. The precise original globe-camera error was not recoverable from its log; projection errors now include their underlying cause.

## Rendering and interaction checks

- Globe drag follows the hand's room-space displacement at table and intermediate sizes. Returning from outside the silhouette cannot trigger a faster navigation mode.
- Globe vector textures use a uniform level, bounded together with all ancestors. Promotion waits for the complete level; terrain mesh detail remains independent. Close immersive views retain distance-based LOD.
- The sky-side calculation follows each eye ray, avoiding a gradient reversal when the projected map center crosses behind the eye. The horizon regression sweeps pitch across 90 degrees at four rolls.
- The simulator exercised the injected renderer failure and continued presenting, with over 150 subsequent frame reports and no further render failure. Inspected captures retain the map.
- An intermediate view at height 8,000,000 settled at zero pending requests with stable pool revision 49 and 11 drapes. Its simulator footprint was about 121 MB.
- A scripted flight from height 1,000,000 to immersive height 4000 completed without a renderer error, settling at zero pending requests, stable pool revision 1279 and 32 drapes. Its simulator footprint was about 427 MB. These numbers do not measure physical-device memory or binocular comfort.

## Executed checks

| Check | Result |
| --- | --- |
| Swift interaction suite | 32 passed |
| Core Rust, `headless,thread-safe-futures`, Metal enabled | 452 passed; 1 existing ignored |
| Bundled style regressions | 4 passed |
| Workspace format check | Passed |
| Changed Rust module/layout guard | Passed |
| Rust release, visionOS device and simulator | Passed |
| Xcode signed device and simulator builds | Passed |
| Physical Vision Pro installation | Confirmed, xrOS 27.0 (24M5361a) detected automatically |

Remote device launch failed with an unavailable CoreDevice XPC service. A subsequent process listing did not show MapLibreVision, so a physical-device rendering run is unconfirmed. Installation succeeded independently.

All requested full workspace CI commands were executed. Strict Clippy stops at the existing `maplibre-build-tools` `manual_ok_err` lint. Native all-target tests and release builds include Android/web crates that reject macOS. Strict documentation reports existing missing docs and broken links, including `WorldCoors`, `WorldTileCoors`, and `Transferables`. The full workspace is not green; targeted renderer tests and both visionOS release targets pass.

## Reproducing frame recovery

Build a Debug simulator app and launch with `--enter --mode immersive --height 4000 --simulate-render-failure-once`. The flag injects one failure after a complete stereo frame exists. Capture `Library/Caches/maplibre-tiles/maplibre.log` from the app container, then run:

```sh
python3 apple/visionos/MapLibreVision/Tests/check-frame-recovery.py /path/to/maplibre.log
```

The checker requires the injected failure, selection of stereo recovery, at least five subsequent frame reports, and no subsequent renderer error. Inspect a simulator capture as well to confirm the retained view. The flag is compiled only in Debug builds.

The presentation order follows [Apple's Compositor Services rendering guide](https://developer.apple.com/documentation/CompositorServices/drawing-fully-immersive-content-using-metal).

Local artifacts use `/private/tmp/map-horizon-*`: crash report, device log, Rust/Swift test logs, CI gate logs/results, signed build/install logs, and recovery/intermediate/flight screenshots. Interaction and terrain contracts are documented in [INTERACTION_DESIGN.md](INTERACTION_DESIGN.md) and [TERRAIN_RENDERING.md](TERRAIN_RENDERING.md).
