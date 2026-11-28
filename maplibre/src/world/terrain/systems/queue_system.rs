//! Shared terrain queue helpers, drape state caching, and uniform utilities.

pub(crate) mod acquire;
pub(crate) mod activity;
pub(crate) use activity::PendingDrapes;
pub(crate) mod cohort;
pub(crate) mod covering;
pub(crate) mod edges;
pub(crate) mod prefetch;
pub(crate) mod steady_cache;
pub(crate) mod surface_covering;
pub(crate) mod uniforms;
pub(crate) use uniforms::TerrainEyeFrame;

/// Divisor of the tile circumference giving the skirt drop, as in GL JS `getSkirtLength`.
pub(crate) const SKIRT_DIVISOR: f64 = 5.0;
/// Drape textures drawn in one frame. Every level of a drape's mip chain is a render pass,
/// and WebGPU / Metal close a pass per target + mip level; keeping `wasm32` tightly bounded
/// avoids 300–500 ms browser render-pass storms while remaining tiles use ancestors or wait 1–2 frames.
#[cfg(target_arch = "wasm32")]
const MAX_DRAPES_PER_FRAME: usize = 5;
#[cfg(not(target_arch = "wasm32"))]
const MAX_DRAPES_PER_FRAME: usize = 24;
/// Drape textures drawn per frame while a host's eye drives the map. A flight lands hundreds
/// of tiles within a few frames, and a display's frame budget holds only a few drapes.
#[cfg(target_arch = "wasm32")]
const EYE_DRAPES_PER_FRAME: usize = 3;
#[cfg(not(target_arch = "wasm32"))]
const EYE_DRAPES_PER_FRAME: usize = 8;

/// How many drapes a frame draws, the rest waiting for later frames.
#[derive(Clone, Copy, Debug)]
pub struct DrapeBudget {
    /// Drapes per frame of the map's own camera.
    pub per_frame: usize,
    /// Drapes per frame while a host's eye drives the map.
    pub per_eye_frame: usize,
    /// GPU time a frame spends on drapes while the camera moves; the drapes past it keep their
    /// previous or an ancestor's texture until a later frame.
    pub moving_time: std::time::Duration,
}

impl Default for DrapeBudget {
    fn default() -> Self {
        #[cfg(target_arch = "wasm32")]
        let moving_time = std::time::Duration::from_millis(2);
        #[cfg(not(target_arch = "wasm32"))]
        let moving_time = std::time::Duration::from_millis(4);
        Self {
            per_frame: MAX_DRAPES_PER_FRAME,
            per_eye_frame: EYE_DRAPES_PER_FRAME,
            moving_time,
        }
    }
}

/// Quantizes drape zoom invalidation to integer zoom crossings with `0.15`-level hysteresis so
/// fractional `0.125` `paint_zoom` steps or DEM camera-reconciliation nudges do not invalidate
/// every cached `1024²` drape texture simultaneously.
pub(crate) fn next_drape_fingerprint_zoom(previous: Option<f64>, requested: f64) -> f64 {
    previous
        .filter(|&prev| requested >= prev - 0.15 && requested <= prev + 1.15)
        .unwrap_or_else(|| requested.floor())
}

#[cfg(test)]
mod tests {
    use super::acquire::{awaiting_first_draw, budget_redraws};
    use crate::terrain::drape_cache::DrapeState;

    #[test]
    fn a_frame_draws_new_tiles_first_and_at_most_the_budget() {
        use DrapeState::{Changed, New, Unchanged};
        let states = [Changed, New, Unchanged, New, Changed, New];
        assert_eq!(
            budget_redraws(&states, 4),
            [true, true, false, true, false, true],
            "three new tiles and the first changed one"
        );
        assert_eq!(
            budget_redraws(&states, 10),
            [true, true, false, true, true, true],
            "everything but the unchanged tile fits"
        );
        assert_eq!(budget_redraws(&states, 0), [false; 6]);
    }

    #[test]
    fn a_new_tile_the_budget_deferred_is_not_drawn_with_borrowed_content() {
        use DrapeState::{Changed, New, Unchanged};
        let states = [New, Changed, New, Unchanged, New];
        let redraw = budget_redraws(&states, 2);
        assert_eq!(redraw, [true, false, true, false, false]);
        assert_eq!(
            awaiting_first_draw(&states, &redraw),
            [false, false, false, false, true],
            "only the undrawn new tile waits; a changed tile keeps showing its last content"
        );
    }

    #[test]
    fn drape_fingerprint_zoom_ignores_eighth_steps_and_steps_with_integer_hysteresis() {
        use super::next_drape_fingerprint_zoom;
        let mut current = None;
        for requested in [12.0, 12.125, 12.5, 12.875, 13.0, 13.14, 11.86] {
            current = Some(next_drape_fingerprint_zoom(current, requested));
            assert_eq!(
                current,
                Some(12.0),
                "fractional zoom {requested} must not invalidate drapes"
            );
        }
        current = Some(next_drape_fingerprint_zoom(current, 13.16));
        assert_eq!(current, Some(13.0));
        current = Some(next_drape_fingerprint_zoom(current, 12.90));
        assert_eq!(
            current,
            Some(13.0),
            "hysteresis holds across 13.0 boundary jitter"
        );
        current = Some(next_drape_fingerprint_zoom(current, 12.80));
        assert_eq!(current, Some(12.0));
    }
}
