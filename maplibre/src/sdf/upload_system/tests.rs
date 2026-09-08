#![allow(clippy::expect_used, clippy::panic)]

use std::collections::HashSet;

use super::pending_layer_data;
use crate::{
    coords::WorldTileCoords, sdf::SymbolLayerData, style::layer::StyleLayer,
    vector::tessellation::OverAlignedVertexBuffer,
};

fn layer_data(style_layer_id: &str) -> SymbolLayerData {
    SymbolLayerData {
        atlas: None,
        coords: WorldTileCoords::default(),
        source_layer: "place".to_string(),
        style_layer_id: style_layer_id.to_string(),
        buffer: OverAlignedVertexBuffer::empty(),
        new_buffer: OverAlignedVertexBuffer::empty(),
        features: Vec::new(),
    }
}

fn style_layer(id: &str) -> StyleLayer {
    serde_json::from_value(serde_json::json!({
        "id": id, "type": "symbol", "source": "openmaptiles", "source-layer": "place"
    }))
    .expect("valid style layer")
}

#[test]
fn style_layers_sharing_a_source_layer_each_get_their_own_data() {
    let layers = [layer_data("place_city"), layer_data("place_town")];

    let town = pending_layer_data(&layers, &HashSet::new(), &style_layer("place_town"))
        .expect("the town layer has data");
    assert_eq!(town.style_layer_id, "place_town");
    assert!(
        pending_layer_data(&layers, &HashSet::new(), &style_layer("place_village")).is_none(),
        "a style layer without tessellated data uploads nothing"
    );
}

#[test]
fn a_layer_already_in_the_pool_is_not_uploaded_again() {
    let layers = [layer_data("place_city")];
    let loaded = HashSet::from(["place_city".to_string()]);

    assert!(pending_layer_data(&layers, &loaded, &style_layer("place_city")).is_none());
}
