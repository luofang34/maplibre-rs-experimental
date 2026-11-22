//! Browser-native asynchronous PNG/JPEG/WebP/GIF/AVIF RGBA8 decoding via `createImageBitmap` and
//! `OffscreenCanvas`, avoiding software codec crates (`png`, `zune-jpeg`, `image-webp`, `gif`) in
//! core WASM bundles and working identically on main and worker threads (including
//! `SharedArrayBuffer` multithreaded builds).

use image::RgbaImage;
use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Blob, ImageBitmap, OffscreenCanvas, OffscreenCanvasRenderingContext2d};

fn js_error_text(err: &JsValue) -> String {
    err.as_string()
        .or_else(|| {
            Reflect::get(err, &JsValue::from_str("message"))
                .ok()
                .and_then(|msg| msg.as_string())
        })
        .unwrap_or_else(|| "js error".to_owned())
}

#[cfg(web_sys_unstable_apis)]
fn get_image_data(
    ctx: &OffscreenCanvasRenderingContext2d,
    width: i32,
    height: i32,
) -> Result<web_sys::ImageData, JsValue> {
    ctx.get_image_data(0, 0, width, height)
}

#[cfg(not(web_sys_unstable_apis))]
fn get_image_data(
    ctx: &OffscreenCanvasRenderingContext2d,
    width: i32,
    height: i32,
) -> Result<web_sys::ImageData, JsValue> {
    ctx.get_image_data(0.0, 0.0, f64::from(width), f64::from(height))
}

/// Decodes encoded image bytes into straight-alpha RGBA8 pixels using the browser's image decoder.
pub(crate) async fn decode_rgba_bytes(bytes: &[u8]) -> Result<RgbaImage, String> {
    if bytes.is_empty() {
        return Err("cannot decode empty image payload".to_owned());
    }
    let len = u32::try_from(bytes.len()).map_err(|_| "image payload exceeds 4 GiB".to_owned())?;
    // Allocate a fresh unshared ArrayBuffer view so `new Blob([...])` succeeds even when WASM
    // linear memory is backed by `SharedArrayBuffer` (`+atomics`).
    let unshared = Uint8Array::new_with_length(len);
    unshared.copy_from(bytes);
    let parts = Array::of1(&unshared);
    let blob = Blob::new_with_u8_array_sequence(&parts).map_err(|e| js_error_text(&e))?;

    let global = js_sys::global();
    let create_fn = Reflect::get(&global, &JsValue::from_str("createImageBitmap"))
        .map_err(|e| js_error_text(&e))?
        .dyn_into::<Function>()
        .map_err(|_| "createImageBitmap is unavailable in this JS global scope".to_owned())?;

    let options = Object::new();
    let _ = Reflect::set(
        &options,
        &JsValue::from_str("colorSpaceConversion"),
        &JsValue::from_str("none"),
    );

    let promise = create_fn
        .call2(&global, &blob, &options)
        .map_err(|e| js_error_text(&e))?
        .dyn_into::<Promise>()
        .map_err(|_| "createImageBitmap did not return a Promise".to_owned())?;

    let bitmap = JsFuture::from(promise)
        .await
        .map_err(|e| js_error_text(&e))?
        .dyn_into::<ImageBitmap>()
        .map_err(|_| "createImageBitmap resolved to non-ImageBitmap".to_owned())?;

    let width = bitmap.width();
    let height = bitmap.height();
    let (Ok(w_i32), Ok(h_i32)) = (i32::try_from(width), i32::try_from(height)) else {
        bitmap.close();
        return Err("decoded image dimensions overflow i32".to_owned());
    };
    if width == 0 || height == 0 {
        bitmap.close();
        return Err("decoded zero-sized image".to_owned());
    }

    let canvas = match OffscreenCanvas::new(width, height) {
        Ok(canvas) => canvas,
        Err(err) => {
            bitmap.close();
            return Err(js_error_text(&err));
        }
    };
    let ctx_opts = Object::new();
    let _ = Reflect::set(
        &ctx_opts,
        &JsValue::from_str("willReadFrequently"),
        &JsValue::TRUE,
    );
    let ctx = canvas
        .get_context_with_context_options("2d", &ctx_opts)
        .ok()
        .flatten()
        .and_then(|obj| obj.dyn_into::<OffscreenCanvasRenderingContext2d>().ok());
    let Some(ctx) = ctx else {
        bitmap.close();
        return Err("OffscreenCanvas 2D context is unavailable".to_owned());
    };

    let draw_res = ctx.draw_image_with_image_bitmap(&bitmap, 0.0, 0.0);
    bitmap.close();
    draw_res.map_err(|e| js_error_text(&e))?;

    let image_data = get_image_data(&ctx, w_i32, h_i32).map_err(|e| js_error_text(&e))?;
    let raw = image_data.data().0;
    RgbaImage::from_raw(width, height, raw)
        .ok_or_else(|| "decoded RGBA8 byte length did not match width * height * 4".to_owned())
}
