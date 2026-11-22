use std::sync::Arc;

use cgmath::{InnerSpace, Vector3, Vector4};

use super::{mercator::max_constant_zoom_pitch, CoveringRequest, MERCATOR};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    projection::globe::{
        camera::GlobeCameraState,
        covering::TileElevationRange,
        covering_tiles::{
            covering_tiles_with_margins, elevation_for_tile_culling, frustum::GlobeFrustum,
            lod::LodContext, sort_by_focus, CoveringMargins, FarPlane, GlobeCoveringError,
            GlobeCoveringOptions,
        },
    },
    render::{
        resolved_view::ResolvedFrameView,
        view_state::{ViewState, ViewStatePadding},
        xr::PrefetchView,
    },
    style::Style,
    tcs::world::World,
};

const ANCHOR_SAFETY_FACTOR: f64 = 0.95;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GlobeAnchorKey {
    style_rev: u64,
    visible_level: u8,
    padding: ViewStatePadding,
    viewport: (u64, u64),
    max_culling_elevation_bits: u64,
    headroom_bits: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct GlobeCoveringAnchor {
    pub(crate) key: GlobeAnchorKey,
    pub(crate) far_plane: FarPlane,
    pub(crate) frustum: GlobeFrustum,
    pub(crate) clipping_plane: Vector4<f64>,
    pub(crate) focus_unit: Vector3<f64>,
    pub(crate) margins: CoveringMargins,
    pub(crate) padding: i32,
    pub(crate) tiles: Arc<[WorldTileCoords]>,
    pub(crate) level: u8,
}

impl GlobeCoveringAnchor {
    /// Checks whether `camera` and `new_key` fall strictly inside the certified geometric margins
    /// of this cached covering. Returns `Some((tiles, exact_order))` when identical, or `None`
    /// when a fresh quadtree walk is required.
    pub(crate) fn try_reuse(
        &self,
        new_key: &GlobeAnchorKey,
        camera: &GlobeCameraState,
    ) -> Option<(Arc<[WorldTileCoords]>, bool)> {
        if self.key != *new_key {
            return None;
        }
        let new_frustum = GlobeFrustum::from_camera(camera, self.far_plane);
        let mut max_plane_delta = 0.0_f64;
        for (p_new, p_old) in new_frustum
            .planes()
            .iter()
            .copied()
            .chain(std::iter::once(camera.clipping_plane()))
            .zip(
                self.frustum
                    .planes()
                    .iter()
                    .copied()
                    .chain(std::iter::once(self.clipping_plane)),
            )
        {
            let dn = (p_new.truncate() - p_old.truncate()).magnitude();
            let dw = (p_new.w - p_old.w).abs();
            max_plane_delta = max_plane_delta.max(self.margins.max_point_radius * dn + dw);
        }
        if max_plane_delta >= self.margins.min_plane_margin * ANCHOR_SAFETY_FACTOR {
            return None;
        }
        if self.margins.min_corner_margin.is_finite() {
            let mut max_corner_delta = 0.0_f64;
            for (c_new, c_old) in new_frustum
                .points()
                .iter()
                .zip(self.frustum.points().iter())
            {
                max_corner_delta = max_corner_delta.max((*c_new - *c_old).magnitude());
            }
            if max_corner_delta >= self.margins.min_corner_margin * ANCHOR_SAFETY_FACTOR {
                return None;
            }
        }
        let focus_delta = (camera.focus_unit_sphere() - self.focus_unit).magnitude();
        if focus_delta < self.margins.min_focus_margin * ANCHOR_SAFETY_FACTOR {
            Some((self.tiles.clone(), true))
        } else if self.padding == 0 {
            Some((self.tiles.clone(), false))
        } else {
            None
        }
    }

    /// Returns a copy of `self.tiles` re-sorted by `camera.focus_unit_sphere()` when `padding == 0`.
    pub(crate) fn sorted_for_camera(&self, camera: &GlobeCameraState) -> Arc<[WorldTileCoords]> {
        let mut vec = self.tiles.to_vec();
        sort_by_focus(&mut vec, camera.focus_unit_sphere());
        Arc::from(vec)
    }
}

pub(crate) fn eligible_globe_anchor_key<'a>(
    style: &Style,
    view_state: &ViewState,
    world: &'a World,
    visible_level: ZoomLevel,
    padding: ViewStatePadding,
) -> Option<(
    GlobeAnchorKey,
    CoveringRequest,
    std::borrow::Cow<'a, GlobeCameraState>,
)> {
    if style.terrain.is_some() || view_state.has_external_view() || view_state.pose_view().is_some()
    {
        return None;
    }
    let projection_type = style
        .projection
        .as_ref()
        .map_or(&MERCATOR, |specification| &specification.projection_type);
    if !projection_type.uses_globe_rendering(view_state.zoom().value()) {
        return None;
    }
    let view_request =
        CoveringRequest::view_for_style(style, visible_level, view_state.zoom().value());
    if u8::from(view_request.level) > 4 || !view_request.zoom_range.serves(view_request.level) {
        return None;
    }
    let has_prefetch = world
        .resources
        .get::<PrefetchView>()
        .and_then(|prefetch| prefetch.view_state.as_ref())
        .is_some();
    if has_prefetch {
        return None;
    }
    if padding == ViewStatePadding::Loose {
        if view_state.eye_settled() {
            return None;
        }
        let pitch_deg = view_state.camera().get_pitch().0.to_degrees().abs();
        let fov_deg = view_state.field_of_view().0.to_degrees();
        let needs_nadir_apron = (view_request.headroom_meters > 0.0 && pitch_deg > 45.0)
            || pitch_deg > max_constant_zoom_pitch(fov_deg);
        if needs_nadir_apron {
            return None;
        }
    }
    let camera = ResolvedFrameView::globe_camera_for_frame(world, style, view_state).ok()?;
    let key = GlobeAnchorKey {
        style_rev: style.revision(),
        visible_level: u8::from(view_request.level),
        padding,
        viewport: (view_state.width().to_bits(), view_state.height().to_bits()),
        max_culling_elevation_bits: elevation_for_tile_culling(
            &camera,
            view_state.center_elevation(),
        )
        .to_bits(),
        headroom_bits: view_request.headroom_meters.to_bits(),
    };
    Some((key, view_request, camera))
}

