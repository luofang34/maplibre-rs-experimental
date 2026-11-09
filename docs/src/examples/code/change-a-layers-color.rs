// The same changes from Rust in the browser, through the handle `web::run_map` resolves to.
use maplibre::{map::handle::MapHandle, style::Style};
use serde_json::json;
use web::{CurrentEnvironment, JSError};

pub fn build_style(base: Style) -> Result<Style, serde_json::Error> {
    let mut document = serde_json::to_value(base)?;
    document["center"] = json!([12.338, 45.4385]);
    document["zoom"] = json!(14);
    // The background follows a theme a page can switch without editing the layer.
    document["state"] = json!({"theme": {"default": "day"}});
    if let Some(layers) = document["layers"].as_array_mut() {
        for layer in layers
            .iter_mut()
            .filter(|layer| layer["id"] == "background")
        {
            layer["paint"]["background-color"] = json!([
                "match",
                ["global-state", "theme"],
                "night",
                "#1d2330",
                "#f8f4f0"
            ]);
        }
    }
    let mut style: Style = serde_json::from_value(document)?;
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32 + 1;
    }
    Ok(style)
}

/// Paints the layer `layer` (`water` or `building`) with `color`.
pub async fn recolor(
    map: &MapHandle<CurrentEnvironment>,
    layer: &'static str,
    color: &'static str,
) -> Result<(), JSError> {
    map.call(move |map| map.set_paint_property(layer, "fill-color", json!(color)))
        .await??;
    Ok(())
}

/// Switches the background between its day and night colors.
pub async fn set_night(map: &MapHandle<CurrentEnvironment>, night: bool) -> Result<(), JSError> {
    let theme = if night { "night" } else { "day" };
    map.call(move |map| map.set_global_state("theme", json!(theme)))
        .await?;
    Ok(())
}
