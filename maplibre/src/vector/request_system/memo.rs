use std::sync::Arc;

use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    io::tile_sources::{source_max_zoom, TileKind},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::{
            compute_globe_anchor, eligible_globe_anchor_key, view_region_for_projection,
            GlobeCoveringAnchor, ProjectionStateError,
        },
        resolved_view::ResolvedFrameView,
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
        world: &World,
        style: &Style,
        view_state: &ViewState,
        renderer: &Renderer,
        pixel_ratio: f32,
        request_padding: ViewStatePadding,
        visible_level: ZoomLevel,
    ) -> Option<Self> {
        let width = view_state.width();
        let height = view_state.height();
        let fovy = view_state.field_of_view().0;
        if !view_state.has_external_view()
            && (!fovy.is_finite()
                || fovy <= 0.0
                || fovy >= std::f64::consts::PI
                || !width.is_finite()
                || width <= 0.0
                || !height.is_finite()
                || height <= 0.0)
        {
            return None;
        }
        let view_proj = world
            .resources
            .get::<ResolvedFrameView>()
            .filter(|resolved| resolved.matches(style, view_state))
            .map(|resolved| resolved.view_projection)
            .unwrap_or_else(|| view_state.view_projection());
        Some(Self {
            view_proj: *AsRef::<[[f64; 4]; 4]>::as_ref(&view_proj.0),
            viewport: (width, height),
            pixel_ratio,
            external_view: view_state.has_external_view(),
            tight_requests: renderer.settings.tight_tile_requests,
            style_rev: style.revision(),
            request_padding,
            visible_level: u8::from(visible_level),
            terrain_active: style.terrain.is_some(),
        })
    }

    #[inline]
    pub(super) fn matches_non_matrix(&self, other: &Self) -> bool {
        self.viewport == other.viewport
            && self.pixel_ratio == other.pixel_ratio
            && self.external_view == other.external_view
            && self.tight_requests == other.tight_requests
            && self.style_rev == other.style_rev
            && self.request_padding == other.request_padding
            && self.visible_level == other.visible_level
            && self.terrain_active == other.terrain_active
    }
}

pub(super) struct RequestMemo {
    pub(super) key: RequestViewKey,
    pub(super) computed: Option<(Arc<[WorldTileCoords]>, u8)>,
    pub(super) globe_anchor: Option<Box<GlobeCoveringAnchor>>,
    pub(super) drapes: Vec<WorldTileCoords>,
    pub(super) prefetch: Vec<WorldTileCoords>,
    pub(super) vector_evictions: u64,
    pub(super) settled: bool,
}

pub(super) enum ResolvedViewTiles {
    SettledReuse,
    Computed {
        computed: Option<(Arc<[WorldTileCoords]>, u8)>,
        globe_anchor: Option<Box<GlobeCoveringAnchor>>,
    },
}

#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_view_tiles(
    memo: Option<&RequestMemo>,
    key: Option<&RequestViewKey>,
    settled_unchanged: bool,
    style: &Style,
    view_state: &ViewState,
    world: &World,
    visible_level: ZoomLevel,
    request_padding: ViewStatePadding,
    tight_tile_requests: bool,
) -> Result<ResolvedViewTiles, ProjectionStateError> {
    if let Some(memo) = memo.filter(|m| key == Some(&m.key)) {
        return Ok(ResolvedViewTiles::Computed {
            computed: memo.computed.clone(),
            globe_anchor: memo.globe_anchor.clone(),
        });
    }
    if let Some((anchor_key, view_request, camera)) =
        eligible_globe_anchor_key(style, view_state, world, visible_level, request_padding)
    {
        if let Some(anchor) = memo.and_then(|memo| memo.globe_anchor.as_deref()) {
            if let Some((cached_tiles, exact_order)) = anchor.try_reuse(&anchor_key, &camera) {
                let source_z = source_max_zoom(style, TileKind::Vector)
                    .map_or(u8::from(visible_level), |m| u8::from(visible_level).min(m));
                let fallback_independent = tight_tile_requests
                    || (cfg!(target_arch = "wasm32")
                        && !view_state.has_external_view()
                        && source_z < 3);
                if exact_order && fallback_independent && settled_unchanged {
                    return Ok(ResolvedViewTiles::SettledReuse);
                }
                let final_tiles = if exact_order {
                    cached_tiles
                } else {
                    anchor.sorted_for_camera(&camera)
                };
                return Ok(ResolvedViewTiles::Computed {
                    computed: Some((final_tiles, anchor.level)),
                    globe_anchor: Some(Box::new(anchor.clone())),
                });
            }
        }
        let built = compute_globe_anchor(
            anchor_key,
            view_request,
            request_padding,
            &camera,
            view_state.center_elevation(),
        )
        .map_err(|source| ProjectionStateError::GlobeCovering { source })?;
        let computed = Some((Arc::clone(&built.tiles), built.level));
        return Ok(ResolvedViewTiles::Computed {
            computed,
            globe_anchor: Some(Box::new(built)),
        });
    }
    let computed =
        view_region_for_projection(style, view_state, world, visible_level, request_padding)?.map(
            |region| {
                (
                    Arc::from(region.iter().collect::<Vec<_>>()),
                    u8::from(visible_level),
                )
            },
        );
    Ok(ResolvedViewTiles::Computed {
        computed,
        globe_anchor: None,
    })
}

pub(super) fn vector_eviction_generation(world: &World) -> u64 {
    match world.resources.get::<Eventually<VectorBufferPool>>() {
        Some(Initialized(pool)) => pool.eviction_generation(),
        _ => 0,
    }
}

pub(super) fn update_loose_covering_cache(
    world: &mut World,
    computed: Option<&(Arc<[WorldTileCoords]>, u8)>,
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
            raster_regions: None,
        };
    }
    cache.view_tiles = Some((
        computed.map(|(tiles, _)| Arc::clone(tiles)),
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