/// Whether vector globe stencil masks require the 32-unit border pass to seal curved seams.
pub(crate) fn needs_vector_stencil_borders(
    transition: f32,
    zoom: f64,
    external_view: bool,
    min_z: u8,
    max_z: u8,
) -> bool {
    if transition <= 0.0 {
        return false;
    }
    transition < 1.0
        || zoom >= super::super::PRECISE_GLOBE_ZOOM
        || external_view
        || (max_z >= 7 && min_z != max_z)
}

pub(crate) fn compute_globe_anchor(
    key: GlobeAnchorKey,
    view_request: CoveringRequest,
    padding: ViewStatePadding,
    camera: &GlobeCameraState,
    center_elevation: f64,
) -> Result<GlobeCoveringAnchor, GlobeCoveringError> {
    let pad = match padding {
        ViewStatePadding::Loose => 1,
        ViewStatePadding::Tight => 0,
    };
    let options = GlobeCoveringOptions {
        zoom: view_request.level,
        requested_zoom: view_request.requested_zoom,
        variable_zoom: false,
        rounding: view_request.rounding,
        zoom_range: view_request.zoom_range,
        padding: pad,
        max_tiles: 512,
    };
    let elevation = TileElevationRange {
        min_meters: 0.0,
        max_meters: elevation_for_tile_culling(camera, center_elevation)
            + view_request.headroom_meters,
    };
    let far_plane = if view_request.headroom_meters > 0.0 {
        FarPlane::ElevatedMeters(view_request.headroom_meters.round() as u32)
    } else {
        FarPlane::Horizon
    };
    let lod = LodContext::new(camera, view_request.requested_zoom);
    let (tiles, frustum, margins) =
        covering_tiles_with_margins(camera, options, (lod, far_plane), &elevation, None)?;
    Ok(GlobeCoveringAnchor {
        key,
        far_plane,
        frustum,
        clipping_plane: camera.clipping_plane(),
        focus_unit: camera.focus_unit_sphere(),
        margins,
        padding: pad,
        tiles: Arc::from(tiles),
        level: u8::from(view_request.level),
    })
}
