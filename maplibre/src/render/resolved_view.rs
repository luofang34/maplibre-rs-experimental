//! Immutable per-frame resolved view snapshot shared across render stages and feature queries.

#![deny(missing_docs)]

use cgmath::{InnerSpace, Matrix4, Vector3, Vector4};

use crate::{
    context::MapContext,
    coords::{Zoom, TILE_SIZE},
    projection::{
        body::Body,
        globe::{
            angular_radians_to_unit_sphere, camera::GlobeCameraState, elevate_surface_point,
            mercator_to_angular_radians,
        },
    },
    render::{
        camera::{InvertedViewProjection, ViewProjection, OPENGL_TO_WGPU_MATRIX},
        projection::{
            flat_shader_projection, globe_camera_for_view, globe_shader_projection,
            ProjectionStateError, ShaderProjectionData,
        },
        view_state::{CameraAuthority, ViewState},
    },
    style::Style,
    tcs::{system::SystemResult, world::World},
};

/// Transition threshold at which globe horizon clipping begins (`GLOBE_Z_CLIPPING_START` in WGSL).
const GLOBE_Z_CLIPPING_START: f64 = 0.2;

/// Exact bitwise key identifying all style and [`ViewState`] inputs that determine a
/// [`ResolvedFrameView`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ViewSnapshotKey {
    width_bits: u64,
    height_bits: u64,
    insets_bits: [u64; 4],
    fovy_bits: u64,
    zoom_bits: u64,
    style_zoom_bits: u64,
    cam_pos_bits: [u64; 2],
    cam_angles_bits: [u64; 3],
    center_elev_bits: u64,
    min_elev_bits: u64,
    body_radius_bits: u64,
    globe_orbits_center: bool,
    terrain_enabled: bool,
    transition_bits: u32,
    authority: CameraAuthority,
    pose_bits: Option<[u64; 9]>,
    external_proj_bits: Option<[u64; 16]>,
    external_cam_bits: Option<[u64; 16]>,
}

fn matrix_bits(m: Matrix4<f64>) -> [u64; 16] {
    [
        m.x.x.to_bits(),
        m.x.y.to_bits(),
        m.x.z.to_bits(),
        m.x.w.to_bits(),
        m.y.x.to_bits(),
        m.y.y.to_bits(),
        m.y.z.to_bits(),
        m.y.w.to_bits(),
        m.z.x.to_bits(),
        m.z.y.to_bits(),
        m.z.z.to_bits(),
        m.z.w.to_bits(),
        m.w.x.to_bits(),
        m.w.y.to_bits(),
        m.w.z.to_bits(),
        m.w.w.to_bits(),
    ]
}

impl ViewSnapshotKey {
    fn capture(style: &Style, view_state: &ViewState, transition: f32) -> Self {
        let insets = view_state.edge_insets();
        let pos = view_state.camera().position();
        let pose_bits = view_state.globe_pose().map(|pose| {
            [
                pose.target[0].to_bits(),
                pose.target[1].to_bits(),
                pose.target[2].to_bits(),
                pose.target_elevation_meters.to_bits(),
                pose.orientation[0].to_bits(),
                pose.orientation[1].to_bits(),
                pose.orientation[2].to_bits(),
                pose.orientation[3].to_bits(),
                pose.distance_meters.to_bits(),
            ]
        });
        Self {
            width_bits: view_state.width().to_bits(),
            height_bits: view_state.height().to_bits(),
            insets_bits: [
                insets.top.to_bits(),
                insets.bottom.to_bits(),
                insets.left.to_bits(),
                insets.right.to_bits(),
            ],
            fovy_bits: view_state.field_of_view().0.to_bits(),
            zoom_bits: view_state.zoom().value().to_bits(),
            style_zoom_bits: view_state.style_zoom().value().to_bits(),
            cam_pos_bits: [pos.x.to_bits(), pos.y.to_bits()],
            cam_angles_bits: [
                view_state.camera().get_bearing().0.to_bits(),
                view_state.camera().get_pitch().0.to_bits(),
                view_state.camera().get_roll().0.to_bits(),
            ],
            center_elev_bits: view_state.center_elevation().to_bits(),
            min_elev_bits: view_state.min_elevation().to_bits(),
            body_radius_bits: view_state.body().radius_meters.to_bits(),
            globe_orbits_center: view_state.globe_orbits_center(),
            terrain_enabled: style.terrain.is_some(),
            transition_bits: transition.to_bits(),
            authority: view_state.camera_authority(),
            pose_bits,
            external_proj_bits: view_state.external_projection().map(matrix_bits),
            external_cam_bits: view_state.external_camera_matrix().map(matrix_bits),
        }
    }
}

