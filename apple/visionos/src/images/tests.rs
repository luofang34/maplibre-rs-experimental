use std::ffi::CStr;

use super::*;

/// Answers by the id: `ok` draws a 2 x 1 image anchored at its right pixel, `none` has no
/// image, `later` is unavailable and anything else fails.
unsafe extern "C" fn draw(
    context: *mut c_void,
    id: *const c_char,
    pixel_ratio: f32,
    image: *mut MaplibreVisionOSImage,
) -> i32 {
    // SAFETY: the test passes a pointer to its pixel buffer as the context.
    let pixels = unsafe { &*(context as *const [u8; 8]) };
    // SAFETY: the provider passes a NUL-terminated id and a writable image.
    let (id, image) = unsafe { (CStr::from_ptr(id).to_str().unwrap_or(""), &mut *image) };
    match id {
        "ok" => {
            image.width = 2;
            image.height = 1;
            image.rgba = pixels.as_ptr();
            image.pixel_ratio = pixel_ratio;
            image.has_anchor = true;
            image.anchor_x = 1.5;
            image.anchor_y = 0.5;
            MAPLIBRE_VISIONOS_IMAGE_READY
        }
        "none" => MAPLIBRE_VISIONOS_IMAGE_ABSENT,
        "later" => MAPLIBRE_VISIONOS_IMAGE_UNAVAILABLE,
        _ => 7,
    }
}

fn ask(provider: &CallbackProvider, id: &str) -> Result<ImageResolution, ImageProviderError> {
    provider.answer(&ImageRequest {
        name: format!("shield:{id}"),
        id: id.to_owned(),
        pixel_ratio: 2.0,
    })
}

#[test]
fn a_host_callback_answers_every_kind_of_request() {
    let mut pixels = [255_u8, 0, 0, 255, 0, 0, 255, 128];
    let provider = CallbackProvider {
        callback: draw,
        context: pixels.as_mut_ptr().cast(),
        generation: "pack-1".to_owned(),
    };
    let Ok(ImageResolution::Image(image)) = ask(&provider, "ok") else {
        panic!("an image");
    };
    assert_eq!((image.image.width, image.image.height), (2, 1));
    assert_eq!(image.image.data, pixels, "the pixels are copied");
    assert_eq!(image.image.pixel_ratio, 2.0);
    assert_eq!(image.anchor, Some([1.5, 0.5]));
    assert_eq!(ask(&provider, "none"), Ok(ImageResolution::Absent));
    assert!(matches!(
        ask(&provider, "later"),
        Err(ImageProviderError::Unavailable(_))
    ));
    assert!(matches!(
        ask(&provider, "bad"),
        Err(ImageProviderError::Failed(_))
    ));
    assert_eq!(provider.generation(), "pack-1");
}
