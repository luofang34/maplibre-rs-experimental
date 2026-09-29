#![allow(clippy::expect_used, clippy::panic)]

use maplibre::{coords::WorldTileCoords, vector::TileTessellated};
use wasm_bindgen_test::wasm_bindgen_test;

use super::*;

#[wasm_bindgen_test]
fn completion_state_survives_worker_buffer_transfer() {
    let coords = WorldTileCoords::from((7, 3, 4_u8.into()));
    for (message, pending, failed) in [
        (FlatBufferTransferable::build_from(coords), false, false),
        (FlatBufferTransferable::build_partial(coords), true, false),
        (
            FlatBufferTransferable::build_failed(coords, true),
            true,
            true,
        ),
        (
            FlatBufferTransferable::build_failed(coords, false),
            false,
            true,
        ),
    ] {
        let buffer = js_sys::Uint8Array::from(message.data()).buffer();
        let decoded =
            FlatBufferTransferable::from_array_buffer(WebMessageTag::TileTessellated, buffer);
        assert_eq!(TileTessellated::coords(&decoded), coords);
        assert_eq!(decoded.pending_symbols(), pending);
        assert_eq!(decoded.failed(), failed);
    }
}
