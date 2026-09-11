//! Distance hierarchy of the demo basemap in a spatial view.
use maplibre::{sdf::visibility::SymbolVisibility, style::Style};
pub(crate) fn visibility(style: &Style) -> SymbolVisibility {
    let layer_distances = style
        .layers
        .iter()
        .filter(|layer| layer.type_ == "symbol")
        .map(|layer| {
            let distance = match layer.source_layer.as_deref() {
                Some("place") if layer.id.contains("country") || layer.id.contains("state") => {
                    500_000.0
                }
                Some("place") if layer.id.contains("city") => 80_000.0,
                Some("place") if layer.id.contains("town") => 40_000.0,
                Some("place") => 20_000.0,
                Some("transportation_name") => 8_000.0,
                _ => 12_000.0,
            };
            (layer.id.clone(), distance)
        })
        .collect();
    SymbolVisibility {
        layer_distances,
        altitude_scale: 8.0,
    }
}
