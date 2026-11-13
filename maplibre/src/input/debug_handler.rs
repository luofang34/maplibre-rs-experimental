//! Viewport edge-inset debug keyboard handler.

use std::time::Duration;

use super::{
    events::{ButtonState, NavKey},
    UpdateState,
};
use crate::context::MapContext;

#[derive(Default)]
pub(crate) struct DebugHandler {
    top_delta: f64,
    bottom_delta: f64,
    left_delta: f64,
    right_delta: f64,
}

impl UpdateState for DebugHandler {
    fn update_state(&mut self, MapContext { view_state, .. }: &mut MapContext, dt: Duration) {
        self.update_parts(view_state, dt);
    }
}

impl DebugHandler {
    pub(crate) fn update_parts(
        &mut self,
        view_state: &mut crate::render::view_state::ViewState,
        dt: Duration,
    ) {
        let dt = (dt.as_secs_f64() * 10.0).min(1.0);

        let top_delta = self.top_delta * dt;
        let bottom_delta = self.bottom_delta * dt;
        let left_delta = self.left_delta * dt;
        let right_delta = self.right_delta * dt;

        let mut edge_insets = *view_state.edge_insets();
        edge_insets.top += top_delta;
        edge_insets.bottom += bottom_delta;
        edge_insets.left += left_delta;
        edge_insets.right += right_delta;
        view_state.set_edge_insets(edge_insets);
        self.top_delta -= top_delta;
        self.bottom_delta -= bottom_delta;
        self.right_delta -= right_delta;
        self.left_delta -= left_delta;
    }

    pub(crate) fn process_key(&mut self, key: NavKey, state: ButtonState) -> bool {
        let amount = if state == ButtonState::Pressed {
            100.0
        } else {
            0.0
        };

        match key {
            NavKey::DebugInsetLeft => {
                self.left_delta += amount;
                true
            }
            NavKey::DebugInsetTop => {
                self.top_delta += amount;
                true
            }
            NavKey::DebugInsetBottom => {
                self.bottom_delta += amount;
                true
            }
            NavKey::DebugInsetRight => {
                self.right_delta += amount;
                true
            }
            _ => false,
        }
    }
}
