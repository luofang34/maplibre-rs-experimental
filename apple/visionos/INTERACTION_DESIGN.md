# Map and synthetic-vision interaction

## Scope and shared actions

The visionOS application implements the navigation, placement, zoom, orbit, clearance and selection recognizers described below. The iPad, desktop/web, aircraft-follow and ruler bindings are design contracts for future hosts, not shipped platform features. These are our interaction decisions, informed by the linked platform guidance rather than a claim that every platform prescribes the same gestures.

The host translates device input into `Select`, `PanSurface`, `ZoomAt`, `OrbitAt`, `LookFrom`, `PlaceWorld`, `MeasureEndpoint` and `Recenter`. Keep input recognition separate from geographic projection and camera policy. Distances remain metres internally; display nautical miles for aviation. Screen coordinates mean logical pixels on 2D displays and angular pointer motion in a headset. Never share a hard-coded metres-per-drag multiplier across map scales.

## Camera policies

| Policy | Target and orientation | Navigation ownership |
| --- | --- | --- |
| Planning map | Geographic focus; north-up or explicit track-up; zero pitch | Drag keeps the geographic point under the pointer; zoom about the captured point. Panning suspends aircraft follow and exposes Recenter. |
| Free world / RTS exploration | Captured ground target; terrain clearance; independent orbit heading and tilt | Pan moves the target, orbit circles it, zoom changes target distance. Head motion only changes the physical view. |
| Aircraft SVS / cockpit | Aircraft pose × camera mount × deliberate look offset × tracked head | Looking changes the look offset; zoom changes field of view within limits. Never pan the aircraft or replace measured pitch/roll with gesture tilt. Recenter clears the look offset. |
| Table world | Retained room placement plus geographic focus | Turn the geography with one pinch; carry the whole object with common two-hand motion; spread zooms and twist rotates. |

Keep north-up, track-up and head-relative look distinct. A compass control states which orientation is active. A north-up reset changes map heading; SVS Recenter restores the forward aircraft view. Neither action changes an aircraft's pose. SVS requires a valid attitude source for an attitude-based display; a GPS-only preview must state that attitude is unavailable. Missing data must not become a fabricated level horizon.

## Platform bindings

| Action | iPad map / free 3D | Vision Pro | Web / desktop RTS style |
| --- | --- | --- | --- |
| Select | Tap feature | Look and short system pinch | Primary click |
| Pan / turn map | One-finger drag | Held single pinch, shared pointer rule | Primary drag; arrows/WASD pan when the canvas owns focus |
| Zoom at target | Two-finger spread about centroid | Two pinches spread; confirmed primary target, then ray midpoint | Wheel/trackpad zoom at pointer; plus/minus at focus |
| Orbit / tilt free 3D | Two-finger twist for yaw; common vertical drag for pitch | Twist for yaw; common two-hand motion for orbit/pitch in immersive | Secondary drag for orbit; Shift + secondary drag for pitch; compass/reset alternative |
| Place table world | Not applicable | Common two-hand translation including depth | Scene object translation handles when applicable |
| Aircraft look | One-finger SVS drag; pinch changes FOV | Physical head motion; deliberate two-hand look offset | Secondary drag or held look key; wheel changes FOV |
| Measure | Stationary two-finger hold; accessible Measure action | Reserved stationary two-pinch hold; feature-card Measure action | Measure action then two points; draggable endpoint handles |

RTS keyboard panning uses a target-depth-based screen speed with bounded acceleration and deceleration, not degrees per second. Edge scrolling is optional and off by default on the web, avoiding motion while reaching browser controls. Pointer capture ends on release, cancellation, window blur or tracking loss. No keyboard movement while a text field has focus. No pan inertia by default in the headset or SVS; optional short, cancellable inertia belongs to 2D map exploration.

UI controls, selected feature handles and ruler endpoints own input before the map does. Then resolve gesture intent once; head movement never participates in that decision. A visible capture cue should identify the locked pivot or endpoint. Supply visible zoom, compass, Recenter, tilt and Measure actions for keyboard, one-hand and assistive access; custom gestures cannot be the only path.

