# Spatial map interaction

## Navigation contract

One short pinch selects the visible feature at the system-provided selection ray. A held pinch that moves pans the immersive map or turns the globe under the grabbed point. Head motion changes the view through the scene; it does not generate navigation input.

Two pinches start an undecided interaction. Relative separation zooms, relative twist rotates, and common hand motion carries the table globe. In immersive free camera, common horizontal and vertical motion orbit and pitch around the ground target instead. Each interaction locks its intent until a hand releases. Translation, scale and rotation therefore cannot accidentally accumulate together. The hand count changing consumes that event and rebases the surviving hand.

The rotation threshold is three degrees. Common motion starts at 15 mm in immersive mode and 25 mm for globe placement. Thresholds apply to net motion from the beginning, so stationary tracking noise cannot accumulate. A separate threshold and a modest dominance margin distinguish spread from twist. A short pinch cannot select after any two-hand interaction, cancellation or drag.

## Free camera and vehicle viewpoint

Free camera captures the ground point under the center of the view when orbit starts. The point stays at the same room position while the scene rotates around it. Zoom changes the camera-to-target distance. If the center looks into sky, use the existing surface focus rather than inventing an intersection behind the viewer. A new orbit can acquire a new target; head motion during an orbit cannot move it.

A fixed viewpoint, such as an airplane camera mount, keeps its scene-space eye position. Artificial look offsets rotate about that position, while aircraft pose and physical head pose remain separate transforms. The host explicitly chooses this camera policy; camera behavior must never switch merely because the view crosses the horizon. Vehicle following and aircraft pose input are future integrations. The current application uses free camera.

## Measurement, reserved interaction

ForeFlight uses a two-finger hold to open a ruler. For this spatial map, reserve a stationary two-pinch hold of 0.6 seconds for measurement: both initial selection rays must hit terrain, no navigation intent may have latched, and neither hand may have moved beyond its dead zone. Show a progress cue before capture. This is a future interaction; holding two hands currently has no navigation effect.

Once captured, each hand owns one geographic endpoint. Dragging it updates that endpoint, not the camera. Releasing retains the ruler, with explicit endpoint handles and a close control. An endpoint can snap to a selected airport using its stable source and feature ID; show the airport identifier so snapping is visible. A pinch on empty space starts ordinary navigation when no handle owns it. Opening a second ruler requires clearing the first. The same operation must be available as “Measure from here” on a feature card for one-handed and accessibility use.

Store endpoints in geographic coordinates independently of tile lifetime. Compute spherical great-circle distance with a stable atan2(cross-length, dot) central angle and a documented mean Earth radius; display nautical miles (1852 metres per NM) and initial/final true bearings. Split the drawn arc at the antimeridian and tessellate to a screen-error tolerance on the globe. Coincident endpoints have zero distance and no bearing. Antipodal endpoints have no unique shortest arc: retain the dragged arc plane and do not report a unique bearing. If ellipsoidal surveying precision is added, expose WGS84 geodesic distance as a distinct calculation policy. Magnetic bearings need a dated magnetic model; do not relabel true bearings as magnetic.

The line follows the geographic arc above sampled terrain with a small rendering offset, while its measured distance remains horizontal. It is drawn and depth-tested as one shared stereo object. Terrain loading must not change its distance. Navigation, measurement ownership and selection are separate input states, sharing hit testing and geographic conversion.

## Gaze and feature selection

Use visionOS focus and confirmed pinch events. Continuous raw eye direction is not needed. A rendered-symbol query returns placed, visible text/icon candidates with layer, source layer, feature ID and properties. Airport cards should resolve these to a stable airport record; labels from adjacent tiles must not create duplicate selections. A future focus affordance can use system-managed interaction regions without continually reporting gaze to the app. Both eyes use the same feature placement and selected ID.

## Verification

Regression coverage must include: head motion with stationary hands; one/two-hand transitions; spread with an asymmetric hand; a small twist; common motion orbiting without zoom; an invariant free-camera target and fixed-camera eye; repeated pole and antimeridian crossings; and selection exclusion after drag or cancellation. Measurement adds antipodal/coincident/dateline tests, retained endpoints across tile eviction, ownership cancellation, and left/right-eye agreement.

References: [ForeFlight ruler](https://support.foreflight.com/hc/en-us/articles/202724569-How-can-the-distance-between-points-be-measured-on-the-map-in-ForeFlight-Mobile), [Apple spatial input privacy](https://developer.apple.com/documentation/visionos/adopting-best-practices-for-privacy), [MapLibre rendered feature queries](https://maplibre.org/maplibre-gl-js/docs/API/classes/Map/#queryrenderedfeatures).
