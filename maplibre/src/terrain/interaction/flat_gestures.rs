//! Pointer-anchored movement on the Mercator ground plane.

use super::*;

/// Moves the center so the world position `target` (pixels at the current zoom) at `elevation`
/// metres sits under `pixel`, as GL JS `setLocationAtPoint` does on the flat map.
pub fn set_location_at_pixel(
    view_state: &mut ViewState,
    target: Vector2<f64>,
    elevation: f64,
    pixel: Point2<f64>,
) {
    let Ok(inverted) = view_state.inverted_view_projection() else {
        return;
    };
    let Some(under_pixel) =
        view_state.window_to_world_at_elevation(&pixel.to_vec(), &inverted, elevation)
    else {
        return;
    };
    view_state.camera_mut().move_relative(target - under_pixel);
}

/// Zooms the flat map around a gesture anchor, keeping its ground point under its pixel.
pub fn zoom_mercator_around(view_state: &mut ViewState, anchor: GestureAnchor, next_zoom: Zoom) {
    let plane = anchor
        .elevation
        .unwrap_or_else(|| view_state.center_elevation());
    let Ok(inverted) = view_state.inverted_view_projection() else {
        return;
    };
    let scale = view_state.zoom().scale_delta(&next_zoom);
    let before = view_state.window_to_world_at_elevation(&anchor.pixel.to_vec(), &inverted, plane);
    view_state.zoom_to(next_zoom);
    if let Some(before) = before {
        set_location_at_pixel(view_state, before * scale, plane, anchor.pixel);
    }
}

/// Pans the flat map so the point on the plane `elevation` metres up that was under
/// `cursor - delta` moves under `cursor`.
pub fn pan_mercator_by_pixels(
    view_state: &mut ViewState,
    cursor: Point2<f64>,
    delta: Vector2<f64>,
    elevation: f64,
) {
    let Ok(inverted) = view_state.inverted_view_projection() else {
        return;
    };
    let previous = cursor.to_vec() - delta;
    let (Some(previous), Some(current)) = (
        view_state.window_to_world_at_elevation(&previous, &inverted, elevation),
        view_state.window_to_world_at_elevation(&cursor.to_vec(), &inverted, elevation),
    ) else {
        return;
    };
    view_state.camera_mut().move_relative(previous - current);
}
