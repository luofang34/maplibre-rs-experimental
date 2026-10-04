//! A label holds to the ground under it while the view moves steadily: the map panning,
//! turning or tilting, or a tracked head turning. Its drawn offset from a circle on the same
//! point changes smoothly from frame to frame, with no jump a pixel snap or a placement
//! lagging the view would make.

use super::{AssetServer, SymbolMap, FONT, GLYPHS, SIZE, SPRITE};
use crate::style::Style;

/// The largest frame-to-frame change in velocity, in pixels, the label's offset from its
/// circle may show. Text this large measures to about a tenth of that; a snapped or lagging
/// label jumps by a pixel.
const STEADY: f64 = 0.5;

fn style() -> Style {
    serde_json::from_value(serde_json::json!({
        "version":8,"center":[0.01,0.010986328],"zoom":14,"pitch":55,"bearing":15,
        "glyphs":GLYPHS,"sprite":SPRITE,
        "sources":{"spot":{"type":"geojson","data":{"type":"Feature","properties":{"name":"Lake"},
            "geometry":{"type":"Point","coordinates":[0.0095,0.0099]}}}},
        "layers":[
            {"id":"background","type":"background","paint":{"background-color":"#000000"}},
            {"id":"dot","type":"circle","source":"spot",
                "paint":{"circle-radius":10,"circle-color":"#0000ff","circle-pitch-alignment":"map"}},
            {"id":"spot-name","type":"symbol","source":"spot",
                "layout":{"text-field":["get","name"],"text-font":[FONT],"text-size":64,
                    "text-allow-overlap":true,"text-anchor":"bottom","text-offset":[0,-0.6]},
                "paint":{"text-color":"#00ff00"}}
        ]
    }))
    .expect("style")
}

/// The centroid of the pixels in which `channel` dominates the others, weighted by how much,
/// within `radius` of `around`.
fn centroid(pixels: &[u8], channel: usize, around: [f64; 2], radius: f64) -> [f64; 2] {
    let (mut x_sum, mut y_sum, mut weight_sum) = (0.0, 0.0, 0.0);
    let low = |at: f64| (at - radius).max(0.0) as u32;
    let high = |at: f64| ((at + radius) as u32).min(SIZE);
    for y in low(around[1])..high(around[1]) {
        for x in low(around[0])..high(around[0]) {
            let index = ((y * SIZE + x) * 4) as usize;
            let others = (0..3)
                .filter(|other| *other != channel)
                .map(|other| f64::from(pixels[index + other]))
                .fold(0.0, f64::max);
            let weight = (f64::from(pixels[index + channel]) - others).max(0.0);
            x_sum += weight * (f64::from(x) + 0.5);
            y_sum += weight * (f64::from(y) + 0.5);
            weight_sum += weight;
        }
    }
    assert!(
        weight_sum > 0.0,
        "channel {channel} is drawn near {around:?}"
    );
    [x_sum / weight_sum, y_sum / weight_sum]
}

/// The label's offset from its circle in each frame after `frames` moves of the view.
async fn offsets(
    map: &mut SymbolMap,
    frames: u32,
    mut draw: impl AsyncFnMut(&mut SymbolMap, u32),
) -> Vec<[f64; 2]> {
    let first = map.read();
    let mut text = centroid(&first, 1, [256.0, 256.0], 250.0);
    let mut circle = centroid(&first, 2, [256.0, 256.0], 250.0);
    let mut offsets = Vec::new();
    for frame in 1..=frames {
        draw(map, frame).await;
        let pixels = map.read();
        text = centroid(&pixels, 1, text, 120.0);
        circle = centroid(&pixels, 2, circle, 40.0);
        offsets.push([text[0] - circle[0], text[1] - circle[1]]);
    }
    offsets
}

fn assert_steady(offsets: &[[f64; 2]], case: &str) {
    let worst = offsets
        .windows(3)
        .map(|w| (w[2][0] - 2.0 * w[1][0] + w[0][0]).hypot(w[2][1] - 2.0 * w[1][1] + w[0][1]))
        .fold(0.0, f64::max);
    assert!(
        worst < STEADY,
        "{case}: the label jumps by {worst} px against its circle: {offsets:.2?}"
    );
}

#[tokio::test]
async fn a_label_holds_to_its_ground_while_the_map_pans_turns_and_tilts() {
    for case in ["pan", "turn", "tilt"] {
        let mut map = SymbolMap::serving(style(), AssetServer::default()).await;
        map.settle().await;
        let offsets = offsets(&mut map, 30, async |map, _| {
            let camera = map.map.view_state_mut().camera_mut();
            match case {
                "pan" => camera.move_relative(cgmath::Vector2::new(0.6, 0.4)),
                "turn" => {
                    let bearing = camera.get_bearing();
                    camera.set_bearing(bearing + cgmath::Rad(0.5_f64.to_radians()));
                }
                _ => {
                    let pitch = camera.get_pitch();
                    camera.set_pitch(pitch + cgmath::Rad(0.3_f64.to_radians()));
                }
            }
            map.frame().await;
        })
        .await;
        assert_steady(&offsets, case);
    }
}

/// One eye 600 m south of the label and 350 m up, looking north and 30 degrees down, turned
/// `yaw` degrees and moved `drift` metres east.
fn head(yaw: f64, drift: f64, clock: u32) -> crate::render::xr::XrFrame {
    use cgmath::{Matrix4, Rad, Vector3};

    use crate::render::{
        camera::EyeFrustum,
        view_state::ExternalAnchor,
        xr::{EyeTarget, ScenePlacement, XrEye, XrFrame},
    };
    XrFrame {
        opaque_environment: true,
        timestamp: std::time::Duration::from_millis(u64::from(clock) * 11),
        placement: ScenePlacement {
            anchor: ExternalAnchor {
                position: crate::coords::LatLon::new(0.0099, 0.0095),
                altitude_meters: 0.0,
            },
            world_from_scene: Matrix4::from_scale(1.0),
        },
        eyes: vec![XrEye {
            world_from_eye: Matrix4::from_translation(Vector3::new(drift, -600.0, 350.0))
                * Matrix4::from_angle_z(Rad(yaw.to_radians()))
                * Matrix4::from_angle_x(Rad(60_f64.to_radians())),
            frustum: EyeFrustum::symmetric(Rad(1.2), 1.0, 0.1, 100_000.0),
            target: EyeTarget {
                color: None,
                depth: None,
            },
        }],
        request_overscan: 1.0,
        prefetch: None,
    }
}

#[tokio::test]
async fn a_label_holds_to_its_ground_while_a_tracked_head_turns() {
    let mut map = SymbolMap::serving(style(), AssetServer::default()).await;
    // Labels place on a slower cadence in a tracked view; the still head lets them load
    // and fade in first.
    for clock in 0..120 {
        map.map
            .run_xr_frame(head(0.0, 0.0, clock))
            .expect("still head");
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }
    let offsets = offsets(&mut map, 40, async |map, frame| {
        let step = f64::from(frame);
        map.map
            .run_xr_frame(head(step * 0.2, step * 0.5, 120 + frame))
            .expect("turning head");
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_steady(&offsets, "tracked head");
}
