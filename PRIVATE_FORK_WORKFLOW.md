# Private fork integration and review branches

`private/main` is the integrated, installable tree. Renderer, platform hosts, styles,
regression tests, and their validation records belong there together.

An active review branch represents one independently reviewable issue. A device
installation, screenshot round, or debugging session does not get a branch. Keep
follow-up fixes on the issue's branch and include its regression guardrail.

The current review topics are:

| Branch | Scope |
| --- | --- |
| `fix/foreground-terrain-refinement` | Foreground priority, bounded replacement textures, fallback retirement, stable paint |
| `fix/terrain-road-perspective` | Road alignment, perspective width, bridge elevation, tunnel occlusion |
| `fix/visionos-absolute-tilt` | Room-relative map tilt with independent head tracking |
| `fix/visionos-map-panel` | Resizable controls with persistent Level and Leave actions |

These branches form a dependency stack above the integrated globe/terrain work.
Their parent branch is the review base; comparing every branch directly with
upstream `main` would include unrelated prerequisites. Renderer topics can be
prepared for MapLibre review as their dependencies become available upstream.
The Swift host changes remain separate platform topics. A topic branch is not a
claim of complete MapLibre style-spec conformance or upstream acceptance.

The earlier `feat/*`, `terrain/*`, and `fix/*` branches record prerequisites. Keep
only branches still useful for reviewing those prerequisites; consolidate them by
issue when preparing their upstream PR, and retire superseded review branches.
`fix/immersive-rendering` is a frozen integration checkpoint, not a new PR topic.
New work starts from `main` or its explicit prerequisite branch.

Before rewriting a published topic, preserve the old refs and verify that the
integrated tree still contains every intended change. Never rewrite or publish to
the public upstream as part of private-fork maintenance. Current-session approval
is required for force pushes or remote branch deletion unless already authorized.

Run fmt, clippy, tests, missing-docs and broken-link documentation checks, and the
release build. Record platform-specific baseline failures separately from the
feature's passing checks. Install from the integrated tree, not a partial topic.
