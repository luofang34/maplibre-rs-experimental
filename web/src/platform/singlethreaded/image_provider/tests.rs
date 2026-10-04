#![allow(clippy::expect_used, clippy::panic)]

use js_sys::Function;
use wasm_bindgen_test::*;

use super::*;

fn provider(body: &str) -> JsImageProvider {
    JsImageProvider {
        draw: Function::new_with_args("id, ratio", body),
        generation: "pack-1".to_owned(),
    }
}

async fn ask(provider: &JsImageProvider, id: &str) -> Result<ImageResolution, ImageProviderError> {
    provider
        .provide(ImageRequest {
            name: format!("shield:{id}"),
            id: id.to_owned(),
            pixel_ratio: 2.0,
        })
        .await
}

#[wasm_bindgen_test]
async fn a_javascript_function_draws_an_image_now_or_later() {
    for body in [
        "return {width: 2, height: 1, data: new Uint8Array([1,2,3,4,5,6,7,8]), anchor: [1.5, 0.5]}",
        "return Promise.resolve({width: 2, height: 1, \
            data: new Uint8ClampedArray([1,2,3,4,5,6,7,8]), anchor: [1.5, 0.5]})",
    ] {
        let Ok(ImageResolution::Image(image)) = ask(&provider(body), "US:I=287").await else {
            panic!("an image from {body}");
        };
        assert_eq!((image.image.width, image.image.height), (2, 1));
        assert_eq!(image.image.data, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(image.image.pixel_ratio, 2.0, "the request's ratio by default");
        assert!(!image.image.sdf);
        assert_eq!(image.anchor, Some([1.5, 0.5]));
    }
}

#[wasm_bindgen_test]
async fn a_javascript_function_answers_absent_unavailable_or_failed() {
    let absent = ["return null", "return {status: 'absent'}"];
    for body in absent {
        assert_eq!(ask(&provider(body), "x").await, Ok(ImageResolution::Absent), "{body}");
    }
    assert!(matches!(
        ask(&provider("return {status: 'unavailable', reason: 'pack loading'}"), "x").await,
        Err(ImageProviderError::Unavailable(reason)) if reason == "pack loading"
    ));
    for body in [
        "return {status: 'failed'}",
        "throw new Error('bad route')",
        "return Promise.reject(new Error('bad route'))",
        "return {width: 2, height: 1, data: [1,2,3]}",
        "return {width: 0.5, height: 1, data: new Uint8Array(4)}",
    ] {
        assert!(
            matches!(ask(&provider(body), "x").await, Err(ImageProviderError::Failed(_))),
            "{body}"
        );
    }
}

#[wasm_bindgen_test]
fn registering_a_function_serves_its_namespace_in_this_module() {
    register_image_provider("jsshield", "pack-1", Function::new_with_args("id, ratio", "return null"));
    assert!(crate::platform::image_providers().provides("jsshield:US:I=287"));
}
