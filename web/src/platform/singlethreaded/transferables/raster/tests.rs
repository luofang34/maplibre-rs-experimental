#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use wasm_bindgen_test::wasm_bindgen_test;

fn transferred(message: FlatBufferTransferable, tag: WebMessageTag) -> FlatBufferTransferable {
    let buffer = js_sys::Uint8Array::from(message.data()).buffer();
    FlatBufferTransferable::from_array_buffer(tag, buffer)
}

#[wasm_bindgen_test]
fn raster_source_and_pixels_survive_worker_buffer_transfer() {
    let coords = WorldTileCoords::from((7, 3, 4_u8.into()));
    for source in [
        RasterSourceId::default(),
        "".into(),
        "raster".into(),
        "elevation".into(),
    ] {
        let image = RgbaImage::from_pixel(2, 1, image::Rgba([12, 34, 56, 255]));
        let message = <FlatBufferTransferable as LayerRaster>::build_from(
            coords,
            source.clone(),
            image.clone(),
        );
        let decoded = transferred(message, WebMessageTag::LayerRaster);
        assert_eq!(LayerRaster::coords(&decoded), coords);
        let data = LayerRaster::to_layer(decoded);
        assert_eq!(data.source, source);
        assert_eq!(data.image, image);
    }
}

#[wasm_bindgen_test]
fn unavailable_raster_source_survives_worker_buffer_transfer() {
    let coords = WorldTileCoords::from((7, 3, 4_u8.into()));
    for source in [
        RasterSourceId::default(),
        "".into(),
        "raster".into(),
        "elevation".into(),
    ] {
        let message =
            <FlatBufferTransferable as LayerRasterMissing>::build_from(coords, source.clone());
        let decoded = transferred(message, WebMessageTag::LayerRasterMissing);
        assert_eq!(LayerRasterMissing::coords(&decoded), coords);
        let data = LayerRasterMissing::to_layer(decoded);
        assert_eq!(data.coords, coords);
        assert_eq!(data.source, source);
    }
}
