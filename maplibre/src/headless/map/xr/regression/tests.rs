#![allow(clippy::expect_used, clippy::panic)]

use crate::{
    context::MapContext,
    coords::LatLon,
    headless::{create_headless_renderer, map::HeadlessMap},
    render::{
        camera::EyeFrustum,
        eventually::{Eventually, Eventually::Initialized},
        frame_input::frame_input_system,
        view_state::ExternalAnchor,
        xr::{EyeTarget, ScenePlacement, XrEye, XrFrame},
        RenderPlugin, RenderStageLabel,
    },
    style::Style,
    tcs::system::{stage::SystemStage, SystemResult},
    terrain::{resources::TerrainResources, DrapePhase},
};
use cgmath::{Matrix4, Rad, SquareMatrix, Vector3};
use std::time::Duration;

#[derive(Default)]
struct Observations {
    extractions: usize,
    eyes: Vec<(usize, Vec<crate::coords::WorldTileCoords>, usize)>,
}

fn ingest(context: &mut MapContext) -> SystemResult {
    let observations = context.world.resources.get_or_init_mut::<Observations>();
    observations.extractions = observations.extractions.wrapping_add(1);
    Ok(())
}

fn observe(context: &mut MapContext) -> SystemResult {
    let world = &mut context.world;
    let drawn = match world.resources.get::<Eventually<TerrainResources>>() {
        Some(Initialized(terrain)) => terrain.draws().iter().map(|draw| draw.coords).collect(),
        _ => Vec::new(),
    };
    let drapes = world
        .resources
        .get::<DrapePhase>()
        .map_or(0, |phase| phase.targets.len());
    let observations = world.resources.get_or_init_mut::<Observations>();
    observations
        .eyes
        .push((observations.extractions, drawn, drapes));
    Ok(())
}

pub(super) fn frame(timestamp: u64) -> XrFrame {
    XrFrame {
        opaque_environment: false,
        timestamp: Duration::from_millis(timestamp),
        placement: ScenePlacement {
            anchor: ExternalAnchor {
                position: LatLon::new(47.26, 11.39),
                altitude_meters: 0.0,
            },
            world_from_scene: Matrix4::identity(),
        },
        eyes: [-0.032, 0.032]
            .into_iter()
            .map(|x| XrEye {
                world_from_eye: Matrix4::from_translation(Vector3::new(x, 0.0, 4000.0)),
                frustum: EyeFrustum::symmetric(Rad(1.4), 1.0, 0.05, 1.0e8),
                target: EyeTarget::default(),
            })
            .collect(),
        request_overscan: 1.5,
        prefetch: None,
    }
}

#[tokio::test]
async fn stereo_ingests_and_refines_terrain_once_per_frame() {
    let style: Style = serde_json::from_str(r##"{"version":8,"sources":{"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"]}},"layers":[{"id":"background","type":"background","paint":{"background-color":"#718474"}}],"terrain":{"source":"dem"}}"##).expect("terrain style");
    let (kernel, renderer) = create_headless_renderer(2048, 2048, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(crate::terrain::TerrainPlugin::<
                crate::terrain::DefaultDemTransferables,
            >::default()),
        ],
    )
    .expect("map");
    // Deterministic ingestion at the same stage as worker results, without network timing.
    map.schedule.remove_stage(RenderStageLabel::Extract);
    map.schedule.add_stage_before(
        RenderStageLabel::Prepare,
        RenderStageLabel::Extract,
        SystemStage::default()
            .with_system(frame_input_system)
            .with_system(ingest),
    );
    map.schedule
        .add_system_to_stage(RenderStageLabel::PhaseSort, observe);
    map.run_xr_frame(frame(16)).expect("first stereo frame");
    map.run_xr_frame(frame(32)).expect("second stereo frame");
    let observations = map
        .world()
        .resources
        .get::<Observations>()
        .expect("observations");
    assert_eq!(observations.extractions, 2);
    for pair in observations.eyes.chunks_exact(2) {
        assert_eq!(
            pair[0].0, pair[1].0,
            "both eyes have the same ingestion generation"
        );
        assert_eq!(
            pair[0].1, pair[1].1,
            "both eyes draw the same terrain refinements"
        );
        assert!(!pair[0].1.is_empty());
        assert!(pair[0].2 <= 8);
        assert_eq!(pair[1].2, 0, "later eyes reuse completed drapes");
    }
    assert!(observations.eyes[0].2 > 0);
    assert_eq!(
        observations.eyes[2].1, observations.eyes[0].1,
        "texture refinement keeps the entire stationary surface visible"
    );
}

#[tokio::test]
async fn head_motion_and_stationary_frames_both_request_visible_tiles() {
    use crate::{projection::ProjectionType, render::frame_input::ViewSource, schedule::Stage};
    let style: Style = serde_json::from_str(r#"{"version":8,"sources":{"v":{"type":"vector","tiles":["https://unused.example/{z}/{x}/{y}.pbf"]}},"layers":[]}"#).expect("style");
    let (kernel, renderer) = create_headless_renderer(128, 128, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::vector::VectorPlugin::<
                crate::vector::DefaultVectorTransferables,
            >::default()),
        ],
    )
    .expect("map");
    let frame = frame(16);
    let eye = &frame.eyes[0];
    let initial = frame
        .placement
        .view_from(eye.world_from_eye, eye.frustum)
        .expect("initial view");
    let settled = frame
        .placement
        .view_from(
            Matrix4::from_translation(Vector3::new(0.0, 0.0, 2000.0)),
            eye.frustum,
        )
        .expect("destination view");
    map.map_context
        .view_state
        .set_external_view(initial, &ProjectionType::Mercator)
        .expect("initial pose");
    map.map_context.view_state.update_references();
    map.frame_input_mut().view = ViewSource::External(settled);
    map.schedule
        .get_stage_mut::<SystemStage>(&RenderStageLabel::Extract)
        .expect("extract")
        .run(&mut map.map_context)
        .expect("moving frame");
    assert!(!map.view_state().eye_settled());
    assert!(
        !map.world().tiles.tiles.is_empty(),
        "head motion must not suspend visible requests"
    );
    map.map_context.world.tiles.clear();
    map.map_context.view_state.update_references();
    map.schedule
        .get_stage_mut::<SystemStage>(&RenderStageLabel::Extract)
        .expect("extract")
        .run(&mut map.map_context)
        .expect("stationary frame");
    assert!(map.view_state().eye_settled());
    assert!(!map.view_state().did_camera_change());
    assert!(
        !map.world().tiles.tiles.is_empty(),
        "settling resumes requests without head movement"
    );
}
