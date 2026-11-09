//! A page starts a map and drives it through the controller `startMapLibre` resolves to.
#![allow(clippy::expect_used, clippy::panic)]

use serde_json::{json, Value};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

/// A style without tiles, so the map needs nothing from its workers.
fn style() -> String {
    json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 1,
        "state": {"theme": {"default": "light"}},
        "sources": {
            "points": {"type": "geojson", "data": {"type": "FeatureCollection", "features": []}}
        },
        "layers": [
            {"id": "paper", "type": "background", "paint": {"background-color": "#ffffff"}}
        ]
    })
    .to_string()
}

/// Workers that ignore what they are sent.
fn inert_workers() -> js_sys::Function {
    js_sys::Function::new_no_args(
        "return new Worker(URL.createObjectURL(new Blob([''], {type: 'text/javascript'})))",
    )
}

fn create_canvas() -> web_sys::HtmlCanvasElement {
    let document = web_sys::window()
        .expect("window")
        .document()
        .expect("document");
    let canvas = document
        .create_element("canvas")
        .expect("canvas")
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .expect("canvas element");
    canvas.set_id("maplibre");
    canvas
        .style()
        .set_property("width", "64px")
        .expect("css width");
    canvas
        .style()
        .set_property("height", "48px")
        .expect("css height");
    document
        .body()
        .expect("body")
        .append_child(&canvas)
        .expect("attached canvas");
    canvas
}

fn arg(value: Value) -> JsValue {
    js_sys::JSON::parse(&value.to_string()).expect("JSON argument")
}

async fn resolved(promise: js_sys::Promise) -> Value {
    let value = JsFuture::from(promise).await.expect("the command succeeds");
    if value.is_undefined() {
        return Value::Null;
    }
    let json = js_sys::JSON::stringify(&value)
        .expect("JSON result")
        .as_string()
        .expect("string");
    serde_json::from_str(&json).expect("parsed result")
}

async fn rejected(promise: js_sys::Promise) -> String {
    let error = JsFuture::from(promise)
        .await
        .expect_err("the command is refused");
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}

fn assert_center(camera: &Value, lng: f64, lat: f64) {
    let center = &camera["center"];
    let (got_lng, got_lat) = (
        center[0].as_f64().expect("lng"),
        center[1].as_f64().expect("lat"),
    );
    assert!(
        (got_lng - lng).abs() < 1e-6 && (got_lat - lat).abs() < 1e-6,
        "center {center} is not [{lng}, {lat}]"
    );
}

/// Headless Chrome offers WebGPU without an adapter; a WebGL build draws with the software GL
/// set in `webdriver.json` once the page has no WebGPU to prefer.
fn prefer_webgl() {
    if cfg!(feature = "web-webgl") {
        js_sys::eval("Object.defineProperty(navigator, 'gpu', {value: undefined})")
            .expect("WebGPU hidden");
    }
}

#[wasm_bindgen_test]
async fn a_started_map_moves_changes_its_style_and_answers_queries() {
    prefer_webgl();
    let canvas = create_canvas();
    let map = web::run_maplibre(inert_workers(), Some(style()))
        .await
        .expect("the map starts");

    resolved(map.jump_to(arg(json!({"center": [10.0, 20.0], "zoom": 3.0})))).await;
    let camera = resolved(map.get_camera()).await;
    assert_center(&camera, 10.0, 20.0);
    assert_eq!(camera["zoom"].as_f64(), Some(3.0));

    let arrived = map.fly_to(arg(
        json!({"center": {"lng": 11.0, "lat": 21.0}, "zoom": 4.0, "duration": 50}),
    ));
    assert_eq!(resolved(arrived).await, json!(true), "the flight arrives");
    let camera = resolved(map.get_camera()).await;
    assert_center(&camera, 11.0, 21.0);
    assert!((camera["zoom"].as_f64().expect("zoom") - 4.0).abs() < 1e-9);

    let easing = map.ease_to(arg(json!({"center": [30.0, 0.0], "duration": 60_000})));
    resolved(map.stop()).await;
    assert_eq!(
        resolved(easing).await,
        json!(false),
        "a stopped move did not arrive"
    );

    resolved(map.set_global_state_property("theme".into(), arg(json!("dark")))).await;
    assert_eq!(
        resolved(map.get_global_state()).await["theme"],
        json!("dark")
    );
    resolved(map.set_global_state_property("theme".into(), JsValue::NULL)).await;
    assert_eq!(
        resolved(map.get_global_state()).await["theme"],
        json!("light"),
        "null restores the declared default"
    );

    resolved(map.set_paint_property(
        "paper".into(),
        "background-color".into(),
        arg(json!("#ff0000")),
    ))
    .await;
    let missing = rejected(map.set_paint_property(
        "nowhere".into(),
        "background-color".into(),
        arg(json!("#ff0000")),
    ))
    .await;
    assert!(missing.contains("nowhere"), "names the layer: {missing}");

    resolved(map.set_geojson_data(
        "points".into(),
        arg(json!({"type": "FeatureCollection", "features": []})),
    ))
    .await;

    assert_eq!(
        resolved(map.query_rendered_features(arg(json!([32.0, 24.0])), JsValue::UNDEFINED)).await,
        json!([]),
        "a background draws nothing to query"
    );
    let unknown = rejected(
        map.query_rendered_features(JsValue::UNDEFINED, arg(json!({"layers": ["nowhere"]}))),
    )
    .await;
    assert!(unknown.contains("nowhere"), "names the layer: {unknown}");

    let invalid = rejected(map.jump_to(arg(json!({"zoom": "near"})))).await;
    assert!(invalid.contains("invalid camera options"), "{invalid}");
    canvas.remove();
}