## Navigation contract

One short pinch selects the visible feature at the system-provided selection ray. A held pinch that moves pans the immersive map or turns the globe under the grabbed point. All indirect drags move a virtual pointer in the initial selection plane. Its reference depth is the initial head-to-hand distance with a 0.6 m minimum; moving a resting hand near the body cannot multiply angular gain. The same ray delta drives globe and immersive navigation. A sphere grab follows ray/surface intersections. Ground panning follows a captured plane and preserves camera altitude; grazing or sky input uses a finite plane at four camera heights. Empty-space globe navigation uses the focus’s visual depth, so its speed follows apparent map scale. Leaving the silhouette holds the last valid surface point until the hand returns; it cannot switch into faster off-globe navigation. Head motion changes the view through the scene; it does not generate navigation input.

Two pinches start an undecided interaction. Relative separation zooms, relative twist rotates, and coherent common hand motion carries the table globe. Carry accepts modest differences between the two hands and starts after 12 mm of shared movement. In immersive free camera, common horizontal and vertical motion orbit and pitch around the ground target instead. Each interaction locks its intent until a hand releases. Translation, scale and rotation therefore cannot accidentally accumulate together. The hand count changing consumes that event and rebases the surviving hand.

The rotation threshold is three degrees. Common motion starts at 15 mm in immersive mode. Coherent globe carrying starts at 12 mm per hand; less coherent common motion requires 25 mm and must dominate spread and twist. A two-hand zoom continues across the immersion boundary without requiring release. Thresholds apply to net motion from the beginning, so stationary tracking noise cannot accumulate. A separate threshold and a modest dominance margin distinguish spread from twist. A short pinch cannot select after any two-hand interaction, cancellation or drag.

## Free camera and vehicle viewpoint

Free camera captures the surface point under the midpoint of the two initial selection rays when orbit starts, with the center head ray as fallback. The same rule applies at table, intermediate and immersive scales. The point stays at the same room position while the scene rotates around it. Zoom changes the camera-to-target distance. If the center looks into sky, use the existing surface focus rather than inventing an intersection behind the viewer. A new orbit can acquire a new target; head motion during an orbit cannot move it.

A fixed viewpoint, such as an airplane camera mount, keeps its scene-space eye position. Artificial look offsets rotate about that position, while aircraft pose and physical head pose remain separate transforms. The host explicitly chooses this camera policy; camera behavior must never switch merely because the view crosses the horizon. Vehicle following and aircraft pose input are future integrations. The current application uses free camera.

## Zoom anchor

At the start of a zoom, capture the surface under the primary system selection ray. If it misses, try the midpoint of the two selection rays, then the head ray and the existing surface focus. Preserve that scene point under the captured ray as height and scene scale change. Zoom preserves the scene orientation and solves the target distance from positive eye-to-surface clearance; orientation changes belong to orbit and mode flights. Physical head motion cannot reacquire a different zoom point midway through the gesture. This uses confirmed system selection input rather than continuous eye tracking.

## Placement and mode flights

Common two-hand motion carries the globe by the same room distance at table and intermediate scales. Carrying preserves its orientation. Twist captures the local surface normal and a pitch axis once per gesture, so later head motion cannot change the rotation axes. A twist at the center of the table globe spins it about its visible normal without swinging the globe around the room.

A mode flight starts from the complete rendered pose, including user placement, orbit corrections and head-relative orientation. The table destination uses the retained room placement, north up and zero tilt. The immersive destination uses horizontal head heading and the remembered immersive tilt; a nearly vertical head direction retains the map bearing. Position, rotation and scale interpolate together, with exact start and end poses. Physical head motion continues to affect the view independently. The tilt control reflects gesture and flight results.

## Measurement, reserved interaction

ForeFlight uses a two-finger hold to open a ruler. For this spatial map, reserve a stationary two-pinch hold of 0.6 seconds for measurement: both initial selection rays must hit terrain, no navigation intent may have latched, and neither hand may have moved beyond its dead zone. Show a progress cue before capture. This is a future interaction; holding two hands currently has no navigation effect.

