use super::*;

/// Lets the drape timer's readback land and be taken, with no drape claiming it again.
fn settle_drape_timer(map: &mut HeadlessMap) -> Option<(std::time::Duration, u32)> {
    for _ in 0..4 {
        map.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll");
        map.run_frame().expect("still frame");
    }
    map.map_context
        .world
        .resources
        .get::<crate::terrain::drape_timing::DrapeCost>()
        .and_then(|cost| cost.last_frame())
}

#[test]
fn drape_time_is_measured_across_every_drape_a_frame_draws() {
    crate::headless::map::test_async::run_async(async {
        let mut map = prepared_map(false).await;
        if !map
            .device()
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
        {
            return;
        }
        // The pitch is a camera move; a budget of an hour lets its frame draw every drape, even
        // on a software rasterizer that takes most of a second for one.
        map.map_context
            .world
            .resources
            .insert(crate::terrain::DrapeBudget {
                moving_time: std::time::Duration::from_secs(3600),
                ..Default::default()
            });
        settle_drape_timer(&mut map);
        // Many drapes at once: a pitched view over tiles not drawn before.
        map.set_max_pitch(cgmath::Deg(85.0));
        map.map_context
            .view_state
            .camera_mut()
            .set_pitch(cgmath::Deg(70.0));
        let required = map
            .required_raster_tile_coords("paint")
            .expect("source covering");
        let missing: Vec<_> = required
            .iter()
            .filter(|coords| {
                map.map_context
                    .world
                    .tiles
                    .query::<&crate::raster::RasterLayersDataComponent>(**coords)
                    .is_none_or(|component| component.layers.is_empty())
            })
            .copied()
            .collect();
        // Apple Metal's cross-pass `MTLCounterSampleBuffer` intermittently drops the end sample
        // (`end <= start` discarded by `GpuTimer::take`); retry up to 4 times until measured.
        let mut many_result = None;
        let mut many_last_child = false;
        for attempt in 0..5_usize {
            let batch: Vec<_> = if attempt == 0 {
                missing
                    .iter()
                    .map(|coords| tile(*coords, false, false))
                    .collect()
            } else {
                many_last_child = attempt % 2 == 1;
                if let Some(Eventually::Initialized(raster)) =
                    map.map_context
                        .world
                        .resources
                        .get_mut::<Eventually<RasterResources>>()
                {
                    for coords in &required {
                        raster.remove_texture(*coords);
                    }
                }
                required
                    .iter()
                    .map(|coords| tile(*coords, false, many_last_child))
                    .collect()
            };
            map.render_sources(ProcessedLayers::default(), batch)
                .expect("pitched frame");
            let measured = settle_drape_timer(&mut map);
            if matches!(measured, Some((_, d)) if d >= 4) {
                many_result = measured;
                break;
            }
        }
        let (many, drawn) = many_result.expect("many drapes timed");
        // One drape: the content of a single tile is replaced.
        let replaced = required[0];
        let mut one_result = None;
        for attempt in 0..5_usize {
            let Some(Eventually::Initialized(raster)) = map
                .map_context
                .world
                .resources
                .get_mut::<Eventually<RasterResources>>()
            else {
                panic!("raster resources");
            };
            raster.remove_texture(replaced);
            let child = many_last_child ^ (attempt % 2 == 0);
            map.render_sources(
                ProcessedLayers::default(),
                vec![tile(replaced, false, child)],
            )
            .expect("one drape");
            assert_eq!(
                map.last_frame_stats().drape_redraws,
                1,
                "one drape shows the tile"
            );
            let measured = settle_drape_timer(&mut map);
            if matches!(measured, Some((_, 1))) {
                one_result = measured;
                break;
            }
        }
        let (one, single) = one_result.expect("one drape timed");
        assert_eq!(single, 1);
        assert!(drawn >= 4, "the pitched view draws several drapes: {drawn}");
        assert!(
            one > std::time::Duration::from_micros(10),
            "a {DRAPE}² drape takes measurable GPU time: {one:?}",
            DRAPE = crate::terrain::resources::DRAPE_SIZE
        );
        assert!(
            many.as_secs_f64() > one.as_secs_f64() * f64::from(drawn) / 3.0,
            "{drawn} drapes take about {drawn} times one: {many:?} against {one:?}"
        );
        // The moving budget divides its time by this, so it must be on the scale of a drape's cost;
        // a running average against one sample varies severalfold on a busy GPU.
        let per_drape = map
            .map_context
            .world
            .resources
            .get::<crate::terrain::drape_timing::DrapeCost>()
            .expect("measured")
            .per_drape();
        assert!(
            per_drape > one / 20 && per_drape < one * 20,
            "the cost the budget uses is one drape's: {per_drape:?} against {one:?}"
        );
    });
}
