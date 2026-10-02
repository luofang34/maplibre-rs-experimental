//! A continuous zoom redraws its drapes within each frame's time budget, keeps every pixel
//! covered while drapes wait, and finishes them once the zoom stops.
use std::time::Duration;

use super::*;
use crate::{coords::Zoom, terrain::drape_timing::DrapeCost};

/// Steps of a zoom in, each a frame, from the fixture's z12.125.
const STEPS: usize = 24;
const STEP: f64 = 0.0625;

struct ZoomRun {
    /// Each moving frame's drape redraws and what its budget allowed.
    moving: Vec<(u32, usize)>,
    /// Redraws of the frames after the zoom stopped, until the map settled.
    settling: Vec<u32>,
}

async fn zoom_in(moving_time: Duration) -> ZoomRun {
    let mut map = cached_map().await;
    let budget = crate::terrain::DrapeBudget {
        moving_time,
        ..Default::default()
    };
    map.map_context.world.resources.insert(budget);
    drag_frame(&mut map, "zoom-start");
    let mut moving = Vec::new();
    for step in 0..STEPS {
        // The camera's position is in world pixels of the current zoom, so it scales with it
        // to keep the view centred where it is.
        let view = &mut map.map_context.view_state;
        let zoom = view.zoom().value() + STEP;
        let position = view.camera().position();
        let scale = 2_f64.powf(STEP);
        view.camera_mut()
            .move_to(cgmath::Point2::new(position.x * scale, position.y * scale));
        view.update_zoom(Zoom::new(zoom));
        // The queue system reads the cost the previous frames measured.
        let allowed = map
            .map_context
            .world
            .resources
            .get::<DrapeCost>()
            .copied()
            .unwrap_or_default()
            .drapes_within(moving_time, budget.per_frame);
        let frame = drag_frame(&mut map, &format!("zoom-{step}"));
        moving.push((frame.drape_redraws, allowed));
    }
    let mut settling = Vec::new();
    while map.needs_redraw() && settling.len() < 30 {
        settling
            .push(drag_frame(&mut map, &format!("zoom-settle-{}", settling.len())).drape_redraws);
    }
    assert!(
        !map.needs_redraw(),
        "the map settles after the zoom: {settling:?}"
    );
    settling.push(drag_frame(&mut map, "zoom-settled").drape_redraws);
    ZoomRun { moving, settling }
}

#[tokio::test]
async fn a_zoom_redraws_drapes_within_its_frame_time_and_finishes_them_after() {
    let bounded = zoom_in(Duration::ZERO).await;
    for (step, (redraws, allowed)) in bounded.moving.iter().enumerate() {
        assert!(
            *redraws as usize <= *allowed,
            "zoom step {step} drew {redraws} drapes, past the {allowed} its time holds"
        );
    }
    assert!(
        bounded.moving.iter().any(|(redraws, _)| *redraws > 0),
        "the zoom draws drapes as it goes: {:?}",
        bounded.moving
    );
    assert_eq!(
        bounded.settling.last().copied().unwrap_or(0),
        0,
        "every drape is current once the map settles: {:?}",
        bounded.settling
    );
    // Without the time budget the same zoom redraws more in some frame, so the budget is what
    // held it back.
    let unbounded = zoom_in(Duration::from_secs(1)).await;
    let most = |run: &ZoomRun| {
        run.moving
            .iter()
            .map(|(redraws, _)| *redraws)
            .max()
            .unwrap_or(0)
    };
    assert!(
        most(&unbounded) > most(&bounded),
        "the budget limits the zoom's redraws: {} unbounded, {} bounded",
        most(&unbounded),
        most(&bounded)
    );
}

#[tokio::test]
async fn a_moving_view_reaching_undrawn_ground_still_covers_it() {
    let mut map = cached_map().await;
    map.map_context
        .world
        .resources
        .insert(crate::terrain::DrapeBudget {
            moving_time: Duration::ZERO,
            ..Default::default()
        });
    drag_frame(&mut map, "jump-start");
    // Drapes here have no drawn ancestor, so they are drawn whatever the time budget says.
    let away = jump(&mut map, 4800.0, "jump-undrawn");
    assert!(away.drape_redraws > 1, "{away:?}");
}
