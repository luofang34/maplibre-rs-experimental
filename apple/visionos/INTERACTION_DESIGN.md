# Spatial map interaction

## Navigation contract

One short pinch selects the visible feature at the system-provided selection ray. A held pinch that moves pans the immersive map or turns the globe under the grabbed point. A globe grab translates its initial surface aim by the hand’s room-space displacement, independent of how close the hand is to the head. Leaving the silhouette holds the last valid surface point until the hand returns; it cannot switch into faster off-globe navigation. Head motion changes the view through the scene; it does not generate navigation input.

Two pinches start an undecided interaction. Relative separation zooms, relative twist rotates, and coherent common hand motion carries the table globe. Carry accepts modest differences between the two hands and starts after 12 mm of shared movement. In immersive free camera, common horizontal and vertical motion orbit and pitch around the ground target instead. Each interaction locks its intent until a hand releases. Translation, scale and rotation therefore cannot accidentally accumulate together. The hand count changing consumes that event and rebases the surviving hand.

The rotation threshold is three degrees. Common motion starts at 15 mm in immersive mode and 25 mm for globe placement. A two-hand zoom continues across the immersion boundary without requiring release. Thresholds apply to net motion from the beginning, so stationary tracking noise cannot accumulate. A separate threshold and a modest dominance margin distinguish spread from twist. A short pinch cannot select after any two-hand interaction, cancellation or drag.

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

## Verification

Regression coverage must include: head motion with stationary hands; one/two-hand transitions; spread with an asymmetric hand; a small twist; common motion orbiting without zoom; an invariant free-camera target and fixed-camera eye; repeated pole and antimeridian crossings; and selection exclusion after drag or cancellation. Measurement adds antipodal/coincident/dateline tests, retained endpoints across tile eviction, ownership cancellation, and left/right-eye agreement.

References: [ForeFlight ruler](https://support.foreflight.com/hc/en-us/articles/202724569-How-can-the-distance-between-points-be-measured-on-the-map-in-ForeFlight-Mobile), [Apple spatial input privacy](https://developer.apple.com/documentation/visionos/adopting-best-practices-for-privacy), [MapLibre rendered feature queries](https://maplibre.org/maplibre-gl-js/docs/API/classes/Map/#queryrenderedfeatures).