Once captured, each hand owns one geographic endpoint. Dragging it updates that endpoint, not the camera. Releasing retains the ruler, with explicit endpoint handles and a close control. An endpoint can snap to a selected airport using its stable source and feature ID; show the airport identifier so snapping is visible. A pinch on empty space starts ordinary navigation when no handle owns it. Opening a second ruler requires clearing the first. The same operation must be available as “Measure from here” on a feature card for one-handed and accessibility use.

Store endpoints in geographic coordinates independently of tile lifetime. Compute spherical great-circle distance with a stable atan2(cross-length, dot) central angle and a documented mean Earth radius; display nautical miles (1852 metres per NM) and initial/final true bearings. Split the drawn arc at the antimeridian and tessellate to a screen-error tolerance on the globe. Coincident endpoints have zero distance and no bearing. Antipodal endpoints have no unique shortest arc: retain the dragged arc plane and do not report a unique bearing. If ellipsoidal surveying precision is added, expose WGS84 geodesic distance as a distinct calculation policy. Magnetic bearings need a dated magnetic model; do not relabel true bearings as magnetic.

The line follows the geographic arc above sampled terrain with a small rendering offset, while its measured distance remains horizontal. It is drawn and depth-tested as one shared stereo object. Terrain loading must not change its distance. Navigation, measurement ownership and selection are separate input states, sharing hit testing and geographic conversion.

## Gaze and feature selection

Use visionOS focus and confirmed pinch events. Continuous raw eye direction is not needed. A rendered-symbol query returns placed, visible text/icon candidates with layer, source layer, feature ID and properties. Airport cards should resolve these to a stable airport record; labels from adjacent tiles must not create duplicate selections. A future focus affordance can use system-managed interaction regions without continually reporting gaze to the app. Both eyes use the same feature placement and selected ID.

## Labels and selectable overlays

Use a stable hierarchy: selected/active navigation objects, major airports and cities, regional places, towns, villages, then local road/POI detail. Aviation overlays need their own source, stable IDs and importance rank; geographic place labels must not compete equally with active route instructions. Keep safety-critical aircraft symbology in its dedicated instrument overlay. Do not infer airport importance from a basemap place rank.

Style labels through MapLibre symbol layers: continuous zoom-dependent sizes, restrained halos, spacing in ems, ordered layers and feature ranks, collision placement, and distinct weight for the major geographic tier. Keep point labels upright and facing the viewport over terrain. Use a shared ground-relative height for text and icon; use an absolute altitude only when the data supplies one. Selection and collision must use the same projected geometry. Reserve variable anchors, rich text, full feature-driven paint and cross-tile identity/fade parity for the renderer work listed in LABEL_RENDERING.md; accepting a style JSON key is not evidence that the property renders correctly.

## Verification

Regression coverage must include: head motion with stationary hands; one/two-hand transitions; spread with an asymmetric hand; a small twist; common motion orbiting without zoom; an invariant free-camera target and fixed-camera eye; repeated pole and antimeridian crossings; and selection exclusion after drag or cancellation. Measurement adds antipodal/coincident/dateline tests, retained endpoints across tile eviction, ownership cancellation, and left/right-eye agreement.

References: [ForeFlight ruler](https://support.foreflight.com/hc/en-us/articles/202724569-How-can-the-distance-between-points-be-measured-on-the-map-in-ForeFlight-Mobile), [Apple spatial input privacy](https://developer.apple.com/documentation/visionos/adopting-best-practices-for-privacy), [MapLibre rendered feature queries](https://maplibre.org/maplibre-gl-js/docs/API/classes/Map/#queryrenderedfeatures).

Additional references: [Apple gestures](https://developer.apple.com/design/human-interface-guidelines/gestures), [Apple spatial game input](https://developer.apple.com/videos/play/wwdc2024/10094/), [ForeFlight SVS and attitude input](https://support.foreflight.com/hc/en-us/articles/218199147-How-can-the-Attitude-Indicator-or-Synthetic-Vision-view-be-accessed-in-ForeFlight-Mobile), [MapLibre symbol style properties](https://maplibre.org/maplibre-style-spec/layers/#symbol).
