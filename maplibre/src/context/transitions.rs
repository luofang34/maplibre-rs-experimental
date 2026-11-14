//! Per-frame advancement of active style paint transitions.

use std::time::Duration;

use crate::{
    context::MapContext,
    io::tile_retry::{self, RequestKind},
    render::frame_signals,
    tcs::system::SystemResult,
    vector::upload_system::style_refresh::DirtyVectorStyle,
};

#[derive(Default)]
struct TransitionClock {
    last_frame_time: Option<Duration>,
}

/// Advances any in-flight [`crate::style::Style`] paint transitions by the elapsed frame time
/// and keeps the frame loop animating until every active transition has settled.
pub(crate) fn style_transition_system(context: &mut MapContext) -> SystemResult {
    let now = context.frame_time();
    if !context.style.has_active_transitions() {
        context
            .world
            .resources
            .get_or_init_mut::<TransitionClock>()
            .last_frame_time = Some(now);
        return Ok(());
    }

    let previous = {
        let clock = context.world.resources.get_or_init_mut::<TransitionClock>();
        clock.last_frame_time.replace(now)
    };
    let delta_ms = previous
        .map(|prev| now.saturating_sub(prev).as_secs_f64() * 1000.0)
        .unwrap_or(0.0);

    if delta_ms > 0.0 {
        let change = context.style.advance_transitions(delta_ms);
        if !change.layers.is_empty() || !change.removed_layers.is_empty() {
            DirtyVectorStyle::mark_style_edit(&mut context.world, change.layers.iter().cloned());
        }
        if change.redraw_tiles {
            tile_retry::refresh(&mut context.world, RequestKind::Vector);
        }
    }

    if context.style.has_active_transitions() {
        frame_signals::keep_animating(&mut context.world);
    } else {
        frame_signals::mark_dirty(&mut context.world);
    }
    Ok(())
}
