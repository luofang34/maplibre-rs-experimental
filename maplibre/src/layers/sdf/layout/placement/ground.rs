//! The ground under a label: the height its anchor is drawn at, and how far the terrain under
//! each of its glyphs stands above or below the terrain under the anchor.

use cgmath::{InnerSpace, Matrix4, Vector4};

use crate::{
    coords::{TileCoords, WorldTileCoords, Zoom, ZOOM_BOUNDS},
    render::{camera::ViewProjection, projection::ShaderProjectionData, view_state::ViewState},
    sdf::{Feature, SymbolLayerData},
    style::layer::SymbolPaint,
    tcs::world::World,
    terrain::coverage::TerrainCoverageIndex,
};

/// Precomputed per-tile matrices and constants shared across all features and glyphs in a tile.
pub(in crate::layers::sdf) struct TileProjector {
    tile: TileCoords,
    pub(in crate::layers::sdf) flat_mvp: Option<Matrix4<f64>>,
    transition: f64,
    clip_cull: bool,
    globe_matrix: Option<Matrix4<f64>>,
    clipping_plane: Option<Vector4<f64>>,
    radius_meters: f64,
    pub(in crate::layers::sdf) z_scale: f64,
    pub(in crate::layers::sdf) tile_y: f64,
}

impl TileProjector {
    pub(in crate::layers::sdf) fn new(
        coords: WorldTileCoords,
        gpu_vp: ViewProjection,
        zoom: Zoom,
        projection: &ShaderProjectionData,
    ) -> Option<Self> {
        let tile = canonical_tile(coords)?;
        let transition = f64::from(projection.transition);
        let flat_mvp = (transition < 1.0).then(|| {
            gpu_vp
                .to_model_view_projection(coords.transform_for_zoom(zoom))
                .get()
        });
        let (globe_matrix, clipping_plane) = if transition > 0.0 {
            (
                Matrix4::<f32>::from(projection.main_matrix).cast::<f64>(),
                Vector4::<f32>::from(projection.clipping_plane).cast::<f64>(),
            )
        } else {
            (None, None)
        };
        Some(Self {
            tile,
            flat_mvp,
            transition,
            clip_cull: projection.transition >= 0.98,
            globe_matrix,
            clipping_plane,
            radius_meters: f64::from(projection.radius_meters),
            z_scale: 2_f64.powi(i32::from(u8::from(coords.z))),
            tile_y: f64::from(coords.y),
        })
    }

    #[inline]
    pub(in crate::layers::sdf) fn project(
        &self,
        anchor: [f64; 2],
        elevation: f64,
    ) -> Option<Vector4<f64>> {
        if self.transition <= 0.0 {
            return self
                .flat_mvp
                .map(|mvp| mvp * Vector4::new(anchor[0], anchor[1], elevation, 1.0));
        }
        let surface = crate::projection::globe::project_tile_coordinates_to_unit_sphere(
            self.tile.x,
            self.tile.y,
            u8::from(self.tile.z),
            anchor[0],
            anchor[1],
        );
        let plane = self.clipping_plane?;
        if self.clip_cull && plane.dot(surface.extend(1.0)) < 0.0 {
            return None;
        }
        let globe =
            self.globe_matrix? * (surface * (1.0 + elevation / self.radius_meters)).extend(1.0);
        if self.transition >= 1.0 {
            return Some(globe);
        }
        let flat = self.flat_mvp? * Vector4::new(anchor[0], anchor[1], elevation, 1.0);
        Some(flat * (1.0 - self.transition) + globe * self.transition)
    }
}

pub(in crate::layers::sdf) fn canonical_tile(coords: WorldTileCoords) -> Option<TileCoords> {
    let bound = ZOOM_BOUNDS.get(usize::from(u8::from(coords.z))).copied()?;
    let count = i32::try_from(bound).ok()?;
    (coords.y >= 0 && coords.y < count).then_some(TileCoords {
        x: coords.x.rem_euclid(count) as u32,
        y: coords.y as u32,
        z: coords.z,
    })
}

#[inline]
pub(in crate::layers::sdf) fn project_with(
    projector: Option<&TileProjector>,
    coords: WorldTileCoords,
    anchor: [f64; 2],
    elevation: f64,
    view: &ViewState,
    projection: &ShaderProjectionData,
) -> Option<Vector4<f64>> {
    match projector {
        Some(projector) => projector.project(anchor, elevation),
        None => project(coords, anchor, elevation, view, projection),
    }
}

