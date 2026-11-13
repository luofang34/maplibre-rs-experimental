// C ABI of the maplibre-rs renderer for the visionOS host app. Mirrors src/lib.rs.
#ifndef MAPLIBRE_VISIONOS_H
#define MAPLIBRE_VISIONOS_H

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct MaplibreVisionOSMap MaplibreVisionOSMap;

// Where the map's scene stands in the host's world.
typedef struct {
    double anchor_latitude;
    double anchor_longitude;
    double anchor_altitude_meters;
    // Column-major 4x4 from the scene (metres east, north, up from the anchor) to the world:
    // a rotation, a translation and one uniform scale.
    const float *world_from_scene;
} MaplibreVisionOSPlacement;

// One eye of a frame.
typedef struct {
    // Column-major 4x4 from eye space (x right, y up, looking along -z) to the world.
    const float *world_from_eye;
    // Tangents of the left, right, top and bottom half angles.
    const float *tangents;
    float near;
    // Not finite or not beyond near: replaced by a far plane that covers the globe.
    float far;
    // id<MTLTexture> of a bgra8Unorm or bgra8Unorm_srgb texture with the pixelFormatView
    // usage the eye is drawn into, or NULL to draw into the map's own texture.
    const void *color_texture;
    // id<MTLTexture> of a Depth32Float texture the eye's depth is written to, or NULL.
    const void *depth_texture;
} MaplibreVisionOSEye;

const char *maplibre_visionos_version(void);

MaplibreVisionOSMap *maplibre_visionos_create(const char *style_json, uint32_t width,
                                              uint32_t height, const char *cache_dir);

/** Fill the full immersive environment while preserving passthrough around a table globe. */
void maplibre_visionos_set_opaque_environment(MaplibreVisionOSMap *map, bool opaque);
/** Set physical-to-logical display pixel ratio (e.g. 2.0 on Retina displays). */
void maplibre_visionos_set_pixel_ratio(MaplibreVisionOSMap *map, double ratio);
/** Explicitly configure the planetary body's mean radius and elevation bounds in metres. */
void maplibre_visionos_set_body(MaplibreVisionOSMap *map, double radius_meters,
                                double highest_ground_meters, double lowest_ground_meters);
/** Update body-fixed Sun direction/radius, Moon disc, twilight band, and atmosphere toggle. */
void maplibre_visionos_set_sky(MaplibreVisionOSMap *map, bool has_sun,
                               const double *sun_ecef_xyz, double sun_radius_deg,
                               bool has_moon, const double *moon_ecef_xyz,
                               double moon_radius_deg, float night_brightness, bool atmosphere);
/** Replace the starfield behind the body (`star_count` tuples of 7 floats: xyz, mag, rgb). */
void maplibre_visionos_set_stars(MaplibreVisionOSMap *map, const float *packed_stars_f32,
                                 uint32_t star_count);
/** Query placed labels/icons at a pixel (y down). Returns required bytes including NUL.
 * Only writes when buffer is non-null and capacity is sufficient. */
size_t maplibre_visionos_query_symbols(const MaplibreVisionOSMap *map, double x, double y,
                                     char *buffer, size_t capacity);
/** Frame timeline: the map's own frame statistics plus the host's spans, under one frame number.
 * Spans with gpu=true are GPU execution time from timestamps; false is CPU time, including
 * command encoding. Export returns required bytes including NUL and only writes (and empties
 * the timeline) when the buffer is large enough. */
void maplibre_visionos_trace_enable(MaplibreVisionOSMap *map, uint32_t capacity);
uint64_t maplibre_visionos_last_frame(MaplibreVisionOSMap *map);
void maplibre_visionos_trace_span(MaplibreVisionOSMap *map, uint64_t frame, const char *name,
                                  bool gpu, uint64_t nanoseconds);
void maplibre_visionos_trace_presentation(MaplibreVisionOSMap *map, uint64_t frame,
                                          uint64_t deadline_nanoseconds,
                                          uint64_t completed_nanoseconds);
void maplibre_visionos_trace_device(MaplibreVisionOSMap *map, uint64_t frame,
                                    uint64_t resident_bytes, int32_t thermal_state);
size_t maplibre_visionos_trace_export(MaplibreVisionOSMap *map, char *buffer, size_t capacity);
uint32_t maplibre_visionos_width(const MaplibreVisionOSMap *map);
uint32_t maplibre_visionos_height(const MaplibreVisionOSMap *map);

// Draws every eye of a frame. Returns the id<MTLTexture> the map draws into when an eye has
// no colour texture, owned by the map and valid until the next call; NULL on failure. Depth
// textures are written before this returns. request_overscan widens each eye's frustum for
// tile requests; 1 for none.
// prefetch, when not NULL, is where the scene will stand when a flight in progress ends:
// the tiles that frame needs are requested now, and the levels passed on the way are not.
const void *maplibre_visionos_render_frame(MaplibreVisionOSMap *map,
                                           const MaplibreVisionOSPlacement *placement,
                                           const MaplibreVisionOSPlacement *prefetch,
                                           const MaplibreVisionOSEye *eyes, uint32_t eye_count,
                                           float request_overscan, double timestamp_seconds);

