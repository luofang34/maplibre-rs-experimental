//! The ground under a label: the height its anchor is drawn at, and how far the terrain under
//! each of its glyphs stands above or below the terrain under the anchor.

use crate::{
    coords::WorldTileCoords,
    sdf::{Feature, SymbolLayerData},
    style::layer::SymbolPaint,
    tcs::world::World,
    terrain::coverage::TerrainCoverageIndex,
};

/// Heights within this of the ground count as on it, so DEM sampling noise never hides a label.
const BURIAL_TOLERANCE_METERS: f32 = 1.0;

/// The terrain elevation in metres at `point`, in tile units of the tile at `coords`; sea level
/// where no DEM covers it.
pub(in crate::sdf) fn elevation_at(world: &World, coords: WorldTileCoords, point: [f64; 2]) -> f32 {
    let scale = 2_f64.powi(i32::from(u8::from(coords.z)));
    let x = (f64::from(coords.x) + point[0] / 4096.0) / scale;
    let y = (f64::from(coords.y) + point[1] / 4096.0) / scale;
    world
        .resources
        .get::<TerrainCoverageIndex>()
        .and_then(|index| {
            index
                .elevation_at(&world.tiles, x, y)
                .or_else(|| index.elevation_cached(&world.tiles, x, y))
        })
        .unwrap_or(0.0) as f32
}

/// The terrain elevation under the feature's text anchor.
pub(in crate::sdf) fn elevation(world: &World, layer: &SymbolLayerData, feature: &Feature) -> f32 {
    elevation_at(world, layer.coords, anchor_point(feature))
}

fn anchor_point(feature: &Feature) -> [f64; 2] {
    [
        f64::from(feature.text_anchor.x),
        f64::from(feature.text_anchor.y),
    ]
}

/// The height of the label's anchor: the terrain under it unless the style places the symbol
/// at an absolute height, plus the style's height offset.
pub(in crate::sdf) fn symbol_elevation(
    world: &World,
    layer: &SymbolLayerData,
    feature: &Feature,
    paint: &SymbolPaint,
    zoom: f64,
) -> f32 {
    let terrain = elevation(world, layer, feature);
    if !paint.uses_shared_height() {
        return terrain;
    }
    let base = if paint.height_follows_ground("text") {
        terrain
    } else {
        0.0
    };
    base + paint.height_offset("text", &feature.data.properties, zoom)
}

/// Whether the anchor at `height` is inside the terrain surface below it, where the depth test
/// hides it. Such a label must not hold collision space or answer queries.
pub(in crate::sdf) fn buried_in_terrain(
    world: &World,
    layer: &SymbolLayerData,
    feature: &Feature,
    height: f32,
) -> bool {
    elevation(world, layer, feature) - height > BURIAL_TOLERANCE_METERS
}

/// The ground a label along a line stands on. Its anchor is drawn at [`Self::elevation`], and
/// each glyph that follows the ground rises with the terrain under it, so a label climbing a
/// slope lies on the slope rather than through it.
pub(in crate::sdf) struct LabelGround<'a> {
    world: &'a World,
    coords: WorldTileCoords,
    /// The height of the label's anchor, as [`symbol_elevation`] gives it.
    pub(in crate::sdf) elevation: f32,
    /// The terrain under the anchor, which glyphs rise from; `None` when the label's height
    /// does not follow the ground.
    anchor_terrain: Option<f32>,
}

impl<'a> LabelGround<'a> {
    pub(in crate::sdf) fn new(
        world: &'a World,
        layer: &SymbolLayerData,
        feature: &Feature,
        paint: &SymbolPaint,
        zoom: f64,
    ) -> Self {
        let follows = paint.height_follows_ground("text")
            && world.resources.get::<TerrainCoverageIndex>().is_some();
        Self {
            world,
            coords: layer.coords,
            elevation: symbol_elevation(world, layer, feature, paint, zoom),
            anchor_terrain: follows.then(|| elevation(world, layer, feature)),
        }
    }

    /// How far the terrain at `point`, in tile units, stands above the terrain under the
    /// anchor, in metres: what a glyph there rises by.
    pub(in crate::sdf) fn rise_at(&self, point: [f64; 2]) -> f32 {
        self.anchor_terrain.map_or(0.0, |anchor| {
            elevation_at(self.world, self.coords, point) - anchor
        })
    }
}