pub(in crate::layers::sdf) fn project(
    coords: WorldTileCoords,
    anchor: [f64; 2],
    elevation: f64,
    view: &ViewState,
    projection: &ShaderProjectionData,
) -> Option<Vector4<f64>> {
    let tile = canonical_tile(coords)?;
    let transition = f64::from(projection.transition);
    if transition <= 0.0 {
        return Some(
            view.gpu_view_projection()
                .to_model_view_projection(coords.transform_for_zoom(view.zoom()))
                .get()
                * Vector4::new(anchor[0], anchor[1], elevation, 1.0),
        );
    }
    let surface = crate::projection::globe::project_tile_coordinates_to_unit_sphere(
        tile.x,
        tile.y,
        u8::from(tile.z),
        anchor[0],
        anchor[1],
    );
    let plane = Vector4::<f32>::from(projection.clipping_plane).cast::<f64>()?;
    if projection.transition >= 0.98 && plane.dot(surface.extend(1.0)) < 0.0 {
        return None;
    }
    let globe = Matrix4::<f32>::from(projection.main_matrix).cast::<f64>()?
        * (surface * (1.0 + elevation / f64::from(projection.radius_meters))).extend(1.0);
    if transition >= 1.0 {
        return Some(globe);
    }
    let flat = view
        .gpu_view_projection()
        .to_model_view_projection(coords.transform_for_zoom(view.zoom()))
        .get()
        * Vector4::new(anchor[0], anchor[1], elevation, 1.0);
    Some(flat * (1.0 - transition) + globe * transition)
}

/// Heights within this of the ground count as on it, so DEM sampling noise never hides a label.
const BURIAL_TOLERANCE_METERS: f32 = 1.0;

/// The terrain elevation in metres at `point`, in tile units of the tile at `coords`; sea level
/// where no DEM covers it.
pub(in crate::layers::sdf) fn elevation_at(
    world: &World,
    coords: WorldTileCoords,
    point: [f64; 2],
) -> f32 {
    let scale = 2_f64.powi(i32::from(u8::from(coords.z)));
    let x = (f64::from(coords.x) + point[0] / 4096.0) / scale;
    let y = (f64::from(coords.y) + point[1] / 4096.0) / scale;
    world
        .resources
        .get::<TerrainCoverageIndex>()
        .and_then(|index| {
            index.elevation_at(&world.tiles, x, y).or_else(|| {
                index.loaded_ground_cached(&world.tiles, x, y, crate::terrain::DemDetail::Finest)
            })
        })
        .unwrap_or(0.0) as f32
}

/// The terrain elevation under the feature's text anchor.
pub(in crate::layers::sdf) fn elevation(
    world: &World,
    layer: &SymbolLayerData,
    feature: &Feature,
) -> f32 {
    elevation_at(world, layer.coords, anchor_point(feature))
}

fn anchor_point(feature: &Feature) -> [f64; 2] {
    [
        f64::from(feature.text_anchor.x),
        f64::from(feature.text_anchor.y),
    ]
}

/// Whether the anchor at `height` is inside the terrain surface below it, where the depth test
/// hides it. Such a label must not hold collision space or answer queries.
pub(in crate::layers::sdf) fn buried_in_terrain(
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
pub(in crate::layers::sdf) struct LabelGround<'a> {
    world: &'a World,
    coords: WorldTileCoords,
    /// The height of the label's anchor above sea level.
    pub(in crate::layers::sdf) elevation: f32,
    /// The terrain under the anchor, which glyphs rise from; `None` when the label's height
    /// does not follow the ground.
    anchor_terrain: Option<f32>,
}

impl<'a> LabelGround<'a> {
    pub(in crate::layers::sdf) fn flat(world: &'a World, coords: WorldTileCoords) -> Self {
        Self {
            world,
            coords,
            elevation: 0.0,
            anchor_terrain: None,
        }
    }

    pub(in crate::layers::sdf) fn new(
        world: &'a World,
        layer: &SymbolLayerData,
        feature: &Feature,
        paint: &SymbolPaint,
        zoom: f64,
    ) -> Self {
        let terrain = elevation(world, layer, feature);
        let follows = paint.height_follows_ground("text")
            && world.resources.get::<TerrainCoverageIndex>().is_some();
        let height = if !paint.uses_shared_height() && paint.height_follows_ground("text") {
            terrain
        } else {
            let base = if paint.height_follows_ground("text") {
                terrain
            } else {
                0.0
            };
            base + paint.height_offset("text", &feature.data.properties, zoom)
        };
        Self {
            world,
            coords: layer.coords,
            elevation: height,
            anchor_terrain: follows.then_some(terrain),
        }
    }

    /// How far the terrain at `point`, in tile units, stands above the terrain under the
    /// anchor, in metres: what a glyph there rises by.
    pub(in crate::layers::sdf) fn rise_at(&self, point: [f64; 2]) -> f32 {
        self.anchor_terrain.map_or(0.0, |anchor| {
            elevation_at(self.world, self.coords, point) - anchor
        })
    }
}
