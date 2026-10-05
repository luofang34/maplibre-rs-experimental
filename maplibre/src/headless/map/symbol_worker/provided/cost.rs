//! What provided shields cost a frame, against the same shields from a sprite. A measurement:
//! run with `--ignored --nocapture` for the figures.

use std::time::{Duration, Instant};

use super::*;

/// Draws one frame 16 ms after the last and returns how long the frame itself took.
async fn timed_frame(map: &mut SymbolMap) -> Duration {
    map.map.frame_input_mut().timestamp += Duration::from_millis(16);
    let start = Instant::now();
    map.map.run_frame().expect("frame");
    let spent = start.elapsed();
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    spent
}

fn percentile(times: &mut [Duration], fraction: f64) -> Duration {
    times.sort();
    times[((times.len() - 1) as f64 * fraction).round() as usize]
}

#[tokio::test]
#[ignore = "a measurement; run with --ignored --nocapture"]
async fn provided_shields_cost_no_more_per_frame_than_sprite_shields() {
    for provided in [false, true] {
        let shields = Shields::new(Answer::Shield(SHIELD));
        let mut style = style("line", false);
        if !provided {
            style
                .set_layout_property("shield", "icon-image", serde_json::json!("marker"))
                .expect("sprite shields");
        }
        let server = AssetServer::default();
        server.serve("https://tiles.test/", road_tile("US:I", "287"));
        let mut map = SymbolMap::serving(style, server).await;
        let providers = map.map.image_providers().expect("registry");
        providers.register("shield", shields.clone());
        // Cold: from the first frame until nothing loads and every shield is drawn.
        let mut cold = Vec::new();
        while cold.len() < 600 && (cold.is_empty() || map.map.needs_redraw()) {
            cold.push(timed_frame(&mut map).await);
        }
        let after_cold = providers.stats();
        let fetched = map.server.requested().len();
        // Warm: panning across tiles whose shields are known.
        let mut warm = Vec::new();
        for _ in 0..240 {
            map.map
                .view_state_mut()
                .camera_mut()
                .move_relative(cgmath::Vector2::new(8.0, 3.0));
            warm.push(timed_frame(&mut map).await);
        }
        let after_warm = providers.stats();
        eprintln!(
            "{}: cold {} frames, p95 {:?}; warm pan p50 {:?} p95 {:?}; tiles fetched {} then {}; \
             calls {} then {}; relaid tiles {} then {}; cache hits {}; answers held {} bytes",
            if provided { "provided" } else { "sprite" },
            cold.len(),
            percentile(&mut cold, 0.95),
            percentile(&mut warm, 0.5),
            percentile(&mut warm, 0.95),
            fetched,
            map.server.requested().len() - fetched,
            after_cold.calls,
            after_warm.calls - after_cold.calls,
            after_cold.relaid,
            after_warm.relaid - after_cold.relaid,
            after_warm.cache_hits,
            after_warm.cached_bytes,
        );
        assert_eq!(
            after_warm.calls, after_cold.calls,
            "no shield is made while panning"
        );
        assert_eq!(
            after_warm.relaid, after_cold.relaid,
            "no tile is laid out twice while panning"
        );
    }
}