/// Active geometric projection mode of a [`ResolvedFrameView`].
#[derive(Clone, Debug)]
pub enum ProjectionMode {
    /// Planar Web Mercator projection (`transition == 0.0`).
    FlatMercator,
    /// Vertical-perspective globe or Mercator-to-globe transition (`transition > 0.0`).
    Globe {
        /// Interpolation weight in `(0.0, 1.0]` (`1.0` for pure globe).
        transition: f32,
        /// Pre-built vertical-perspective globe camera state.
        camera: Box<GlobeCameraState>,
    },
}

/// Window-space coordinates and clip depth of a projected 3D world point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectedWindowPoint {
    /// Viewport pixel coordinates `[x, y]` with `y` pointing downward.
    pub window: [f64; 2],
    /// Unreversed normalized clip depth (`clip.z / clip.w`) for front-to-back ordering.
    pub depth: f64,
    /// Homogeneous perspective distance `clip.w`.
    pub clip_w: f64,
}

/// Immutable per-frame camera and projection snapshot derived once from [`Style`] and [`ViewState`].
#[derive(Clone, Debug)]
pub struct ResolvedFrameView {
    key: ViewSnapshotKey,
    /// Authoritative camera representation driving this frame.
    pub authority: CameraAuthority,
    /// Active geometric projection mode (flat Mercator or globe / transition).
    pub projection: ProjectionMode,
    /// Precomputed GPU uniform data for projection-aware shaders.
    pub shader_projection: ShaderProjectionData,
    /// Unreversed clip-space view-projection matrix (`[0, w]` depth) for CPU math.
    pub view_projection: ViewProjection,
    /// Reversed-Z GPU clip-space view-projection matrix.
    pub gpu_view_projection: ViewProjection,
    /// Pre-inverted Mercator view-projection matrix when invertible.
    pub inverted_view_projection: Option<InvertedViewProjection>,
    /// Transform mapping normalized device coordinates to window pixels.
    pub clip_to_window: Matrix4<f64>,
    /// Continuous map zoom level.
    pub zoom: Zoom,
    /// Style evaluation zoom level.
    pub style_zoom: Zoom,
    /// Web Mercator world width in pixels at `zoom`.
    pub world_size: f64,
    /// Mercator pixels per metre at the map center.
    pub pixels_per_meter: f64,
    /// Orbit distance in pixels from eye to map center.
    pub camera_to_center_distance: f64,
    /// Elevation in metres of the orbit target at the map center.
    pub center_elevation: f64,
    /// Eye position in Mercator world pixels (`x, y`) and metres above sea level (`z`).
    pub eye_position: Vector3<f64>,
    /// Viewport logical dimensions `(width, height)` in pixels.
    pub viewport_size: (f64, f64),
    /// Clockwise map bearing in radians.
    pub bearing_radians: f64,
    /// Camera pitch in radians.
    pub pitch_radians: f64,
    /// Planetary body geometry.
    pub body: Body,
    /// Whether 3D terrain is declared on the active style.
    pub terrain_enabled: bool,
}

impl ResolvedFrameView {
    /// Resolves all per-frame camera matrices, globe state, and shader projection data in one pass.
    pub fn resolve(style: &Style, view_state: &ViewState) -> Result<Self, ProjectionStateError> {
        let zoom = view_state.zoom();
        let transition = style.projection.as_ref().map_or(0.0, |spec| {
            spec.projection_type.globe_transition(zoom.value())
        });
        let key = ViewSnapshotKey::capture(style, view_state, transition);
        let (projection, shader_projection) = if transition == 0.0 {
            (
                ProjectionMode::FlatMercator,
                flat_shader_projection(view_state),
            )
        } else {
            let globe = globe_camera_for_view(view_state)?;
            let shader = globe_shader_projection(view_state, transition, &globe)?;
            (
                ProjectionMode::Globe {
                    transition,
                    camera: Box::new(globe),
                },
                shader,
            )
        };

        let view_projection = view_state.view_projection();
        let gpu_view_projection = view_state.gpu_view_projection();
        let inverted_view_projection = view_state.inverted_view_projection().ok();

        Ok(Self {
            key,
            authority: view_state.camera_authority(),
            projection,
            shader_projection,
            view_projection,
            gpu_view_projection,
            inverted_view_projection,
            clip_to_window: view_state.clip_to_window_transform(),
            zoom,
            style_zoom: view_state.style_zoom(),
            world_size: TILE_SIZE * 2.0_f64.powf(zoom.value()),
            pixels_per_meter: view_state.pixels_per_meter(),
            camera_to_center_distance: view_state.camera_to_center_distance(),
            center_elevation: view_state.center_elevation(),
            eye_position: view_state.eye_position(),
            viewport_size: view_state.viewport_size(),
            bearing_radians: view_state.camera().get_bearing().0,
            pitch_radians: view_state.camera().get_pitch().0,
            body: view_state.body(),
            terrain_enabled: style.terrain.is_some(),
        })
    }

