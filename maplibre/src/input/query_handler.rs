//! Pointer-click rendered-geometry query logger.

use std::time::Duration;

use cgmath::Vector2;

use super::{
    events::{ButtonState, PointerButton},
    projection::{globe_world_at_screen, uses_globe},
    UpdateState,
};
use crate::{
    context::MapContext, coords::WorldCoords, io::geometry_index::IndexedGeometry,
    render::tile_view_pattern::DEFAULT_TILE_SIZE,
};

#[derive(Default)]
pub(crate) struct QueryHandler {
    window_position: Option<Vector2<f64>>,
    clicking: bool,
}

impl QueryHandler {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn process_touch_start(&mut self) -> bool {
        self.clicking = true;
        true
    }

    pub(crate) fn process_touch_end(&mut self) -> bool {
        self.clicking = false;
        true
    }

    pub(crate) fn process_window_position(
        &mut self,
        window_position: &Vector2<f64>,
        _touch: bool,
    ) -> bool {
        self.window_position = Some(*window_position);
        true
    }

    pub(crate) fn process_pointer_button(
        &mut self,
        button: PointerButton,
        state: ButtonState,
    ) -> bool {
        if button != PointerButton::Primary {
            return false;
        }
        self.clicking = state == ButtonState::Pressed;
        true
    }
}

impl UpdateState for QueryHandler {
    fn update_state(
        &mut self,
        MapContext {
            style,
            view_state,
            world,
            ..
        }: &mut MapContext,
        _dt: Duration,
    ) {
        self.update_parts(style, view_state, world);
    }
}

impl QueryHandler {
    pub(crate) fn update_parts(
        &mut self,
        style: &crate::style::Style,
        view_state: &mut crate::render::view_state::ViewState,
        world: &crate::tcs::world::World,
    ) {
        if self.clicking {
            if let Some(window_position) = self.window_position {
                let Ok(inverted_view_proj) = view_state.inverted_view_projection() else {
                    return;
                };

                let z = view_state.zoom().zoom_level(DEFAULT_TILE_SIZE);
                let zoom = view_state.zoom();

                let coordinates = if uses_globe(style, view_state) {
                    globe_world_at_screen(style, view_state, window_position)
                        .map(|coordinates| Vector2::new(coordinates.x, coordinates.y))
                } else {
                    view_state.window_to_world_at_ground(
                        &window_position,
                        &inverted_view_proj,
                        false,
                    )
                };
                if let Some(coordinates) = coordinates {
                    if let Some(geometries) = world
                        .tiles
                        .geometry_index
                        .query_point(
                            &WorldCoords {
                                x: coordinates.x,
                                y: coordinates.y,
                            },
                            z,
                            zoom,
                        )
                        .map(|geometries| {
                            geometries
                                .iter()
                                .cloned()
                                .cloned()
                                .collect::<Vec<IndexedGeometry<f64>>>()
                        })
                    {
                        tracing::info!(
                            "Clicked on geometry: {:?}",
                            geometries
                                .iter()
                                .map(|geometry| &geometry.properties)
                                .collect::<Vec<_>>()
                        );
                    } else {
                        tracing::info!("No geometry found.");
                    }
                }
            }
            self.clicking = false;
        }
    }
}
