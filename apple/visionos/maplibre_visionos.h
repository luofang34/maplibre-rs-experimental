// C ABI of the maplibre-rs renderer for the visionOS host app. Mirrors src/lib.rs.
#ifndef MAPLIBRE_VISIONOS_H
#define MAPLIBRE_VISIONOS_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct MaplibreVisionOSMap MaplibreVisionOSMap;

const char *maplibre_visionos_version(void);

MaplibreVisionOSMap *maplibre_visionos_create(const char *style_json, uint32_t width,
                                              uint32_t height, const char *cache_dir);

uint32_t maplibre_visionos_width(const MaplibreVisionOSMap *map);
uint32_t maplibre_visionos_height(const MaplibreVisionOSMap *map);

// Returns the id<MTLTexture> of the frame, owned by the map and valid until the next render.
const void *maplibre_visionos_render(MaplibreVisionOSMap *map, double anchor_latitude,
                                     double anchor_longitude, double anchor_altitude_meters,
                                     const float *view, const float *tangents, float near,
                                     float far, double timestamp_seconds);

void maplibre_visionos_destroy(MaplibreVisionOSMap *map);

#ifdef __cplusplus
}
#endif

#endif
