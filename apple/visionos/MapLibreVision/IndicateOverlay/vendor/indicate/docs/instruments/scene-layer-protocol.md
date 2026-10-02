# Instrument scene-layer protocol

This document specifies the bounded layer vocabulary carried by the
instrument scene IR. It is a deterministic engineering contract for simulator
and embedded backends; it does not claim certification credit for a backend.

## Frame and command encoding

A scene starts with `SCENE_FORMAT_VERSION` and then contains zero or more
commands:

```text
[format_version u8]
[opcode u8][payload_len u16 little-endian][payload bytes]...
```

The layer vocabulary in format version 1 is:

| Opcode | Name | Payload |
|---|---|---|
| `0x50` | `BeginLayer` | exactly one `LayerId` byte |
| `0x51` | `EndLayer` | exactly one `LayerId` byte |

A zero-length or extended layer-marker payload is malformed. An unknown layer
identifier is malformed even though an unknown ordinary drawing opcode may be
counted and skipped. Changing the marker shape or adding a layer identifier
requires a scene format version change.

## Layers and z-order

The numeric identifier is the z-order. Commands paint in encoding order.

| ID | Layer | Content |
|---:|---|---|
| 0 | `Background` | Replaceable sky/ground or raster/depth imagery only |
| 1 | `Attitude` | Horizon line, pitch ladder, roll scale, aircraft reference, primary orientation |
| 2 | `Tapes` | Air-data tapes, readouts, and HSI data boxes |
| 3 | `Guidance` | CDI, course/deviation scales, and navigation guidance |
| 4 | `Annunciation` | Flags, miscompares, and source or sensor failure cues |
| 5 | `Failure` | Display-level failure and reversion content |

Each present layer appears exactly once, layers are strictly ascending, and
layers do not nest. A drawing or state command outside a layer fails the
frame. The end marker must name the open layer.

## Graphics-state isolation

Every layer has this exact structural envelope:

```text
BeginLayer(id)
Save
    zero or more drawing/state commands
Restore
EndLayer(id)
```

The outer Save and Restore use the ordinary `0x01` and `0x02` opcodes. They
isolate transform, clip, fill, stroke, and other backend graphics state. No
command may precede the Save or follow its matching Restore within the layer.
Nested Save/Restore pairs are allowed inside the envelope up to the stack
budget and must balance before the outer Restore.

This shape preserves state isolation on a decoder that understands the
drawing vocabulary but skips `0x50` and `0x51` as unknown opcodes.

The byte-exact empty `Background` layer corpus is:

```text
01
50 01 00 00
01 00 00
02 00 00
51 01 00 00
```

## Resource bounds

The conforming engineering profile is fixed by constants exported from
`indicate-instrument-scene`:

| Resource | Bound |
|---|---:|
| Defined layers | 6 |
| Commands per layer, including the isolation Save/Restore | 4096 |
| Graphics-state depth, including the isolation Save | 32 |
| Encoded bytes per scene | 65,536 |

Exceeding any bound fails the whole frame. Validation is allocation-free and
must finish before a backend makes any part of the frame visible.

## Unknown commands and corruption

An unknown ordinary opcode is accepted only when it is well framed and lies
inside the active state-isolation envelope. It counts against the layer
command budget and increments the unknown-command report.

The frame fails on:

- a bad format version, truncated command, or malformed known payload;
- an unknown layer identifier;
- a command outside a layer or outside its isolation envelope;
- a nested, duplicate, descending, mismatched, or unclosed layer;
- an unbalanced or over-depth graphics-state stack;
- a per-layer command or scene-byte budget overrun.

## Required panel profiles

Structural validation deliberately accepts any ascending subset because panel
types do not all use the same bands. The host that selects the panel must
enforce its required layer mask before visible commit:

| Panel | Required layers | Optional layers |
|---|---|---|
| PFD | `Attitude`, `Tapes`, `Guidance`, `Annunciation` | `Background`, `Failure` |
| HSI | `Attitude`, `Tapes`, `Guidance`, `Annunciation` | `Background`, `Failure` |
| Autoflight | `Tapes`, `Annunciation` | `Background`, `Attitude`, `Guidance`, `Failure` |
| Monitor | `Tapes`, `Annunciation` | `Background`, `Attitude`, `Guidance`, `Failure` |
| Configuration | `Tapes`, `Annunciation` | `Background`, `Attitude`, `Guidance`, `Failure` |

This table is the `required_layers` mask of each shipped descriptor, not an
independent statement of intent: the required column is the mask, the optional
column is its complement over the defined layers, and both are listed in
ascending layer order. `layer_profile_doc_tests` in
`indicate-instrument-panels` parses this table and fails the build if either
column disagrees with the descriptor it describes, so a mask change must
arrive with its row. The section holds this one table and no other.

Among the bands above `Background`, a panel requires one when it opens that
band on *every* frame, not when the band always carries content. The PFD
requires `Guidance` on those terms: it opens the band unconditionally so the
layer contract holds under every degradation, and the flight-director command
bars inside it disappear when the director is disengaged, invalid, or
decluttered by the unusual-attitude tier. A backend must therefore not read an
empty `Guidance` band as a missing one.

`Background` is deliberately outside that rule and appears in no panel's
required mask, which is why it sits in the optional column of every row above
— including the HSI's and the monitor's, both of which do open it on every
frame. The mask is what a host enforces before visible commit, and requiring
the one band a compositor may replace or drop would forbid the composition
that band exists for. What a panel owes in that band is stated by its
`BackgroundCapability` instead, and proven by the admission harness: an
`Opaque` or `Cedeable` panel must lay a full-frame opaque cover there in every
case the harness draws, which it draws at the empty config. That is a stronger
obligation than the mask could express, so for an `Opaque` panel the band is
mandatory in practice and optional in the mask, and both are correct.

A `Cedeable` panel is the case the mask suits exactly. It owes the cover at its
default and may drop the band on request — the PFD does, under a synthetic-
vision or no-background configuration — so the band is genuinely absent from
some of its frames. Requiring it would forbid the only configuration the
capability exists to allow.

A well-framed prefix that ends at a layer boundary is a smaller structural
scene, not proof of a complete panel frame. Missing a required layer is a
display failure and must not advance the visible render generation.

`Failure` does not substitute for a required panel layer. A non-success render
status bypasses scene consumption and is covered by the backend-owned failure
page, so the failed producer is not required to generate its own failure
content.

Frame generation, snapshot identity, transport integrity, and panel selection
remain outside the scene bytes. A consumer combines those transport checks
with the required-layer mask and `validate_layers` before commit.

## Background and SVS boundary

`Background` is the only replaceable band. An SVS raster or depth compositor
occupies that band and remains below `Attitude`. `BackgroundMode::None` omits
the scene background while retaining the horizon line, pitch ladder, roll
scale, aircraft reference, tapes, guidance, and annunciations as applicable.

No background producer owns a critical layer. Duplicate-layer rejection
prevents two producers from claiming the same band in one scene.
