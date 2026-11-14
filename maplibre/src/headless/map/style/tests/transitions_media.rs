use serde_json::json;

use super::{center_rgba, vector_map, world_box_feature};
use crate::{
    background::BackgroundPlugin,
    headless::{create_headless_renderer, map::HeadlessMap, HeadlessPlugin},
    raster::{DefaultRasterTransferables, RasterPlugin},
    render::RenderPlugin,
    style::Style,
};

#[test]
fn video_and_canvas_sources_render_pushed_rgba_frames_and_corner_updates() {
    crate::headless::map::test_async::run_async(async {
        let full_quad = [
            [-180.0, 85.0511],
            [180.0, 85.0511],
            [180.0, -85.0511],
            [-180.0, -85.0511],
        ];
        let style: Style = serde_json::from_value(json!({
            "version": 8,
            "center": [0.0, 0.0],
            "zoom": 0.0,
            "sources": {
                "vid": {
                    "type": "video",
                    "urls": ["offline://clip.mp4"],
                    "coordinates": full_quad
                },
                "canv": {
                    "type": "canvas",
                    "canvas": "hud-canvas",
                    "animate": true,
                    "coordinates": full_quad
                }
            },
            "layers": [
                {"id": "bg", "type": "background", "paint": {"background-color": "#000000"}},
                {"id": "vid-layer", "type": "raster", "source": "vid"},
                {"id": "canv-layer", "type": "raster", "source": "canv"}
            ]
        }))
        .expect("valid media style");
        let (kernel, renderer) = create_headless_renderer(64, 64, None)
            .await
            .expect("renderer");
        let mut map = HeadlessMap::new(
            style,
            renderer,
            kernel,
            vec![
                Box::new(RenderPlugin),
                Box::new(BackgroundPlugin),
                Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
                Box::new(HeadlessPlugin::new(false).preserve_tile_sources()),
            ],
        )
        .expect("headless map");

        assert!(map
            .set_video_source_frame("vid", 2, 2, &[0; 15], None)
            .is_err());
        assert!(map
            .set_video_source_frame("canv", 1, 1, &[0; 4], None)
            .is_err());
        assert!(map
            .set_canvas_source_frame("vid", 1, 1, &[0; 4], None)
            .is_err());

        let solid = |rgba: [u8; 4]| -> Vec<u8> { rgba.repeat(16) };
        map.set_video_source_frame("vid", 4, 4, &solid([255, 0, 0, 255]), None)
            .expect("red video frame");
        map.run_frame().expect("frame 1");
        map.run_frame().expect("frame 2");
        assert_eq!(center_rgba(&map), [255, 0, 0, 255]);

        map.set_video_source_frame("vid", 4, 4, &solid([0, 255, 0, 255]), None)
            .expect("green video frame");
        map.run_frame().expect("frame 3");
        assert_eq!(center_rgba(&map), [0, 255, 0, 255]);

        map.set_canvas_source_frame("canv", 4, 4, &solid([0, 0, 255, 255]), None)
            .expect("blue canvas frame");
        map.run_frame().expect("frame 4");
        assert_eq!(center_rgba(&map), [0, 0, 255, 255]);

        // Move canvas quad away from the origin so the underlying green video shows at center.
        let corner_quad = [[120.0, 70.0], [170.0, 70.0], [170.0, 30.0], [120.0, 30.0]];
        map.set_canvas_coordinates("canv", corner_quad)
            .expect("move canvas corners");
        map.run_frame().expect("frame 5");
        assert_eq!(center_rgba(&map), [0, 255, 0, 255]);
    })
}

#[test]
fn paint_transitions_and_set_style_diffing_work_in_headless_frames() {
    crate::headless::map::test_async::run_async(async {
        let mut map = vector_map(json!({
            "version": 8,
            "center": [0.0, 0.0],
            "zoom": 0.0,
            "sources": {
                "parks": {
                    "type": "geojson",
                    "data": {"type": "FeatureCollection", "features": [world_box_feature(json!({}))]}
                }
            },
            "layers": [
                {"id": "bg", "type": "background", "paint": {"background-color": "#000000"}},
                {
                    "id": "park",
                    "type": "fill",
                    "source": "parks",
                    "paint": {
                        "fill-color": "#000000",
                        "fill-color-transition": {"duration": 64}
                    }
                }
            ]
        }))
        .await;
        assert_eq!(center_rgba(&map), [0, 0, 0, 255]);
        map.mutate_style(|style| style.set_paint_property("park", "fill-color", json!("#ffffff")))
            .expect("start park fill-color transition");
        // Advance 2 headless frames (32 ms out of 64 ms): color is midway between 0 and 255.
        let step = std::time::Duration::from_millis(16);
        map.frame_input_mut().advance(step);
        map.run_frame().expect("t=16ms");
        map.frame_input_mut().advance(step);
        map.run_frame().expect("t=32ms");
        let mid = center_rgba(&map)[0];
        assert!(
            (40..=220).contains(&mid),
            "expected mid-transition gray, got {mid}"
        );
        for _ in 0..4 {
            map.frame_input_mut().advance(step);
            map.run_frame().expect("finish transition");
        }
        assert_eq!(center_rgba(&map), [255, 255, 255, 255]);

        // Whole-style replacement retaining identical `parks` GeoJSON source while recoloring `park`.
        let next_style: Style = serde_json::from_value(json!({
            "version": 8,
            "center": [0.0, 0.0],
            "zoom": 0.0,
            "sources": {
                "parks": {
                    "type": "geojson",
                    "data": {"type": "FeatureCollection", "features": [world_box_feature(json!({}))]}
                }
            },
            "layers": [
                {"id": "bg", "type": "background", "paint": {"background-color": "#000000"}},
                {"id": "park", "type": "fill", "source": "parks", "paint": {"fill-color": "#00ff00"}}
            ]
        }))
        .expect("next style");
        let diff = map.set_style(next_style);
        assert!(diff.updated_sources.is_empty());
        assert!(
            !diff.redraw_tiles,
            "identical parks source keeps loaded vector tiles"
        );
        map.run_frame().expect("diffed style frame");
        assert_eq!(center_rgba(&map), [0, 255, 0, 255]);
    })
}
