//! Synchronizes host-pushed `video` and `canvas` RGBA frames into visible raster tiles.

use std::{collections::HashMap, sync::Arc};

use image::RgbaImage;

use crate::{
    coords::WorldTileCoords,
    raster::{
        image_source, resource::RasterResources, AvailableRasterLayerData, RasterLayerData,
        RasterLayersDataComponent,
    },
    render::{
        eventually::Eventually,
        projection::raster_source_regions,
        view_state::{ViewState, ViewStatePadding},
    },
    style::{
        source::{CanvasSource, MediaFrame, Source, VideoSource},
        Style,
    },
    tcs::world::World,
};

#[derive(Default)]
struct DynamicMediaState {
    /// Cached `(generation, Arc<RgbaImage>)` per source name so panning/zooming between frame
    /// pushes reuses the decoded buffer without reallocating.
    decoded: HashMap<String, (u64, Arc<RgbaImage>)>,
    /// Last `generation` resampled into `world.tiles` at `(source_name, coords)`.
    tiles: HashMap<(String, WorldTileCoords), u64>,
}

/// Returns `true` when `source_name` names a `"video"` or `"canvas"` source in `style`.
pub(crate) fn is_media_source(style: &Style, source_name: Option<&str>) -> bool {
    source_name
        .and_then(|name| style.sources.get(name))
        .is_some_and(|source| matches!(source, Source::Video(_) | Source::Canvas(_)))
}

/// Geographic bounding box `(west, south, east, north)` of a 4-corner quad when it does not wrap
/// across the antimeridian.
fn quad_bounds(coordinates: [[f64; 2]; 4]) -> Option<(f64, f64, f64, f64)> {
    let longitudes = coordinates.map(|[longitude, _]| longitude);
    let latitudes = coordinates.map(|[_, latitude]| latitude);
    let fold = |values: [f64; 4]| {
        values
            .into_iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), value| {
                (lo.min(value), hi.max(value))
            })
    };
    let ((west, east), (south, north)) = (fold(longitudes), fold(latitudes));
    (west >= -180.0 && east <= 180.0).then_some((west, south, east, north))
}

/// Resamples any visible tiles of `"video"` and `"canvas"` sources whose frame or corner
/// generation changed since they were last populated, marking their GPU bindings dirty for
/// [`super::upload_system::upload_system`].
pub(crate) fn sync_media_sources(style: &Style, view_state: &ViewState, world: &mut World) {
    let has_active_frame = style.sources.values().any(|source| {
        matches!(
            source,
            Source::Video(VideoSource { frame: Some(_), .. })
                | Source::Canvas(CanvasSource { frame: Some(_), .. })
        )
    });
    if !has_active_frame {
        return;
    }

    let Ok(regions) = raster_source_regions(style, view_state, world, ViewStatePadding::Tight)
    else {
        return;
    };

    let mut state = world
        .resources
        .remove::<DynamicMediaState>()
        .unwrap_or_default();

    // Prune cache entries for sources that left the style.
    state
        .decoded
        .retain(|name, _| is_media_source(style, Some(name.as_str())));
    state
        .tiles
        .retain(|(name, _), _| is_media_source(style, Some(name.as_str())));

    for (source_id, covering_tiles) in regions {
        let Some(name) = source_id.name() else {
            continue;
        };
        let (frame, coordinates, generation) = match style.sources.get(name) {
            Some(Source::Video(video)) => {
                (video.frame.as_ref(), video.coordinates, video.generation)
            }
            Some(Source::Canvas(canvas)) => {
                (canvas.frame.as_ref(), canvas.coordinates, canvas.generation)
            }
            _ => continue,
        };
        let Some(frame) = frame else {
            continue;
        };
        let Some(picture) = cached_frame_image(&mut state, name, frame, generation) else {
            continue;
        };
        let bounds = quad_bounds(coordinates);

        for coords in covering_tiles {
            if bounds.is_some_and(|bounds| !crate::io::tile_sources::tile_in_bounds(coords, bounds))
            {
                continue;
            }
            let has_tile_data = world
                .tiles
                .query::<&RasterLayersDataComponent>(coords)
                .is_some_and(|component| component.has_source_image(&source_id));
            let key = (name.to_owned(), coords);
            if has_tile_data && state.tiles.get(&key) == Some(&generation) {
                continue;
            }

            let resampled = image_source::render_tile(&picture, coordinates, coords)
                .unwrap_or_else(|| RgbaImage::new(1, 1));
            let exists = world
                .tiles
                .query::<&RasterLayersDataComponent>(coords)
                .is_some();
            let Some(mut tile) = world.tiles.spawn_mut(coords) else {
                continue;
            };
            if !exists {
                tile.insert(RasterLayersDataComponent::default());
            }
            if let Some(component) = world
                .tiles
                .query_mut::<&mut RasterLayersDataComponent>(coords)
            {
                component.record(RasterLayerData::Available(AvailableRasterLayerData {
                    coords,
                    source: source_id.clone(),
                    image: resampled,
                }));
            }
            if let Some(Eventually::Initialized(raster)) =
                world.resources.get_mut::<Eventually<RasterResources>>()
            {
                raster.tile_data_changed(&source_id, coords);
            }
            state.tiles.insert(key, generation);
        }
    }

    world.resources.insert(state);
}

fn cached_frame_image(
    state: &mut DynamicMediaState,
    name: &str,
    frame: &MediaFrame,
    generation: u64,
) -> Option<Arc<RgbaImage>> {
    if let Some((cached_gen, image)) = state.decoded.get(name) {
        if *cached_gen == generation {
            return Some(Arc::clone(image));
        }
    }
    let image = Arc::new(RgbaImage::from_raw(
        frame.width,
        frame.height,
        frame.rgba.to_vec(),
    )?);
    state
        .decoded
        .insert(name.to_owned(), (generation, Arc::clone(&image)));
    state.tiles.retain(|(src, _), _| src != name);
    Some(image)
}
