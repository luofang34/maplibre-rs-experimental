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
/** Query placed labels/icons at a pixel (y down). Returns required bytes including NUL.
 * Only writes when buffer is non-null and capacity is sufficient. */
size_t maplibre_visionos_query_symbols(const MaplibreVisionOSMap *map, double x, double y,
                                     char *buffer, size_t capacity);
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

void maplibre_visionos_destroy(MaplibreVisionOSMap *map);

#ifdef __cplusplus
}
#endif

#endif
