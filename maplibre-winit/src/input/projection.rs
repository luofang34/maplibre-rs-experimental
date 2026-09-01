//! Projection-aware coordinate conversion for input handlers.

use cgmath::{Point2, Vector2};
use maplibre::{
    coords::{WorldCoords, Zoom},
    projection::globe::{
        camera::GlobeCameraState,
        interaction::{pan_center_to_anchor, GlobePanUpdate},
    },
    render::{projection::globe_camera_for_view, view_state::ViewState},
    style::Style,
};

pub fn active_globe_camera(style: &Style, view_state: &ViewState) -> Option<GlobeCameraState> {
    uses_globe(style, view_state)
        .then(|| globe_camera_for_view(view_state).ok())
        .flatten()
}

pub fn uses_globe(style: &Style, view_state: &ViewState) -> bool {
    style.projection.as_ref().is_some_and(|projection| {
        projection
            .projection_type
            .globe_transition(view_state.zoom().value())
            > 0.0
    })
}

pub fn globe_world_at_screen(
    style: &Style,
    view_state: &ViewState,
    screen: Vector2<f64>,
) -> Option<WorldCoords> {
    let camera = active_globe_camera(style, view_state)?;
    let pixel = Point2::new(screen.x, screen.y);
    camera
        .is_point_on_map_surface(pixel)
        .then(|| camera.screen_point_to_location(pixel))
        .flatten()
        .map(|location| WorldCoords::from_lat_lon(location, view_state.zoom()))
}

pub fn pan_globe_between_pixels(
    style: &Style,
    view_state: &mut ViewState,
    previous: Vector2<f64>,
    current: Vector2<f64>,
) -> bool {
    let Some(camera) = active_globe_camera(style, view_state) else {
        return false;
    };
    let Some(anchor) = camera.screen_point_to_location(Point2::new(previous.x, previous.y)) else {
        return false;
    };
    let Some(cursor_location) = camera.screen_point_to_location(Point2::new(current.x, current.y))
    else {
        return false;
    };
    apply_pan_update(
        view_state,
        pan_center_to_anchor(
            camera.center(),
            camera.bearing_degrees(),
            anchor,
            cursor_location,
        ),
    );
    true
}

pub fn zoom_globe_around_pixel(
    style: &Style,
    view_state: &mut ViewState,
    screen: Vector2<f64>,
    next_zoom: Zoom,
) -> bool {
    let Some(before) = active_globe_camera(style, view_state) else {
        return false;
    };
    let Some(anchor) = before.screen_point_to_location(Point2::new(screen.x, screen.y)) else {
        return false;
    };
    view_state.update_zoom(next_zoom);
    let Some(after) = active_globe_camera(style, view_state) else {
        return true;
    };
    let Some(cursor_location) = after.screen_point_to_location(Point2::new(screen.x, screen.y))
    else {
        return true;
    };
    apply_pan_update(
        view_state,
        pan_center_to_anchor(
            after.center(),
            after.bearing_degrees(),
            anchor,
            cursor_location,
        ),
    );
    true
}

fn apply_pan_update(view_state: &mut ViewState, update: GlobePanUpdate) {
    let zoom = view_state.zoom() + Zoom::new(update.zoom_adjustment);
    view_state.update_zoom(zoom);
    let center = WorldCoords::from_lat_lon(update.center, zoom);
    view_state
        .camera_mut()
        .move_to(Point2::new(center.x, center.y));
}
