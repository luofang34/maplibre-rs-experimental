//! Feature identity, Web Mercator projection, and streaming property visitors.

use std::{borrow::Cow, fmt};

use serde::{
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
    Deserialize,
};
use serde_json::Value as JsonValue;

use crate::style::expression::{FeatureProperties, Value, GEOMETRY_TYPE_PROPERTY, ID_PROPERTY};

/// Web Mercator latitude clamp in degrees (matching `geojson::index::read`).
const MAX_LATITUDE: f64 = 85.051_128_78;

/// Raw feature identifier preserved for stationary-order comparison.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::layers::vector::systems::upload_system::dynamic_points) enum RawFeatureId {
    None,
    UInt(u64),
    Other(Box<str>),
}

impl RawFeatureId {
    pub(in crate::layers::vector::systems::upload_system::dynamic_points) fn from_json(
        value: Option<&JsonValue>,
    ) -> Self {
        let Some(v) = value.filter(|v| !v.is_null()) else {
            return Self::None;
        };
        if let Some(id) = integer_id_json(v) {
            return Self::UInt(id);
        }
        match v {
            JsonValue::String(s) => Self::Other(s.clone().into_boxed_str()),
            other => Self::Other(other.to_string().into_boxed_str()),
        }
    }

    #[inline]
    pub(super) fn as_u64(&self) -> Option<u64> {
        match self {
            Self::UInt(id) => Some(*id),
            Self::None | Self::Other(_) => None,
        }
    }
}

#[inline]
fn integer_id_json(value: &JsonValue) -> Option<u64> {
    value.as_u64().or_else(|| {
        let number = value.as_f64()?;
        (number >= 0.0 && number.fract() == 0.0 && number < 9.0e15).then_some(number as u64)
    })
}

#[inline]
fn integer_id_expr(value: &Value) -> Option<u64> {
    match value {
        Value::Number(n) if *n >= 0.0 && n.fract() == 0.0 && *n < 9.0e15 => Some(*n as u64),
        _ => None,
    }
}

#[inline]
fn resolve_numeric_id(
    raw_id: &RawFeatureId,
    props: &FeatureProperties,
    promoted: Option<&str>,
    generated: Option<usize>,
) -> Option<u64> {
    if let Some(name) = promoted {
        return props.get(name).and_then(integer_id_expr);
    }
    if let Some(idx) = generated {
        return Some(idx as u64);
    }
    raw_id.as_u64()
}

#[inline]
pub(in crate::layers::vector::systems::upload_system::dynamic_points) fn project_lon_lat(
    lon: f64,
    lat: f64,
) -> Option<[f64; 2]> {
    if !lon.is_finite() || !lat.is_finite() {
        return None;
    }
    let lat_rad = lat.clamp(-MAX_LATITUDE, MAX_LATITUDE).to_radians();
    Some([
        (lon + 180.0) / 360.0,
        0.5 - (std::f64::consts::FRAC_PI_4 + lat_rad / 2.0).tan().ln()
            / (2.0 * std::f64::consts::PI),
    ])
}

pub(in crate::layers::vector::systems::upload_system::dynamic_points) fn fill_properties_from_json(
    target: &mut FeatureProperties,
    props: Option<&JsonValue>,
    raw_id: &RawFeatureId,
    promoted: Option<&str>,
    generated: Option<usize>,
    inject_identity: bool,
) {
    target.clear();
    if let Some(map) = props.and_then(JsonValue::as_object) {
        target.reserve(map.len() + usize::from(inject_identity) * 2);
        for (k, v) in map {
            if let Some(converted) = json_member_to_expr_value(v) {
                target.insert(k.clone(), converted);
            }
        }
    }
    if inject_identity {
        apply_identity_tags(target, raw_id, promoted, generated);
    }
}

fn json_member_to_expr_value(value: &JsonValue) -> Option<Value> {
    match value {
        JsonValue::Null => None,
        JsonValue::Bool(b) => Some(Value::Bool(*b)),
        JsonValue::Number(n) => n.as_f64().map(Value::Number),
        JsonValue::String(s) => Some(Value::String(s.clone())),
        other => Some(Value::String(other.to_string())),
    }
}

