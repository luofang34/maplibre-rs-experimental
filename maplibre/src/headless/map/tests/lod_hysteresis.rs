//! A map camera that moves a hair each frame keeps the levels its tiles were drawn at: a
//! tile sitting on a level threshold must not switch between its fine and coarse version.

use crate::{
    coords::{WorldTileCoords, Zoom},
    headless::{create_headless_renderer, HeadlessPlugin},
    projection::tile_covering::covers,
    raster::{DefaultRasterTransferables, RasterPlugin},
    render::{
        projection::{raster_source_regions, terrain_region, view_region_for_projection},
        tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::{ViewState, ViewStatePadding},
        RenderPlugin,
    },
    style::Style,
    tcs::world::World,
    terrain::{DefaultDemTransferables, TerrainPlugin},
};

const DEM_TILE_SIZE: u32 = 256;

/// A riding view: low over the ground, pitched toward the horizon, on the globe with terrain.
fn style() -> Style {
    serde_json::from_value(serde_json::json!({
        "version": 8, "center": [8.545, 47.397], "zoom": 16.3, "pitch": 80,
        "projection": {"type": "vertical-perspective"},
        "terrain": {"source": "dem"},
        "sources": {
            "dem": {"type": "raster-dem", "tiles": ["offline://dem"], "tileSize": DEM_TILE_SIZE,
                "maxzoom": 15, "encoding": "terrarium"},
            "imagery": {"type": "raster", "tiles": ["offline://imagery"], "tileSize": 256,
                "maxzoom": 19}
        },
        "layers": [
            {"id": "background", "type": "background",
                "paint": {"background-color": "#101010"}},
            {"id": "imagery", "type": "raster", "source": "imagery"}
        ]
    }))
    .expect("style")
}

/// The coverings a frame selects, each splitting the quadtree by its own rule.
#[derive(Clone, Copy, Debug)]
enum Covering {
    Terrain,
    View,
    Imagery,
}

impl Covering {
    /// The tiles `view` selects in `world`; an empty world holds no level history.
    fn select(self, style: &Style, view: &ViewState, world: &World) -> Vec<WorldTileCoords> {
        let tight = ViewStatePadding::Tight;
        let mut tiles: Vec<_> = match self {
            Self::Terrain => terrain_region(style, view, world, DEM_TILE_SIZE, tight)
                .expect("terrain covering")
                .map(|region| region.iter().collect())
                .unwrap_or_default(),
            Self::View => {
                let level = view.zoom().zoom_level(DEFAULT_TILE_SIZE);
                view_region_for_projection(style, view, world, level, tight)
                    .expect("view covering")
                    .map(|region| region.iter().collect())
                    .unwrap_or_default()
            }
            Self::Imagery => raster_source_regions(style, view, world, tight)
                .expect("raster covering")
                .into_iter()
                .flat_map(|(_, tiles)| tiles)
                .collect(),
        };
        tiles.sort();
        tiles
    }

    /// The tiles the last frame drew: the terrain surface it built, or for the view and the
    /// imagery, what its systems select from the level history that frame snapshotted.
    fn drawn(self, style: &Style, map: &super::super::HeadlessMap) -> Vec<WorldTileCoords> {
        match self {
            Self::Terrain => {
                let mut tiles = crate::terrain::drawn_surface_tiles(map.world()).to_vec();
                tiles.sort();
                tiles
            }
            Self::View | Self::Imagery => self.select(style, map.view_state(), map.world()),
        }
    }
}

/// Whether `after` replaces some tile of `before` by its children or by its parent, and
/// changes nothing else: a level switch, not ground entering or leaving the view.
fn only_level_changes(before: &[WorldTileCoords], after: &[WorldTileCoords]) -> bool {
    let related = |a: WorldTileCoords, b: WorldTileCoords| covers(a, b) || covers(b, a);
    let removed: Vec<_> = before.iter().filter(|t| !after.contains(t)).collect();
    let added: Vec<_> = after.iter().filter(|t| !before.contains(t)).collect();
    !removed.is_empty()
        && removed
            .iter()
            .all(|r| added.iter().any(|a| related(**r, **a)))
        && added
            .iter()
            .all(|a| removed.iter().any(|r| related(**r, **a)))
}

/// A zoom above `from` at which `covering`, selected without any level history, switches a
/// tile's level and nothing else.
fn level_threshold(covering: Covering, style: &Style, view: &ViewState, from: f64) -> f64 {
    let at = |zoom: f64| {
        let mut view = view.clone();
        view.update_zoom(Zoom::new(zoom));
        covering.select(style, &view, &World::default())
    };
    let step = 1e-3;
    for index in 0..1000 {
        let (mut low, mut high) = (from + step * index as f64, from + step * (index + 1) as f64);
        let below = at(low);
        if below == at(high) {
            continue;
        }
        for _ in 0..50 {
            let middle = (low + high) / 2.0;
            if at(middle) == below {
                low = middle;
            } else {
                high = middle;
            }
        }
        if only_level_changes(&at(low), &at(high)) {
            return (low + high) / 2.0;
        }
    }
    panic!("{covering:?}: no level threshold within a zoom of {from}");
}

async fn map(style: &Style) -> super::super::HeadlessMap {
    let (kernel, renderer) = create_headless_renderer(1024, 768, None)
        .await
        .expect("renderer");
    super::super::HeadlessMap::new(
        style.clone(),
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
            Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
            Box::new(HeadlessPlugin::new(false)),
        ],
    )
    .expect("map")
}

/// Frames whose camera stands a few micrometres to either side of a level threshold of
/// `covering` all draw the levels the first of them drew.
async fn wavering_keeps_levels(covering: Covering) {
    let style = style();
    let mut map = map(&style).await;
    let start = map.view_state().zoom().value();
    let edge = level_threshold(covering, &style, map.view_state(), start);
    let poses = [edge - 1e-9, edge + 1e-9];
    let mut view = map.view_state().clone();
    let [below, above] = poses.map(|zoom| {
        view.update_zoom(Zoom::new(zoom));
        covering.select(&style, &view, &World::default())
    });
    assert!(
        only_level_changes(&below, &above),
        "{covering:?}: without a history the two poses draw a tile at different levels"
    );
    let mut drawn = Vec::new();
    for frame in 0..12 {
        map.view_state_mut()
            .update_zoom(Zoom::new(poses[frame % poses.len()]));
        map.run_frame().expect("frame");
        let tiles = covering.drawn(&style, &map);
        assert!(!tiles.is_empty(), "{covering:?}: frame {frame} drew tiles");
        drawn.push(tiles);
    }
    for (frame, tiles) in drawn.iter().enumerate().skip(1) {
        assert_eq!(
            tiles, &drawn[0],
            "{covering:?}: frame {frame} switched levels under a camera that barely moved"
        );
    }
}

#[tokio::test]
async fn a_camera_wavering_across_a_terrain_level_threshold_keeps_the_drawn_levels() {
    wavering_keeps_levels(Covering::Terrain).await;
}

#[tokio::test]
async fn a_camera_wavering_across_a_view_level_threshold_keeps_the_drawn_levels() {
    wavering_keeps_levels(Covering::View).await;
}

#[tokio::test]
async fn a_camera_wavering_across_a_raster_level_threshold_keeps_the_drawn_levels() {
    wavering_keeps_levels(Covering::Imagery).await;
}
