//! Per-frame quantized zoom state for vector paint evaluation and terrain drape caching.

use crate::tcs::world::World;

/// Rounds `zoom` to eighth-zoom steps for drape paint evaluation.
pub(crate) fn drape_paint_zoom(zoom: f64) -> f64 {
    (zoom * 8.0).round() / 8.0
}

#[derive(Default)]
struct PaintZoom(Option<f64>);

pub(crate) fn stabilize_zoom(world: &mut World, requested: f64) -> f64 {
    let state = world.resources.get_or_init_mut::<PaintZoom>();
    let zoom = next_zoom(state.0, requested);
    state.0 = Some(zoom);
    zoom
}

fn next_zoom(previous: Option<f64>, requested: f64) -> f64 {
    previous
        .filter(|previous| (requested - previous).abs() < 0.125)
        .unwrap_or_else(|| drape_paint_zoom(requested))
}

/// Returns the stabilized drape paint zoom for the current frame.
pub(crate) fn current_zoom(world: &World, fallback: f64) -> f64 {
    world
        .resources
        .get::<PaintZoom>()
        .and_then(|state| state.0)
        .unwrap_or_else(|| drape_paint_zoom(fallback))
}

#[cfg(test)]
mod tests {
    use super::next_zoom;

    #[test]
    fn stationary_view_jitter_does_not_alternate_drape_styles() {
        let mut previous = None;
        for requested in [12.059, 12.066, 12.061, 12.071, 12.062] {
            let selected = next_zoom(previous, requested);
            assert_eq!(selected, 12.0);
            previous = Some(selected);
        }
        assert_eq!(next_zoom(previous, 12.3), 12.25);
        assert_eq!(next_zoom(Some(12.25), 12.13), 12.25);
    }
}
