# Display-reason registry

This document is the cross-shell contract for display reasons. A display
reason is a machine-readable statement of why a display is not showing
current, valid panel imagery. The Rust shell, the Swift shell, and the
JavaScript shell each report display reasons for the renderer-health
monitor input ([`AIR-IN-013`](requirements.md#air-in-013)). One registry
defines the reasons for all three shells. A cross-shell comparison of
display health then compares states, not vocabularies.

This document is a deterministic engineering contract for simulator and
embedded shells. It does not claim certification credit for a shell.

## The registry

The authoritative enumeration is the `DisplayFault` enum in
`indicate-alerts` (`crates/indicate-alerts/src/condition.rs`). Each
reason has a stable one-byte code. The alert manager packs the code into
a stable two-byte `AlertId` as `0x0500 | code`, where `0x05` is the
display family selector. A shell may use the code or the `AlertId` as
the cross-shell identity of a reason.

| Code | Reason | Identity | Class | Meaning |
|---:|---|---|---|---|
| 1 | `RendererStalled` | `0x0501` | Warning | The renderer stopped making progress. |
| 2 | `FrameGenerationLost` | `0x0502` | Warning | Frame generation stopped advancing. |
| 3 | `CommandBufferCorrupt` | `0x0503` | Warning | The draw-command buffer failed its integrity check. |
| 4 | `BackendLost` | `0x0504` | Caution | The rendering backend was lost. |
| 5 | `RetainedImage` | `0x0505` | Warning | A retained last-good image is suspected on the output path. |

The table lists every reason in ascending code order. `DisplayFault::ALL`
carries the same list in the same order. The registry doc tests in
`crates/indicate-alerts/src/condition/tests.rs` parse this table and fail
the build when the table and the code disagree. A change to the registry
must include its row. The section holds this one table and no other.

Annunciation labels such as `DSP STALL` are shell presentation. They are
not part of the registry.

## Append-only rule

The registry is append-only, like the scene opcode vocabulary
(ADR-0017). A new reason takes the next free code. Codes are never
renumbered and never reused. A shell that does not know a reason can
then still show that the display is not current.

A code is a position in the list, counted from one, and `code()`
computes it from that position. Two reasons therefore cannot be given
the same code by hand.

A reason is not deleted. To retire one, keep its row and its code, and
write `retired` in its row. The code is not used again. This is the same rule
the requirement registry uses: a retired identifier stays in the
registry with its disposition.

## Unknown reasons on an older shell

A shell that receives a reason code it does not know must not show a
healthy display. It must show that the display is not current, under a
generic reason.

The Rust side decodes fail-closed. `DisplayFault::from_code` gives
`None` for a code outside the registry, and `class_of` gives `None` for
the identity that code packs into. `None` names no reason. It does not
name a healthy state, and the shell supplies the generic presentation.

`None` is a decoding answer, not an alert. The Rust vocabulary carries
no display condition for an unknown code, so a shell that needs one
raises it from its own code. A variant that carries a raw display code,
as the frame-mismatch condition does, would remove that step.

## Swift and JavaScript mirrors

The Swift and JavaScript mirrors live in their downstream repositories.
Each mirror must declare the same reasons with the same codes as the
table above. A mirror must not declare a reason that this registry does
not define.

No mechanism in this repository makes a stale mirror fail. An append
moves none of the five contract values, so a pinned consumer reports no
error across it. Each downstream repository must therefore carry its
own drift check against this table, and must run it. Until a mirror
carries one, agreement between the shells is a convention, not a
guarantee.