#[inline]
pub(super) fn apply_identity_tags(
    target: &mut FeatureProperties,
    raw_id: &RawFeatureId,
    promoted: Option<&str>,
    generated: Option<usize>,
) {
    let resolved_id = resolve_numeric_id(raw_id, target, promoted, generated);
    set_or_insert(
        target,
        GEOMETRY_TYPE_PROPERTY,
        Value::String("Point".to_owned()),
    );
    if let Some(id) = resolved_id {
        set_or_insert(target, ID_PROPERTY, Value::Number(id as f64));
    }
}

#[inline]
pub(super) fn set_or_insert(target: &mut FeatureProperties, key: &str, value: Value) {
    if let Some(slot) = target.get_mut(key) {
        *slot = value;
    } else {
        target.insert(key.to_owned(), value);
    }
}

pub(super) struct RawIdSeed;

impl<'de> DeserializeSeed<'de> for RawIdSeed {
    type Value = RawFeatureId;

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<RawFeatureId, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for RawIdSeed {
    type Value = RawFeatureId;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON feature id")
    }

    fn visit_unit<E: de::Error>(self) -> Result<RawFeatureId, E> {
        Ok(RawFeatureId::None)
    }

    fn visit_none<E: de::Error>(self) -> Result<RawFeatureId, E> {
        Ok(RawFeatureId::None)
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<RawFeatureId, E> {
        Ok(RawFeatureId::UInt(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<RawFeatureId, E> {
        if let Ok(u) = u64::try_from(v) {
            Ok(RawFeatureId::UInt(u))
        } else {
            Ok(RawFeatureId::Other(v.to_string().into_boxed_str()))
        }
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<RawFeatureId, E> {
        if v >= 0.0 && v.fract() == 0.0 && v < 9.0e15 {
            Ok(RawFeatureId::UInt(v as u64))
        } else {
            Ok(RawFeatureId::Other(v.to_string().into_boxed_str()))
        }
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<RawFeatureId, E> {
        Ok(RawFeatureId::Other(v.to_string().into_boxed_str()))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<RawFeatureId, E> {
        Ok(RawFeatureId::Other(v.into()))
    }
}

pub(super) struct PropertiesSeed<'a> {
    pub(super) target: &'a mut FeatureProperties,
}

impl<'de> DeserializeSeed<'de> for PropertiesSeed<'_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_option(self)
    }
}

impl<'de> Visitor<'de> for PropertiesSeed<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a feature properties object or null")
    }

    fn visit_none<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_some<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(PropertiesMapVisitor {
            target: self.target,
        })
    }
}

struct PropertiesMapVisitor<'a> {
    target: &'a mut FeatureProperties,
}

impl<'de> Visitor<'de> for PropertiesMapVisitor<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a feature properties object")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        while let Some(key) = map.next_key::<Cow<'de, str>>()? {
            if let Some(val) = map.next_value_seed(PropValueSeed)? {
                set_or_insert(self.target, key.as_ref(), val);
            } else if let Some(slot) = self.target.get_mut(key.as_ref()) {
                *slot = Value::Null;
            }
        }
        Ok(())
    }
}

struct PropValueSeed;

impl<'de> DeserializeSeed<'de> for PropValueSeed {
    type Value = Option<Value>;

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Option<Value>, D::Error> {
        deserializer.deserialize_any(ScalarOrNestedVisitor)
    }
}

struct ScalarOrNestedVisitor;

impl<'de> Visitor<'de> for ScalarOrNestedVisitor {
    type Value = Option<Value>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON property value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Option<Value>, E> {
        Ok(None)
    }

    fn visit_none<E: de::Error>(self) -> Result<Option<Value>, E> {
        Ok(None)
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Option<Value>, E> {
        Ok(Some(Value::Bool(v)))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Option<Value>, E> {
        Ok(Some(Value::Number(v as f64)))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Option<Value>, E> {
        Ok(Some(Value::Number(v as f64)))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Option<Value>, E> {
        Ok(Some(Value::Number(v)))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Option<Value>, E> {
        Ok(Some(Value::String(v.to_owned())))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Option<Value>, E> {
        Ok(Some(Value::String(v)))
    }

    fn visit_seq<S: SeqAccess<'de>>(self, seq: S) -> Result<Option<Value>, S::Error> {
        let v = JsonValue::deserialize(de::value::SeqAccessDeserializer::new(seq))?;
        Ok(Some(Value::String(v.to_string())))
    }

    fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Option<Value>, M::Error> {
        let v = JsonValue::deserialize(de::value::MapAccessDeserializer::new(map))?;
        Ok(Some(Value::String(v.to_string())))
    }
}
