# Head-worn display presentation policy

A head-worn display (HWD) presents information near the observer's eyes.
A head-up display (HUD) presents information through a fixed combiner.
The flight mode annunciator (FMA) identifies active modes, armed modes, and automation engagement.

This policy defines display responsibilities, content reservations, reference frames, and declutter precedence.
It applies to the independent HWD instrument set.
Fixed instruments share flight semantics through the [flight guidance contract](flight-guidance-contract.md).
They retain their own layout.

## Evidence and interpretation

The [FAA 2026 report index](https://www.faa.gov/data_research/research/med_humanfacs/oamtechreports/2020s/2026) lists two relevant experimental reports.
The index was checked on September 16, 2026.

[DOT/FAA/AM-26/02](https://www.faa.gov/data_research/research/med_humanfacs/oamtechreports/media/202602.pdf) studied 24 crews in a Boeing 737 simulator.
It compared a HUD, binocular HWD, and monocular HWD during approach, landing, and rollout.
Flightpath and energy management were broadly similar.
Workload increased with the monocular HWD.
Optical characteristics affected landing and runway-incursion detection.
The noncollimated HoloLens 2 and disabled simulator motion limit generalization.
These results support testing external-event detection alongside flight tracking.
They do not establish an optimal FMA position.

[DOT/FAA/AM-26/03](https://www.faa.gov/data_research/research/med_humanfacs/oamtechreports/media/202603.pdf) studied 11 crews using a HUD and monocular HWD.
It compared enhanced flight vision system (EFVS) imagery during routine approaches in day and night conditions.
An EFVS provides sensor imagery of the external environment.
Flightpath differences were generally small, while HWD workload increased.
Pilot feedback supported clearing EFVS imagery during the transition to natural landing references.
The study did not include unexpected failures or environmental hazards.
Its noncollimated HWD and disabled simulator motion limit generalization.
Clearing sensor imagery and suppressing duplicate instruments therefore need separate controls.

[DOT/FAA/AM-25/07](https://www.faa.gov/data_research/research/med_humanfacs/oamtechreports/media/202507.pdf), pages 35–36, reviews visual clutter.
Dense overlays can mask information and delay unexpected-target detection.
The review supports task-dependent declutter and fewer unnecessary central indicators.
Its publication date does not make every reviewed experiment recent.

[AC 25-11B](https://www.faa.gov/documentlibrary/media/advisory_circular/ac_25-11b.pdf) addresses display consistency, clutter, and HUD presentation.
[AC 25.1329-1C Change 2](https://www.faa.gov/documentLibrary/media/Advisory_Circular/AC_25.1329-1C_CHG2.pdf) addresses flight guidance indications and transitions.
These references support retaining clear automation state when reducing other content.

The coordinates below are engineering choices for evaluation.
No cited experiment validates these exact coordinates or a universal corner preference.
Device optics, field of view, angular text size, and tracking latency require separate evaluation.

## State and responsibility

`PanelData` carries aircraft state and qualified guidance reports.
`AlertOutput` carries the shared alert manager's ordered snapshot.
`DisplayContext` carries the host's display choices and current visibility evidence.
Display choices cannot command the autopilot, flight director, thrust controller, sensors, or weapons.

`draw_hwd` accepts these inputs and an explicit compact-layout selection.
`presentation_plan` gives the host the same resolved declutter decisions.
Descriptor entry points select the primary role and normal flight detail.
They preserve compatibility with panel admission and flat raster review.

The host supplies resolved visibility through `alternate_flight_display`.
A valid true value means another functioning, readable flight display is currently visible.
The host must expire this evidence when tracking, occlusion, display health, or visibility becomes uncertain.
Head angle alone cannot establish visibility.
Missing, stale, degraded, and failed evidence cannot authorize blanking.

## Roles, tasks, and precedence

Role and task are independent of aircraft guidance modes.

| Selection | Meaning | Retained information |
|---|---|---|
| Primary | HWD supplies the flight reference | Flight measurements, attitude, targets, envelope, modes, alerts |
| Supplemental | Another display can supply duplicate flight information | Modes, alerts, primary-data flags, display status |
| Flight | Navigation and flight monitoring | Receiver deviations and normal flight information |
| Mission | Reserve task status space | Flight essentials, guidance, targets, envelope, modes, alerts |
| Normal | Include secondary groundspeed | All required flight information |
| Reduced | Remove secondary groundspeed | Scales, barometric setting, targets, envelope, modes, alerts |
| Recovery | Automatic unusual-attitude override | Aircraft attitude, flight measurements, modes, alerts |

Declutter follows this precedence:

1. Apply the aircraft's unusual-attitude presentation policy.
2. Retain urgent alerts and alert-manager failure indications.
3. Restore flight information when required flight data loses validity.
4. Apply confirmed duplicate-display blanking only in the supplemental role.
5. Apply the requested task and detail level.

A caution or warning restores flight geometry and suspends the mission reservation.
Required flight-reference failures also suspend that reservation.
Recovery suppresses receiver detail and invalid steering cues.
A prominent `UNUSUAL ATTITUDE` indication appears even when the observer faces the aircraft nose.
A separate label identifies nose-high, nose-low, high-bank, or inverted attitude.
Extreme-pitch chevrons retain their separate airframe thresholds.
It overrides both duplicate-display blanking and the mission task.
The existing off-axis entry and exit cones prevent repeated layout switching.
They do not select an aircraft guidance mode.

The display labels effective presentation state, including `RECOVERY`, `HUD BLANK`, and `COCKPIT BLANK`.
It uses `FLIGHT PRIORITY` when an urgent condition suspends a requested mission task.

## Content reservations

Coordinates use the 1200 by 600 logical frame.
They do not represent physical display pixels or validated angular dimensions.
A zone is a content reservation, not an opaque background panel.

| Region | Reservation | Rule |
|---|---|---|
| Automation | x 60–450; y 0–122 | Stable active, armed, engagement, protection, and transition indications |
| Orientation | Upper center | Datum-qualified heading and track; separate from FMA |
| Alerts | x 840–1170; y 0–122 | Ordered alert stack, overflow, and manager health |
| Central external view | x 450–750; y 160–335 | No routine status text or opaque task panel |
| Flight measurements | Left and right side groups | Speed coordinate, limits, altitude datum, and vertical speed |
| Aircraft guidance | Lower center | Bounded aircraft-reference command bars or compact attitude instrument |
| Mission status | x 900–1180; y 480–590 | Available only when `PresentationPlan::mission` is true |
| System status | Lower edge | Data failures and effective HWD presentation state |
| Host context | x 450–830; y 584–600 | One fitted playback or connection status line |

World-registered cues keep their true angular positions.
A target marker must not move to an empty screen location to avoid a label collision.
A future label compositor can offset labels while retaining their association with the marker.
Status zones remain stable instead of chasing moving targets.

`AUTOMATION_ZONE`, `ALERT_ZONE`, `CENTRAL_VIEW_ZONE`, `MISSION_ZONE`, and `HOST_CONTEXT_ZONE` expose the principal reservations.
The mission task removes receiver detail that could occupy the mission reservation.
Barometric settings remain outside that reservation in both layouts.

Nonconformal text shares one projection, so head movement cannot collapse its reserved spacing.
World cues retain their physical registration and can cross instrument regions.
The browser supports head sweeps for this review.
A device compositor still needs clipping, field-of-view checks, and readability validation.

## Reference frames

`layer_reference` supplies the projection contract for each instrument layer.

| Content | Reference |
|---|---|
| Forward scales and bounded flight director | Display-fixed |
| Compact aircraft-attitude instrument and measurements | Display-fixed |
| FMA, numeric heading, alerts, and system status | Display-fixed |
| Angular geometry from `directions` | Earth-referenced directions |

A head turn changes the projection, never measured aircraft attitude or flight-director errors.
The host projects nonconformal instrument layers through one display-fixed frame.
It projects angular geometry separately.
The browser adapter preserves layer references in its exported production geometry.

## Automation attention

Active and armed modes retain the shared vocabulary and color meanings.
Autopilot, flight director, and automatic thrust retain independent engagement indications.
Explicit `ON`, `OFF`, `ARM`, and unknown indications avoid hidden engagement assumptions.
The HWD reduces FMA width without reducing its text size.

The transition indication occupies a small, dedicated event slot above the modes.
The indication uses producer elapsed time plus acquisition age.
It expires after five seconds; redraw and head movement cannot restart it.
Five seconds is the current engineering setting, not a claimed FAA requirement.
Reversion uses amber attention and the shared alert manager's independent annunciation.

The guidance contract does not identify which individual mode field changed.
The HWD therefore identifies the event without falsely highlighting a specific changed channel.
A future field-change contract must carry explicit, source-qualified transition identity.

## Mission and sensor extension

The [DCS F/A-18C guide](https://www.digitalcombatsimulator.com/upload/iblock/8d7/2s3e89jqknz7xmti8hrhe1bjt2uw1s3e/DCS%20FA-18C%20Early%20Access%20Guide%20EN.pdf), page 257, describes reject levels and automatic HUD blanking.
This example supports explicit task profiles and duplicate suppression in simulation.
It does not establish a universal civil-flight layout.

The mission reservation is implemented; radar and weapon adapters are not connected.
DCS-specific task data must remain separate from `GuidanceSample` and its flight targets.
A radar track cannot become a flight-director command merely because both appear on the HWD.

A task report needs source identity, sequence, acquisition time, validity, and coordinate frame.
It also needs track identity, selection state, and the distinction between measured and predicted position.
Weapon availability and selection need explicit simulator state.
Missing data must not imply readiness, a valid track, or an authorized action.

Task composition follows these rules:

1. Retain flight warnings, failure indications, and required control references.
2. Admit only task content appropriate to the selected task.
3. Suppress duplicate cues already supplied by the visible HUD or cockpit display.
4. Preserve true registration for world and sensor cues.
5. Keep detailed radar, maps, and checklists in explicitly opened panels.
6. Provide a separate, immediate control to clear optional sensor imagery.
7. Retain flight symbology when the operator clears sensor imagery.

The HWD set emits no sensor imagery.
The imagery-clear requirement is an integration rule, not an implemented EFVS feature.
The current `Normal` and `Reduced` selections do not claim aircraft-specific DCS reject levels.

## Verification

Generate the gallery with `cargo run --locked -p hwd-bench -- target/hwd-flight-review.html`.
Run `node tools/hwd-bench/browser-check.mjs target/hwd-flight-review.html` for browser interaction checks.
Set `CHROME_BINARY` when Chrome uses a different installation path.


`presentation::tests` checks visibility qualification, recovery precedence, urgent alerts, mission reservations, FMA bounds, and transition expiry.
`cross_set_tests` checks shared mode meanings, target identity, invalid commands, envelope gaps, and alert retention.
The gallery generator rejects intersecting text bounds and text outside the logical frame.
Bounds include glyph advances, text anchors, and two logical pixels for halos and separation.
Additional tests combine invalid sources, extreme readouts, all display contexts, and both layouts.
The host context reservation remains clear in these checks.
These guards cover nonconformal text; they do not prove perceptual readability or prevent world-cue crossings.

The browser gallery renders 55 source cases under six production display contexts.
Its mission profile shows the reservation without inventing radar or weapon data.
A review-only zone overlay exposes the principal reservations.

Flight samples remain frozen in the gallery.
Only transition attention follows the review clock.
Changing head pose, profile, or brightness does not restart that clock.
The explicit replay control starts another review of the recorded event.

Human evaluation must compare external-event detection, mode recall, tracking error, workload, and task completion.
Scenarios must include unexpected hazards, mode reversion, disconnects, failed visibility evidence, head turns, and visual landing transitions.
Tests must cover bright terrain, darkness, display loss, and monocular and binocular hardware where applicable.
Successful software tests cannot establish a human-performance advantage for this layout.