// Draws one frame from the map's orbit / free-globe camera at (latitude, longitude), zoom,
// pitch_deg, bearing_deg, and roll_deg. Returns the map's offscreen id<MTLTexture> or NULL.
const void *maplibre_visionos_render_orbit(MaplibreVisionOSMap *map, double latitude,
                                           double longitude, double zoom, double pitch_deg,
                                           double bearing_deg, double roll_deg,
                                           double timestamp_seconds);

// Effective style zoom of the last rendered frame (eye-height zoom for external 6-DoF frames,
// camera zoom for orbit frames).
double maplibre_visionos_effective_zoom(const MaplibreVisionOSMap *map);

// Raycasts logical viewport point (screen_x, screen_y) onto the globe / loaded DEM surface.
bool maplibre_visionos_screen_to_ground(const MaplibreVisionOSMap *map, double screen_x,
                                        double screen_y, double *out_latitude,
                                        double *out_longitude, double *out_elevation_meters);

// Moves the orbit camera so the 3D ground anchor lands directly under (screen_x, screen_y).
bool maplibre_visionos_move_anchor_to_screen(
    MaplibreVisionOSMap *map, double anchor_lat, double anchor_lon, double anchor_elev_m,
    double screen_x, double screen_y, double zoom, double pitch_deg, double bearing_deg,
    bool north_locked, double *out_center_lat, double *out_center_lon, double *out_bearing_deg);

// Incremental globe drag from (from_x, from_y) to (to_x, to_y), including empty-space off-limb
// drags, cross-limb hyperbolic trackball continuity, and North-Up / FreeGlobe modes.
bool maplibre_visionos_drag_orbit(MaplibreVisionOSMap *map, double from_x, double from_y,
                                  double to_x, double to_y, double zoom, double pitch_deg,
                                  double bearing_deg, bool north_locked, double *out_center_lat,
                                  double *out_center_lon, double *out_bearing_deg);

// The Metal command queue (id<MTLCommandQueue>) the map draws on, owned by the map and valid
// while it lives. Work committed on it runs after the map's draws, so copies out of the map's
// texture need no wait.
const void *maplibre_visionos_command_queue(const MaplibreVisionOSMap *map);

// Terrain elevation in metres at a location from the loaded DEM tiles; NaN when unknown.
float maplibre_visionos_terrain_elevation(const MaplibreVisionOSMap *map, double latitude,
                                          double longitude);

// Writes a line of the host's into the renderer's log file.
void maplibre_visionos_note(const char *message);

// Bytes the process may still take before the system would kill it (os_proc_available_memory);
// below a reserve the map takes nothing new on. Zero when the host cannot tell.
void maplibre_visionos_set_available_memory(MaplibreVisionOSMap *map, uint64_t available_bytes);

// Replaces the inline GeoJSON document of a named GeoJSON source in-place without recreating
// the map or flushing loaded raster/vector/DEM tiles.
bool maplibre_visionos_set_geojson_source(MaplibreVisionOSMap *map, const char *source_name,
                                          const char *geojson_json);

void maplibre_visionos_destroy(MaplibreVisionOSMap *map);

// Images labels name in a namespace, made by the host on request, such as road shields drawn
// from route attributes. The callback's statuses:
#define MAPLIBRE_VISIONOS_IMAGE_READY 0        // drawn into *image
#define MAPLIBRE_VISIONOS_IMAGE_ABSENT 1       // no image; the style's fallback is drawn
#define MAPLIBRE_VISIONOS_IMAGE_FAILED 2       // cannot be drawn; kept until invalidated
#define MAPLIBRE_VISIONOS_IMAGE_UNAVAILABLE 3  // not yet; asked for again later

typedef struct {
    uint32_t width;
    uint32_t height;
    // width * height * 4 straight-alpha RGBA bytes, copied by the map before it calls release.
    const uint8_t *rgba;
    // Called once with release_context after the copy, on the callback's thread, whatever the
    // callback answered; NULL when nothing needs freeing.
    void (*release)(void *release_context);
    void *release_context;
    // Image pixels per layout pixel.
    float pixel_ratio;
    // Whether anchor_x/anchor_y (image pixels from the top-left) are placed where the centre
    // would be, such as the body of a shield with a banner above it.
    bool has_anchor;
    float anchor_x;
    float anchor_y;
} MaplibreVisionOSImage;

// Draws the image named `id` (after "namespace:") for `pixel_ratio` device pixels per layout
// pixel. Called on tile worker threads, several at once, never on the thread that draws.
typedef int32_t (*MaplibreVisionOSImageCallback)(void *context, const char *id,
                                                 float pixel_ratio,
                                                 MaplibreVisionOSImage *image);

/** Make `callback` draw the images named "namespace:..." that neither sprite nor style supply.
 * `generation` names the resources they come from; `context` must outlive the map. */
bool maplibre_visionos_register_image_provider(MaplibreVisionOSMap *map, const char *namespace_,
                                               const char *generation,
                                               MaplibreVisionOSImageCallback callback,
                                               void *context);
/** Forget a namespace's images and request again the tiles that drew one; returns their count. */
size_t maplibre_visionos_invalidate_images(MaplibreVisionOSMap *map, const char *namespace_);


#ifdef __cplusplus
}
#endif

#endif
