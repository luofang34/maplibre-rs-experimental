// Cross-platform C11 / C++ ABI for embedding maplibre-rs.
#ifndef MAPLIBRE_H
#define MAPLIBRE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/** Opaque map instance owning an offscreen renderer and worker runtime. */
typedef struct MaplibreMap MaplibreMap;

/** Opaque thread-safe handle (`Send + Sync`) capable of queuing map updates from any thread. */
typedef struct MaplibreHandle MaplibreHandle;

/** Initial configuration passed to `maplibre_map_create`. */
typedef struct {
    /** Physical render target width in pixels (must be > 0). */
    uint32_t width;
    /** Physical render target height in pixels (must be > 0). */
    uint32_t height;
    /** Physical-to-logical pixel ratio; `<= 0.0` defaults to `1.0`. */
    double pixel_ratio;
    /** Optional NUL-terminated UTF-8 path for HTTP tile caching, or `NULL`. */
    const char *cache_dir;
    /** Maximum camera pitch in degrees; `<= 0.0` uses the default `60.0` limit. */
    double max_pitch_deg;
} MaplibreConfig;

/** Map camera state in geographic degrees and continuous MapLibre zoom. */
typedef struct {
    /** Center latitude in degrees (`[-85.0511, 85.0511]` in constrained Mercator mode). */
    double latitude;
    /** Center longitude in degrees. */
    double longitude;
    /** Continuous MapLibre zoom level. */
    double zoom;
    /** Bearing in degrees clockwise from north. */
    double bearing_deg;
    /** Pitch in degrees away from nadir. */
    double pitch_deg;
} MaplibreCamera;

/** Static NUL-terminated library version string (e.g. `"0.1.0"`). */
const char *maplibre_version(void);

/**
 * Thread-local NUL-terminated error message from the most recent `maplibre_*` call on the
 * calling thread. Returns an empty string (`""`) when the most recent call succeeded.
 * Valid until the next `maplibre_*` call on the same thread.
 */
const char *maplibre_last_error_message(void);

/** Frees a heap-allocated string returned by `maplibre_map_query_rendered_features_json`. */
void maplibre_string_free(char *s);

/**
 * Creates a map configured by `config` and `style_json` (NUL-terminated UTF-8 MapLibre Style JSON).
 * Returns `NULL` on failure and records details in `maplibre_last_error_message()`.
 */
MaplibreMap *maplibre_map_create(const MaplibreConfig *config, const char *style_json);

/** Destroys a map created by `maplibre_map_create`; `NULL` is ignored. */
void maplibre_map_destroy(MaplibreMap *map);

/** Resizes `map` to `width` x `height` physical pixels and updates `pixel_ratio` (`<= 0.0` -> `1.0`). */
bool maplibre_map_resize(MaplibreMap *map, uint32_t width, uint32_t height, double pixel_ratio);

/** Clones a thread-safe handle to `map` that may be used from any OS thread. */
MaplibreHandle *maplibre_map_clone_handle(const MaplibreMap *map);

/** Frees a handle returned by `maplibre_map_clone_handle`; `NULL` is ignored. */
void maplibre_handle_destroy(MaplibreHandle *handle);

/** Queues an asynchronous camera jump from any thread via `handle`. */
bool maplibre_handle_set_camera(const MaplibreHandle *handle, const MaplibreCamera *camera);

/** Validates `style_json` and queues an asynchronous style replacement from any thread. */
bool maplibre_handle_set_style_json(const MaplibreHandle *handle, const char *style_json);

/**
 * Validates `state_json` (JSON object) and queues an asynchronous `feature-state` merge from any
 * thread. `source_layer` may be `NULL` or `""` for GeoJSON sources.
 */
bool maplibre_handle_set_feature_state_json(const MaplibreHandle *handle, const char *source,
                                            const char *source_layer, const char *feature_id,
                                            const char *state_json);

/** Reads the map's current camera into `*out_camera`. */
bool maplibre_map_get_camera(const MaplibreMap *map, MaplibreCamera *out_camera);

/** Jumps the map's camera to `*camera`. */
bool maplibre_map_set_camera(MaplibreMap *map, const MaplibreCamera *camera);

/** Replaces the map's style document with `style_json` while retaining the camera. */
bool maplibre_map_set_style_json(MaplibreMap *map, const char *style_json);

/** Sets a paint property on `layer_id` from JSON `value_json` (`"null"` restores default). */
bool maplibre_map_set_paint_property_json(MaplibreMap *map, const char *layer_id,
                                          const char *property_name, const char *value_json);

/** Sets a layout property on `layer_id` from JSON `value_json` (`"null"` restores default). */
bool maplibre_map_set_layout_property_json(MaplibreMap *map, const char *layer_id,
                                           const char *property_name, const char *value_json);

/** Sets or clears (`NULL` or `"null"`) a filter expression on `layer_id`. */
bool maplibre_map_set_filter_json(MaplibreMap *map, const char *layer_id, const char *filter_json);

/** Toggles `layer_id` layout `visibility` between `"visible"` (`true`) and `"none"` (`false`). */
bool maplibre_map_set_layer_visibility(MaplibreMap *map, const char *layer_id, bool visible);

/** Replaces the inline GeoJSON document of `source_id` with `geojson_json`. */
bool maplibre_map_set_geojson_source_data(MaplibreMap *map, const char *source_id,
                                          const char *geojson_json);

/**
 * Merges JSON object `state_json` into the runtime `feature-state` of `feature_id` in `source`
 * (and optional `source_layer`, `NULL` for GeoJSON sources).
 */
bool maplibre_map_set_feature_state_json(MaplibreMap *map, const char *source,
                                         const char *source_layer, const char *feature_id,
                                         const char *state_json);

/**
 * Removes runtime `feature-state` entries in `source` / optional `source_layer`.
 * `feature_id` may be `NULL` to clear all features; `state_key` may be `NULL` to clear all keys.
 */
bool maplibre_map_remove_feature_state(MaplibreMap *map, const char *source,
                                       const char *source_layer, const char *feature_id,
                                       const char *state_key);

/**
 * Adds or replaces a named sprite/icon image from straight-alpha RGBA8 bytes (`rgba_len` must
 * equal `width * height * 4`). `pixel_ratio <= 0.0` defaults to `1.0`.
 */
bool maplibre_map_add_image_rgba8(MaplibreMap *map, const char *image_id, uint32_t width,
                                  uint32_t height, const uint8_t *rgba, size_t rgba_len,
                                  float pixel_ratio, bool sdf);

/** Returns `true` when the map has dirty state, camera motion, or arrived/pending tile work. */
bool maplibre_map_needs_redraw(const MaplibreMap *map);

/**
 * Renders a live/settled frame and copies tightly packed RGBA8 pixels into `out_rgba`
 * (`out_rgba_len` must be at least `width * height * 4`).
 */
bool maplibre_map_render_rgba8(MaplibreMap *map, uint8_t *out_rgba, size_t out_rgba_len);

/**
 * Queries rendered features intersecting the screen-pixel box `[min_x, min_y, max_x, max_y]`
 * (pass `min_x == max_x` and `min_y == max_y` for a point query). `options_json` may be `NULL`
 * or a JSON object `{"layers": [...], "filter": [...]}`.
 * Returns a heap-allocated NUL-terminated JSON array string that the caller frees with
 * `maplibre_string_free`, or `NULL` on error.
 */
char *maplibre_map_query_rendered_features_json(const MaplibreMap *map, double min_x, double min_y,
                                                double max_x, double max_y,
                                                const char *options_json);

#ifdef __cplusplus
}
#endif

#endif /* MAPLIBRE_H */
