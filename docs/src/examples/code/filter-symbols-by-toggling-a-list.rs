// Dynamic layer filter updates through `MapHandle`.
use maplibre::{map::handle::MapHandle, style::Style};
#[cfg(not(target_arch = "wasm32"))]
use maplibre_winit::NativeEnvironment as CurrentEnvironment;
use serde_json::json;
#[cfg(target_arch = "wasm32")]
use web::{CurrentEnvironment, JSError};

#[cfg(not(target_arch = "wasm32"))]
type JSError = Box<dyn std::error::Error>;

pub fn build_style(base: Style) -> Result<Style, serde_json::Error> {
    let mut document = serde_json::to_value(base)?;
    document["center"] = json!([-77.04, 38.907]);
    document["zoom"] = json!(11.5);
    document["sources"]["places"] = json!({
        "type": "geojson",
        "data": {
            "type": "FeatureCollection",
            "features": [
                {
                    "type": "Feature",
                    "properties": {"category": "theatre"},
                    "geometry": {"type": "Point", "coordinates": [-77.038659, 38.931567]}
                },
                {
                    "type": "Feature",
                    "properties": {"category": "theatre"},
                    "geometry": {"type": "Point", "coordinates": [-77.003168, 38.894651]}
                },
                {
                    "type": "Feature",
                    "properties": {"category": "museum"},
                    "geometry": {"type": "Point", "coordinates": [-77.031706, 38.914581]}
                },
                {
                    "type": "Feature",
                    "properties": {"category": "museum"},
                    "geometry": {"type": "Point", "coordinates": [-77.020945, 38.878241]}
                },
                {
                    "type": "Feature",
                    "properties": {"category": "music"},
                    "geometry": {"type": "Point", "coordinates": [-77.007481, 38.876516]}
                },
                {
                    "type": "Feature",
                    "properties": {"category": "music"},
                    "geometry": {"type": "Point", "coordinates": [-77.090372, 38.881189]}
                }
            ]
        }
    });
    if let Some(layers) = document["layers"].as_array_mut() {
        layers.push(json!({
            "id": "places",
            "type": "circle",
            "source": "places",
            "paint": {
                "circle-radius": 8,
                "circle-stroke-width": 2,
                "circle-color": [
                    "match",
                    ["get", "category"],
                    "theatre", "#e11d48",
                    "museum", "#2563eb",
                    "#16a34a"
                ],
                "circle-stroke-color": "#ffffff"
            }
        }));
    }
    let mut style: Style = serde_json::from_value(document)?;
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32 + 1;
    }
    Ok(style)
}

/// Shows only the places whose `category` equals `category`, or all of them when `None`.
pub async fn filter_by_category(
    map: &MapHandle<CurrentEnvironment>,
    category: Option<&'static str>,
) -> Result<(), JSError> {
    let filter = category.map(|value| json!(["==", ["get", "category"], value]));
    map.call(move |map| map.set_filter("places", filter))
        .await??;
    Ok(())
}
