// The same visibility toggle from Rust in the browser, through the handle `web::run_map`
// resolves to.
use maplibre::{map::handle::MapHandle, style::Style};
use serde_json::json;
use web::{CurrentEnvironment, JSError};

pub fn build_style(mut style: Style) -> Style {
    style.center = Some([-71.977221, -13.517379]);
    style.zoom = Some(14.0);
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32 + 1;
    }
    style
}

/// Shows or hides `layer` (`water` or `building`) by setting its `visibility` layout property.
pub async fn set_layer_visible(
    map: &MapHandle<CurrentEnvironment>,
    layer: &'static str,
    visible: bool,
) -> Result<(), JSError> {
    let visibility = if visible { "visible" } else { "none" };
    map.call(move |map| map.set_layout_property(layer, "visibility", json!(visibility)))
        .await??;
    Ok(())
}