    /// Returns `true` when this snapshot already matches every relevant input of `style` and
    /// `view_state`.
    #[inline]
    pub fn matches(&self, style: &Style, view_state: &ViewState) -> bool {
        let transition = style.projection.as_ref().map_or(0.0, |spec| {
            spec.projection_type
                .globe_transition(view_state.zoom().value())
        });
        self.key == ViewSnapshotKey::capture(style, view_state, transition)
    }

    /// Borrows the current snapshot in `world.resources` when up to date, or resolves a fresh one.
    pub fn get_or_resolve<'a>(
        world: &'a World,
        style: &Style,
        view_state: &ViewState,
    ) -> Result<std::borrow::Cow<'a, Self>, ProjectionStateError> {
        if let Some(cached) = world.resources.get::<Self>() {
            if cached.matches(style, view_state) {
                return Ok(std::borrow::Cow::Borrowed(cached));
            }
        }
        Self::resolve(style, view_state).map(std::borrow::Cow::Owned)
    }

    /// Returns the active Mercator-to-globe transition weight in `0.0..=1.0`.
    #[inline]
    pub fn globe_transition(&self) -> f32 {
        match &self.projection {
            ProjectionMode::FlatMercator => 0.0,
            ProjectionMode::Globe { transition, .. } => *transition,
        }
    }

    /// Borrows the precomputed [`GlobeCameraState`] when globe rendering is active.
    #[inline]
    pub fn globe_camera(&self) -> Option<&GlobeCameraState> {
        match &self.projection {
            ProjectionMode::FlatMercator => None,
            ProjectionMode::Globe { camera, .. } => Some(camera),
        }
    }

    /// Clip-space `w` of the view center, blended across Mercator-to-globe transitions.
    #[inline]
    pub fn center_clip_w(&self) -> f64 {
        f64::from(self.shader_projection.center_clip_w)
    }

    /// Tile-unit scale factor for map-aligned widths/radii at normalized Mercator `y` (`0..=1`),
    /// matching `project_symbol_scale` in `projection.vertex.wgsl`.
    #[inline]
    pub fn globe_scale_at_mercator_y(&self, mercator_y: f64) -> f64 {
        let transition = self.globe_transition();
        if transition <= 0.0 {
            return 1.0;
        }
        let (_, lat) = mercator_to_angular_radians(0.0, mercator_y.clamp(0.0, 1.0));
        let cos_lat = lat.cos().max(1e-3);
        let globe_circle_x = f64::from(self.shader_projection.globe_circle[0]);
        1.0 + (globe_circle_x / cos_lat - 1.0) * f64::from(transition)
    }

    /// Signed horizon distance of normalized Mercator `(x, y)` matching `globe_horizon_distance`
    /// in `projection.vertex.wgsl` (`>= 0.0` on the visible hemisphere, `< 0.0` occluded).
    #[inline]
    pub fn horizon_distance_at_mercator(&self, mercator_x: f64, mercator_y: f64) -> f64 {
        let ProjectionMode::Globe { transition, camera } = &self.projection else {
            return 1.0;
        };
        let (lon, lat) = mercator_to_angular_radians(mercator_x, mercator_y.clamp(0.0, 1.0));
        let surface = angular_radians_to_unit_sphere(lon, lat);
        let clipping_t = ((f64::from(*transition) - GLOBE_Z_CLIPPING_START)
            / (1.0 - GLOBE_Z_CLIPPING_START))
            .clamp(0.0, 1.0);
        let plane = camera.clipping_plane();
        let dot = plane.truncate().dot(surface) + plane.w;
        (1.0 - clipping_t) + clipping_t * dot
    }

    /// Projects a world-pixel position `[x, y]` elevated by `elevation_meters` above sea level
    /// into window coordinates, returning `None` behind the camera or occluded behind the globe
    /// horizon.
    pub fn project_world_point(
        &self,
        [x, y]: [f64; 2],
        elevation_meters: f64,
    ) -> Option<ProjectedWindowPoint> {
        let clip = match &self.projection {
            ProjectionMode::FlatMercator => {
                self.view_projection
                    .project(Vector4::new(x, y, elevation_meters, 1.0))
            }
            ProjectionMode::Globe { transition, camera } => {
                let mx = x / self.world_size;
                let my = (y / self.world_size).clamp(0.0, 1.0);
                let (lon, lat) = mercator_to_angular_radians(mx, my);
                let surface = angular_radians_to_unit_sphere(lon, lat);
                let clipping_t = ((f64::from(*transition) - GLOBE_Z_CLIPPING_START)
                    / (1.0 - GLOBE_Z_CLIPPING_START))
                    .clamp(0.0, 1.0);
                let plane = camera.clipping_plane();
                let horizon_dist =
                    (1.0 - clipping_t) + clipping_t * (plane.truncate().dot(surface) + plane.w);
                if horizon_dist < 0.0 {
                    return None;
                }
                let elevated = elevate_surface_point(surface, elevation_meters, self.body);
                let globe_clip =
                    OPENGL_TO_WGPU_MATRIX * camera.view_projection() * elevated.extend(1.0);
                if *transition >= 1.0 {
                    globe_clip
                } else {
                    let flat_clip =
                        self.view_projection
                            .project(Vector4::new(x, y, elevation_meters, 1.0));
                    let t = f64::from(*transition);
                    flat_clip * (1.0 - t) + globe_clip * t
                }
            }
        };

        if !clip.w.is_finite()
            || clip.w <= 0.0
            || !clip.x.is_finite()
            || !clip.y.is_finite()
            || !clip.z.is_finite()
        {
            return None;
        }
        let ndc = Vector4::new(clip.x / clip.w, clip.y / clip.w, clip.z / clip.w, 1.0);
        let window = self.clip_to_window * ndc;
        Some(ProjectedWindowPoint {
            window: [window.x, window.y],
            depth: ndc.z,
            clip_w: clip.w,
        })
    }
}

