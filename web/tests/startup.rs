#![allow(clippy::expect_used, clippy::panic)]

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
async fn worker_creation_failure_preserves_the_javascript_message() {
    let document = web_sys::window()
        .expect("window")
        .document()
        .expect("document");
    let canvas = document.create_element("canvas").expect("canvas");
    canvas.set_id("maplibre");
    document
        .body()
        .expect("body")
        .append_child(&canvas)
        .expect("attached canvas");
    let worker = js_sys::Function::new_no_args("throw new Error('worker creation denied')");
    let style = r#"{"version":8,"sources":{},"layers":[]}"#;
    let error = web::run_maplibre(worker, Some(style.into()))
        .await
        .expect_err("worker creation must reject initialization");
    assert!(error.to_string().contains("worker creation denied"));
    canvas.remove();
}

#[wasm_bindgen_test]
async fn malformed_style_rejects_with_parse_context_before_starting_workers() {
    let worker = js_sys::Function::new_no_args("throw new Error('worker must not start')");
    let error = web::run_maplibre(worker, Some("{invalid".into()))
        .await
        .expect_err("malformed style must reject initialization");
    let message = error.to_string();
    assert!(message.contains("invalid map style"), "{message}");
    assert!(
        message.contains("line 1"),
        "parse location is preserved: {message}"
    );
}
