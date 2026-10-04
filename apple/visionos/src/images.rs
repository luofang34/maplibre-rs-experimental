//! C callbacks that make the images labels name in a namespace, such as road shields the host
//! draws from route attributes.

use std::{
    ffi::{c_char, c_void, CString},
    sync::Arc,
};

use maplibre::{
    sdf::assets::{
        ImageProviderError, ImageRequest, ImageResolution, ProvideFuture, ProvidedImage,
        StyleImageProvider,
    },
    style::StyleImage,
};

use crate::{api::c_str, MaplibreVisionOSMap};

/// The callback drew the image into its out-parameter.
pub const MAPLIBRE_VISIONOS_IMAGE_READY: i32 = 0;
/// The name is understood but has no image; the style's fallback is drawn.
pub const MAPLIBRE_VISIONOS_IMAGE_ABSENT: i32 = 1;
/// The name cannot be drawn; the answer is kept until the namespace is invalidated.
pub const MAPLIBRE_VISIONOS_IMAGE_FAILED: i32 = 2;
/// The image cannot be drawn yet; it is asked for again later.
pub const MAPLIBRE_VISIONOS_IMAGE_UNAVAILABLE: i32 = 3;

/// An image a host callback drew. Mirrors `MaplibreVisionOSImage` in the header.
#[repr(C)]
pub struct MaplibreVisionOSImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` straight-alpha RGBA bytes, read before the callback's caller
    /// returns.
    pub rgba: *const u8,
    /// Image pixels per layout pixel.
    pub pixel_ratio: f32,
    /// Whether `anchor_x` and `anchor_y` place the image instead of its centre.
    pub has_anchor: bool,
    /// The point, in image pixels from the top-left, placed where the centre would be.
    pub anchor_x: f32,
    /// See `anchor_x`.
    pub anchor_y: f32,
}

/// Draws the image named `id` (the name after the namespace and colon) for `pixel_ratio`
/// device pixels per layout pixel into `image` and returns one of the
/// `MAPLIBRE_VISIONOS_IMAGE_*` statuses.
pub type MaplibreVisionOSImageCallback = unsafe extern "C" fn(
    context: *mut c_void,
    id: *const c_char,
    pixel_ratio: f32,
    image: *mut MaplibreVisionOSImage,
) -> i32;

/// A host callback and its context.
struct CallbackProvider {
    callback: MaplibreVisionOSImageCallback,
    context: *mut c_void,
    generation: String,
}

// SAFETY: registration requires a callback that may be called from any thread, several
// calls at once, with a context that stays valid while the map lives.
unsafe impl Send for CallbackProvider {}
// SAFETY: as for `Send`; the provider only reads its fields.
unsafe impl Sync for CallbackProvider {}

impl CallbackProvider {
    fn answer(&self, request: &ImageRequest) -> Result<ImageResolution, ImageProviderError> {
        let id = CString::new(request.id.as_str())
            .map_err(|_| ImageProviderError::Failed("the id holds a NUL".to_owned()))?;
        let mut image = MaplibreVisionOSImage {
            width: 0,
            height: 0,
            rgba: std::ptr::null(),
            pixel_ratio: request.pixel_ratio,
            has_anchor: false,
            anchor_x: 0.0,
            anchor_y: 0.0,
        };
        // SAFETY: the host registered a callback that accepts these arguments from any
        // thread; `id` and `image` live until it returns.
        let status =
            unsafe { (self.callback)(self.context, id.as_ptr(), request.pixel_ratio, &mut image) };
        match status {
            MAPLIBRE_VISIONOS_IMAGE_READY => copied(&image).map(ImageResolution::Image),
            MAPLIBRE_VISIONOS_IMAGE_ABSENT => Ok(ImageResolution::Absent),
            MAPLIBRE_VISIONOS_IMAGE_UNAVAILABLE => Err(ImageProviderError::Unavailable(
                "the host cannot draw it yet".to_owned(),
            )),
            other => Err(ImageProviderError::Failed(format!(
                "the host's callback returned {other}"
            ))),
        }
    }
}

/// The image the callback drew, copied out of the host's memory.
fn copied(image: &MaplibreVisionOSImage) -> Result<ProvidedImage, ImageProviderError> {
    let bytes = image.width as usize * image.height as usize * 4;
    if image.rgba.is_null() || bytes == 0 {
        return Err(ImageProviderError::Failed(
            "the callback drew no pixels".to_owned(),
        ));
    }
    // SAFETY: a ready image holds `width * height * 4` readable bytes until the callback's
    // caller returns, which is after this copy.
    let data = unsafe { std::slice::from_raw_parts(image.rgba, bytes) }.to_vec();
    Ok(ProvidedImage {
        image: StyleImage {
            width: image.width,
            height: image.height,
            data,
            pixel_ratio: image.pixel_ratio,
            sdf: false,
        },
        anchor: image.has_anchor.then_some([image.anchor_x, image.anchor_y]),
    })
}

impl StyleImageProvider for CallbackProvider {
    fn generation(&self) -> String {
        self.generation.clone()
    }

    fn provide(&self, request: ImageRequest) -> ProvideFuture<'_> {
        Box::pin(async move { self.answer(&request) })
    }
}

/// Makes `callback` draw the images named `namespace:…` that neither the sprite nor the
/// style supplies, replacing any callback of that namespace. `generation` names the
/// resources the images come from; images made under another are not reused. Returns
/// whether the callback was registered.
///
/// # Safety
///
/// `map` must be a map created by this library, or null; `namespace` and `generation`
/// NUL-terminated strings. `callback` is called on tile worker threads, several calls at
/// once, never on the thread that draws, and `context` must stay valid while the map lives.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_register_image_provider(
    map: *mut MaplibreVisionOSMap,
    namespace: *const c_char,
    generation: *const c_char,
    callback: Option<MaplibreVisionOSImageCallback>,
    context: *mut c_void,
) -> bool {
    // SAFETY: the caller upholds the contract documented above.
    let (handle, namespace, generation) =
        unsafe { (map.as_ref(), c_str(namespace), c_str(generation)) };
    let (Some(handle), Some(namespace), Some(callback)) = (handle, namespace, callback) else {
        return false;
    };
    let Some(providers) = handle.map.image_providers() else {
        return false;
    };
    providers.register(
        namespace,
        Arc::new(CallbackProvider {
            callback,
            context,
            generation: generation.unwrap_or_default().to_owned(),
        }),
    );
    true
}

/// Forgets every image of `namespace` and requests again the tiles that drew one, such as
/// after the host's resources changed. Returns how many tiles are requested again.
///
/// # Safety
///
/// `map` must be a map created by this library, or null, used by its render thread only;
/// `namespace` a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn maplibre_visionos_invalidate_images(
    map: *mut MaplibreVisionOSMap,
    namespace: *const c_char,
) -> usize {
    // SAFETY: the caller upholds the contract documented above.
    let (handle, namespace) = unsafe { (map.as_mut(), c_str(namespace)) };
    match (handle, namespace) {
        (Some(handle), Some(namespace)) => handle.map.invalidate_provided_images(namespace),
        _ => 0,
    }
}

#[cfg(test)]
mod tests;
