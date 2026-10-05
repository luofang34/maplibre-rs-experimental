//! Image providers written in JavaScript. A single-threaded build runs labels' providers in
//! its tile workers, each with its own copy of the module, so a host hands the workers a
//! JavaScript module that registers its providers there (see `imageProviderModules` of
//! `startMapLibre`).

use std::sync::Arc;

use js_sys::{Function, Promise, Reflect, Uint8Array};
use maplibre::{
    sdf::assets::{
        ImageProviderError, ImageRequest, ImageResolution, ProvideFuture, ProvidedImage,
        StyleImageProvider,
    },
    style::StyleImage,
};
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::JsFuture;

/// A JavaScript function `(id, pixelRatio) => image | Promise<image>`.
///
/// An image is `{width, height, data, pixelRatio?, anchor?}` with `data` straight-alpha RGBA
/// bytes and `anchor` an `[x, y]` in image pixels; `null`, `undefined` or `{status: "absent"}`
/// has no image; `{status: "unavailable" | "failed", reason?}` cannot be drawn for now or at
/// all.
struct JsImageProvider {
    draw: Function,
    generation: String,
}

// SAFETY: this module is built without atomics, so it runs on one thread and the function is
// never reached from another.
unsafe impl Send for JsImageProvider {}
// SAFETY: as for `Send`.
unsafe impl Sync for JsImageProvider {}

impl StyleImageProvider for JsImageProvider {
    fn generation(&self) -> String {
        self.generation.clone()
    }

    fn provide(&self, request: ImageRequest) -> ProvideFuture<'_> {
        Box::pin(async move { self.answer(&request).await })
    }
}

fn describe(error: &JsValue) -> String {
    error
        .as_string()
        .or_else(|| {
            Reflect::get(error, &"message".into())
                .ok()
                .and_then(|message| message.as_string())
        })
        .unwrap_or_else(|| format!("{error:?}"))
}

fn field(value: &JsValue, name: &str) -> JsValue {
    Reflect::get(value, &name.into()).unwrap_or(JsValue::UNDEFINED)
}

impl JsImageProvider {
    async fn answer(&self, request: &ImageRequest) -> Result<ImageResolution, ImageProviderError> {
        let failed = |error: &JsValue| ImageProviderError::Failed(describe(error));
        let mut value = self
            .draw
            .call2(
                &JsValue::NULL,
                &JsValue::from_str(&request.id),
                &JsValue::from_f64(f64::from(request.pixel_ratio)),
            )
            .map_err(|error| failed(&error))?;
        if let Some(promise) = value.dyn_ref::<Promise>() {
            value = JsFuture::from(promise.clone())
                .await
                .map_err(|error| failed(&error))?;
        }
        image(&value, request.pixel_ratio)
    }
}

/// What a provider function's answer says.
fn image(value: &JsValue, pixel_ratio: f32) -> Result<ImageResolution, ImageProviderError> {
    if value.is_null() || value.is_undefined() {
        return Ok(ImageResolution::Absent);
    }
    let reason = || {
        field(value, "reason")
            .as_string()
            .unwrap_or_else(|| "no reason given".to_owned())
    };
    match field(value, "status").as_string().as_deref() {
        None | Some("ready") => {}
        Some("absent") => return Ok(ImageResolution::Absent),
        Some("unavailable") => return Err(ImageProviderError::Unavailable(reason())),
        Some(_) => return Err(ImageProviderError::Failed(reason())),
    }
    let size = |name| {
        field(value, name)
            .as_f64()
            .filter(|size| size.fract() == 0.0 && (1.0..=4096.0).contains(size))
            .map(|size| size as u32)
            .ok_or_else(|| ImageProviderError::Failed(format!("{name} is not a pixel count")))
    };
    let (width, height) = (size("width")?, size("height")?);
    let data = field(value, "data");
    if !(data.is_instance_of::<Uint8Array>() || data.is_instance_of::<js_sys::Uint8ClampedArray>())
    {
        return Err(ImageProviderError::Failed(
            "data is not a Uint8Array or Uint8ClampedArray".to_owned(),
        ));
    }
    let anchor = field(value, "anchor");
    let anchor = js_sys::Array::is_array(&anchor)
        .then(|| {
            let anchor = js_sys::Array::from(&anchor);
            Some([
                anchor.get(0).as_f64()? as f32,
                anchor.get(1).as_f64()? as f32,
            ])
        })
        .flatten();
    Ok(ImageResolution::Image(ProvidedImage {
        image: StyleImage {
            width,
            height,
            data: Uint8Array::new(&data).to_vec(),
            pixel_ratio: field(value, "pixelRatio")
                .as_f64()
                .map_or(pixel_ratio, |ratio| ratio as f32),
            sdf: false,
        },
        anchor,
    }))
}

/// Makes `draw` the provider of the images named `namespace:…` in this copy of the module:
/// in a tile worker of a single-threaded build, the one that lays labels out. `generation`
/// names the resources the images come from; images made under another are not reused.
#[wasm_bindgen]
pub fn register_image_provider(namespace: &str, generation: &str, draw: Function) {
    crate::platform::image_providers().register(
        namespace,
        Arc::new(JsImageProvider {
            draw,
            generation: generation.to_owned(),
        }),
    );
}

#[cfg(test)]
mod tests;
