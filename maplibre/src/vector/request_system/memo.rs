use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        view_state::{ViewState, ViewStatePadding},
        Renderer,
    },
    style::Style,
    tcs::world::World,
    vector::VectorBufferPool,
};

#[derive(Clone, PartialEq)]
pub(super) struct RequestViewKey {
    pub(super) view_proj: [[f64; 4]; 4],
    pub(super) viewport: (f64, f64),
    pub(super) pixel_ratio: f32,
    pub(super) external_view: bool,
    pub(super) tight_requests: bool,
    pub(super) style_rev: u64,
    pub(super) request_padding: ViewStatePadding,
    pub(super) visible_level: u8,
    pub(super) terrain_active: bool,
}

impl RequestViewKey {
    pub(super) fn build(
        style: &Style,
        view_state: &ViewState,
        renderer: &Renderer,
        pixel_ratio: f32,
        request_padding: ViewStatePadding,
        visible_level: ZoomLevel,
    ) -> Self {
        Self {
            view_proj: *AsRef::<[[f64; 4]; 4]>::as_ref(&view_state.view_projection().0),
            viewport: (view_state.width(), view_state.height()),
            pixel_ratio,
            external_view: view_state.has_external_view(),
            tight_requests: renderer.settings.tight_tile_requests,
            style_rev: style.revision(),
            request_padding,
            visible_level: u8::from(visible_level),
            terrain_active: style.terrain.is_some(),
        }
    }
}

pub(super) struct RequestMemo {
    pub(super) key: RequestViewKey,
    pub(super) computed: Option<(Vec<WorldTileCoords>, u8)>,
    pub(super) drapes: Vec<WorldTileCoords>,
    pub(super) prefetch: Vec<WorldTileCoords>,
    pub(super) vector_evictions: u64,
    pub(super) settled: bool,
}

pub(super) fn vector_eviction_generation(world: &World) -> u64 {
    match world.resources.get::<Eventually<VectorBufferPool>>() {
        Some(Initialized(pool)) => pool.eviction_generation(),
        _ => 0,
    }
}

pub(super) fn update_loose_covering_cache(
    world: &mut World,
    computed: Option<&(Vec<WorldTileCoords>, u8)>,
    request_padding: ViewStatePadding,
    visible_level: ZoomLevel,
) {
    let host_frame = crate::render::frame_signals::host_frame_in_progress(world);
    let cache = world
        .resources
        .get_or_init_mut::<crate::render::eye_covering::FrameLooseCovering>();
    if cache.host_frame != host_frame {
        *cache = crate::render::eye_covering::FrameLooseCovering {
            host_frame,
            view_tiles: None,
            raster_tiles: None,
        };
    }
    cache.view_tiles = Some((
        computed.map(|(tiles, _)| tiles.clone()),
        request_padding,
        visible_level,
    ));
}

/// Device pixels per layout pixel of the display: as the host reported it, or else as the
/// surface and viewport sizes give it.
pub(super) fn display_pixel_ratio(
    world: &World,
    renderer: &Renderer,
    view_state: &ViewState,
) -> f32 {
    world
        .resources
        .get::<crate::sdf::provided::DisplayPixelRatio>()
        .map_or_else(
            || {
                pixel_ratio(
                    renderer.state().surface().size().width(),
                    view_state.width(),
                )
            },
            |ratio| ratio.0,
        )
}

/// Device pixels per layout pixel read from the surface and viewport, to the hundredth so that
/// a ratio computed from rounded sizes does not ask for images again.
fn pixel_ratio(physical_width: u32, logical_width: f64) -> f32 {
    let ratio = f64::from(physical_width) / logical_width.max(1.0);
    ((ratio * 100.0).round() / 100.0).clamp(0.25, 8.0) as f32
}
