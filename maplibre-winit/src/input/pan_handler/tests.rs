use maplibre::{coords::LatLon, terrain::interaction::globe::TerrainAnchor};
use winit::event::{ElementState, MouseButton};

use super::PanHandler;

#[test]
fn a_press_while_inertia_carries_the_last_drag_grabs_anew() {
    // A drag has ended and its inertia still runs on its plane and anchor.
    let mut handler = PanHandler {
        gesture_plane: Some(1500.0),
        globe_anchor: Some((
            TerrainAnchor {
                location: LatLon::new(27.7, 88.0),
                elevation: 1500.0,
            },
            cgmath::Vector2::new(400.0, 300.0),
        )),
        ..PanHandler::default()
    };
    assert!(handler.process_mouse_key_press(&MouseButton::Left, &ElementState::Pressed));
    assert_eq!(handler.gesture_plane, None);
    assert_eq!(handler.globe_anchor, None);
    let mut touched = PanHandler {
        gesture_plane: Some(0.0),
        ..PanHandler::default()
    };
    assert!(touched.process_touch_start(&cgmath::Vector2::new(10.0, 10.0)));
    assert_eq!(touched.gesture_plane, None);
}