/// Publishes or refreshes [`ResolvedFrameView`] in [`World::resources`] when camera or projection
/// inputs have changed.
pub fn resolve_frame_view_system(
    MapContext {
        world,
        style,
        view_state,
        ..
    }: &mut MapContext,
) -> SystemResult {
    if world
        .resources
        .get::<ResolvedFrameView>()
        .is_some_and(|cached| cached.matches(style, view_state))
    {
        return Ok(());
    }
    match ResolvedFrameView::resolve(style, view_state) {
        Ok(resolved) => {
            world.resources.insert(resolved);
        }
        Err(_) => {
            world.resources.remove::<ResolvedFrameView>();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use cgmath::{Deg, Rad};

    use super::*;
    use crate::{
        coords::{LatLon, WorldCoords},
        projection::{ProjectionSpecification, ProjectionType},
        window::PhysicalSize,
    };

    fn base_view(zoom: f64) -> ViewState {
        let z = Zoom::new(zoom);
        let center = WorldCoords::from_lat_lon(LatLon::new(0.0, 0.0), z);
        ViewState::new(
            PhysicalSize::new(512, 512).expect("size"),
            center,
            z,
            Deg(0.0),
            Rad(0.6435011087932844),
        )
    }

    #[test]
    fn snapshot_matches_until_camera_or_projection_changes() {
        let mut style = Style::default();
        let mut view = base_view(4.0);
        let resolved = ResolvedFrameView::resolve(&style, &view).expect("resolve flat");
        assert_eq!(resolved.authority, CameraAuthority::ConstrainedMap);
        assert!(matches!(resolved.projection, ProjectionMode::FlatMercator));
        assert!(resolved.matches(&style, &view));

        view.set_center_elevation(250.0);
        assert!(!resolved.matches(&style, &view));

        let resolved_elev = ResolvedFrameView::resolve(&style, &view).expect("resolve elevated");
        assert!(resolved_elev.matches(&style, &view));

        style.projection = Some(ProjectionSpecification {
            projection_type: ProjectionType::Globe,
        });
        assert!(!resolved_elev.matches(&style, &view));
        let globe = ResolvedFrameView::resolve(&style, &view).expect("resolve globe");
        assert_eq!(globe.globe_transition(), 1.0);
        assert!(globe.globe_camera().is_some());
    }

    #[test]
    fn globe_projection_projects_front_center_and_occludes_antipode() {
        let style = Style {
            projection: Some(ProjectionSpecification {
                projection_type: ProjectionType::Globe,
            }),
            ..Style::default()
        };
        let view = base_view(2.0);
        let resolved = ResolvedFrameView::resolve(&style, &view).expect("resolve globe");
        let front = WorldCoords::from_lat_lon(LatLon::new(0.0, 0.0), view.zoom());
        let projected = resolved
            .project_world_point([front.x, front.y], 0.0)
            .expect("front of globe visible");
        assert!((projected.window[0] - 256.0).abs() < 1e-6);
        assert!((projected.window[1] - 256.0).abs() < 1e-6);

        let antipode = WorldCoords::from_lat_lon(LatLon::new(0.0, 180.0), view.zoom());
        assert!(
            resolved
                .project_world_point([antipode.x, antipode.y], 0.0)
                .is_none(),
            "antipode behind globe horizon must be occluded"
        );
    }
}
