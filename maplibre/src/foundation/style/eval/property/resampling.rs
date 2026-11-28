//! Raster and DEM resampling mode parsed from JSON style properties.

use serde::{Deserialize, Serialize};

use super::StyleProperty;

/// Requested raster texture filter, a constant, a zoom stop table, or a zoom expression.
#[derive(Debug, Clone)]
pub enum RasterResampling {
    /// One filter at every zoom.
    Constant(ResamplingMode),
    /// The filter of the last stop at or below the zoom, or of the first stop below them all.
    ByZoom {
        /// Stops as zoom and filter, in ascending zoom order.
        stops: Vec<(f64, ResamplingMode)>,
    },
    /// Expression array evaluating to `"linear"` or `"nearest"` at the view zoom.
    Expression(Vec<serde_json::Value>),
}

/// A raster texture filter.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResamplingMode {
    /// Interpolates neighboring texels.
    #[serde(rename = "linear")]
    Linear,
    /// Selects the nearest texel without interpolation.
    #[serde(rename = "nearest")]
    Nearest,
}

impl ResamplingMode {
    pub(crate) fn from_str(value: &str) -> Result<Self, serde_json::Error> {
        match value {
            "linear" => Ok(Self::Linear),
            "nearest" => Ok(Self::Nearest),
            _ => Err(serde::de::Error::custom(
                "resampling mode must be \"linear\" or \"nearest\"",
            )),
        }
    }
}

impl Serialize for RasterResampling {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            Self::Constant(mode) => mode.serialize(serializer),
            Self::ByZoom { stops } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("stops", stops)?;
                map.end()
            }
            Self::Expression(items) => items.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for RasterResampling {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Self::from_json_value(&value).map_err(serde::de::Error::custom)
    }
}

fn parse_stop(item: &serde_json::Value) -> Result<(f64, ResamplingMode), serde_json::Error> {
    let Some(pair) = item.as_array() else {
        return Err(serde::de::Error::custom("resampling stop must be pair"));
    };
    let [z_val, m_val] = pair.as_slice() else {
        return Err(serde::de::Error::custom("resampling stop must be pair"));
    };
    let Some(zoom) = z_val.as_f64() else {
        return Err(serde::de::Error::custom(
            "resampling stop zoom must be number",
        ));
    };
    let Some(mode_str) = m_val.as_str() else {
        return Err(serde::de::Error::custom(
            "resampling stop mode must be string",
        ));
    };
    Ok((zoom, ResamplingMode::from_str(mode_str)?))
}

fn parse_stops_object(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<RasterResampling, serde_json::Error> {
    let Some(stops_arr) = map.get("stops").and_then(|v| v.as_array()) else {
        return Err(serde::de::Error::custom("resampling object missing stops"));
    };
    let mut stops = Vec::with_capacity(stops_arr.len());
    for item in stops_arr {
        stops.push(parse_stop(item)?);
    }
    Ok(RasterResampling::ByZoom { stops })
}

impl RasterResampling {
    /// Parses `raster-resampling` / `*-resampling` directly from a JSON [`serde_json::Value`].
    pub(crate) fn from_json_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        match value {
            serde_json::Value::String(s) => Ok(Self::Constant(ResamplingMode::from_str(s)?)),
            serde_json::Value::Array(items) => Ok(Self::Expression(items.clone())),
            serde_json::Value::Object(map) => parse_stops_object(map),
            _ => Err(serde::de::Error::custom("invalid resampling property")),
        }
    }

    /// Whether texels are picked without interpolation at `zoom`.
    pub fn is_nearest_at(&self, zoom: f64) -> bool {
        match self {
            Self::Constant(mode) => *mode == ResamplingMode::Nearest,
            Self::ByZoom { stops } => stops
                .iter()
                .rev()
                .find(|(stop, _)| *stop <= zoom)
                .or_else(|| stops.first())
                .is_some_and(|(_, mode)| *mode == ResamplingMode::Nearest),
            Self::Expression(items) => {
                StyleProperty::<String>::parse(&serde_json::Value::Array(items.clone()))
                    .evaluate_at_zoom(zoom)
                    .is_some_and(|mode| mode == "nearest")
            }
        }
    }
}
