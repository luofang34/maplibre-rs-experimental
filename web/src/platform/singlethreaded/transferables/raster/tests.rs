#![allow(clippy::expect_used, clippy::panic)]
use wasm_bindgen_test::wasm_bindgen_test;

use super::*;

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
fn dem_raster_source_and_pixels_survive_worker_buffer_transfer() {
    let coords = WorldTileCoords::from((7, 3, 4_u8.into()));
    let image = RgbaImage::from_pixel(4, 4, image::Rgba([100, 128, 10, 255]));
    let unpack = [6553.6, 25.6, 0.1, -10000.0];
    let dem = Arc::new(DemTile::from_image_despiked(&image, unpack, 100.0).expect("dem"));
    let circumference = 40_075_016.685_578_49;
    let source: RasterSourceId = "dem".into();
    let message = <FlatBufferTransferable as LayerRaster>::build_from_dem(
        coords,
        source.clone(),
        Arc::clone(&dem),
        circumference,
        Arc::new(RgbaImage::new(0, 0)),
    );
    let decoded = transferred(message, WebMessageTag::LayerRaster);
    assert_eq!(LayerRaster::coords(&decoded), coords);
    let Ok(shared) = LayerRaster::into_dem_or_layer(decoded) else {
        panic!("expected raw DEM frame");
    };
    assert_eq!(shared.coords, coords);
    assert_eq!(shared.source, source);
    assert_eq!(*shared.tile, *dem);
    assert!((shared.circumference_meters - circumference).abs() < 1e-6);
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
