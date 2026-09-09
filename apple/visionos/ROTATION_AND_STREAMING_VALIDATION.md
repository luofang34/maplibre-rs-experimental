# Rotation crash and immersive terrain streaming

The device report `MapLibreVision-2026-09-09-042548.000.ips` records a SIGTRAP on
the main thread. Symbolication identifies `ContentView.swift:29`, the integer
conversion of the displayed tilt. The renderer log reports singular external
views immediately before the trap. The concurrent Jetsam report names another
process; it does not identify this app exit as an OOM kill.

A deterministic Swift regression reproduces invalid rotation after 37 repeated
tilt/orbit updates. Quaternion lengths grow far beyond one, corrupting the scene
matrix and producing a nonfinite tilt. Normalizing the accumulated quaternion
and the from/to axes keeps 10,000 updates rigid. Invalid tilt inputs are ignored,
nonfinite feedback is withheld, and the readout avoids a trapping integer cast.

## Renderer behavior

- Every stereo eye is validated before any frame state or content changes. A bad
  eye returns a typed error and allows the host to retain its complete frame.
- A rejected generic external input retains the last valid external camera.
- Terrain textures refine the largest remaining detail deficit first. Distance
  breaks ties, so one foreground patch cannot consume all refinement slots.
- Eight texture slots remain available for replacement groups. At low pressure
  this permits 24 displayed targets rather than 16 within the same 32-texture cap.
- Motion prediction samples the first eye every 100 ms and looks 350 ms ahead,
  capped at 20 degrees and twice the camera height of travel. It accounts for
  anchor rebasing, rejects discontinuities, and resets across tracking gaps.
  Prediction affects requests only. Physical head tracking remains immediate.
- Explicit flight destinations retain precedence over inferred movement.
- Terrain mode requests a bounded margin of vector and DEM tiles after visible
  requests. The candidate allowance uses a 32 MiB retained-data estimate with an
  8 MiB reservation per unknown tile, limiting it to four unknown tiles. Actual
  decoded tile size may exceed its estimate; this is admission control, not a
  hard bound on worker allocations. Existing global backpressure still applies.
- Prefetch covering updates at most ten times per second. Critical memory clears
  it immediately. Speculative coverage errors do not fail a displayed frame.

## Verification

- Swift: 49 tests passed, including 10,000 tilt/orbit updates and invalid inputs.
- Renderer/style suite: 491 Rust tests plus 5 style tests passed, one ignored.
  An additional terrain-prefetch integration test passed in a focused rerun
  together with its admission-budget test: 497 distinct renderer/style tests.
- Stereo regression verifies an invalid second eye leaves the previous view and
  frame generation unchanged, then accepts the next valid stereo frame.
- Terrain regressions check complete, nonoverlapping coverage and balanced detail
  demand; prefetch tests check offscreen preparation, capacity, memory-pressure
  clearing, bounded prediction, sampling cadence and anchor rebasing.
- Rust release libraries built for device and simulator. Both Xcode app builds
  succeeded. The signed device app installed successfully, database sequence 1756.
- Remote launch timed out. Physical gesture feel, sustained flight and the visual
  result on the headset remain to be confirmed in use.

Full workspace CI was executed: formatting passed; Clippy stops in existing
`maplibre-build-tools` manual-`ok` code; workspace tests and release include
Android/Web targets incompatible with the macOS host, plus existing API and
benchmark failures. Documentation gates report existing missing docs and broken
links. The workspace-wide gates are not claimed green.

Artifacts are `/private/tmp/map-coverage-{crash.ips,tilt-repro.log,swift.log,
tests.log,prefetch-integration.log,ci-results.json,rust-device.log,rust-sim.log,
xcode-device.log,xcode-sim.log,install.log}` on the build machine.

The strategy requests detail useful at the current projected resolution and
keeps nearby data ready within resource limits. Cold network loads, abrupt
teleports and arbitrary future motion cannot be guaranteed invisible.
