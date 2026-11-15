//! Zero-DOM-allocation streaming GeoJSON parser for stationary `Point` `FeatureCollection`s.

mod values;

use std::{borrow::Cow, fmt};

use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use values::{apply_identity_tags, PropertiesSeed, RawIdSeed};
pub(super) use values::{fill_properties_from_json, project_lon_lat, RawFeatureId};

use crate::style::expression::{FeatureProperties, Value};

/// Streams `bytes` as a GeoJSON `FeatureCollection` of stationary `Point`s matching
/// `expected_keys`, updating `scratch_props` in-place without allocating `serde_json::Value`.
pub(super) fn try_parse_stationary_bytes(
    bytes: &[u8],
    expected_keys: &[(RawFeatureId, [u64; 2])],
    scratch_props: &mut [FeatureProperties],
    promoted: Option<&str>,
    generate_id: bool,
    inject_identity: bool,
) -> bool {
    if expected_keys.is_empty() || expected_keys.len() != scratch_props.len() {
        return false;
    }
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let seed = CollectionSeed {
        expected_keys,
        scratch_props,
        promoted,
        generate_id,
        inject_identity,
    };
    seed.deserialize(&mut de).is_ok() && de.end().is_ok()
}

struct CollectionSeed<'a> {
    expected_keys: &'a [(RawFeatureId, [u64; 2])],
    scratch_props: &'a mut [FeatureProperties],
    promoted: Option<&'a str>,
    generate_id: bool,
    inject_identity: bool,
}

impl<'de> DeserializeSeed<'de> for CollectionSeed<'_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for CollectionSeed<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a stationary Point FeatureCollection")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        let mut is_fc = false;
        let mut parsed_features: Option<usize> = None;
        let mut scratch_opt = Some(&mut *self.scratch_props);
        while let Some(key) = map.next_key::<Cow<'de, str>>()? {
            match key.as_ref() {
                "type" => {
                    let kind: Cow<'de, str> = map.next_value()?;
                    if kind.as_ref() != "FeatureCollection" {
                        return Err(de::Error::custom("not FeatureCollection"));
                    }
                    is_fc = true;
                }
                "features" => {
                    let Some(scratch) = scratch_opt.take() else {
                        return Err(de::Error::custom("duplicate features"));
                    };
                    let count = map.next_value_seed(FeaturesSeqSeed {
                        expected_keys: self.expected_keys,
                        scratch_props: scratch,
                        promoted: self.promoted,
                        generate_id: self.generate_id,
                        inject_identity: self.inject_identity,
                    })?;
                    parsed_features = Some(count);
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        if !is_fc || parsed_features != Some(self.expected_keys.len()) {
            return Err(de::Error::custom("collection mismatch"));
        }
        Ok(())
    }
}

struct FeaturesSeqSeed<'a> {
    expected_keys: &'a [(RawFeatureId, [u64; 2])],
    scratch_props: &'a mut [FeatureProperties],
    promoted: Option<&'a str>,
    generate_id: bool,
    inject_identity: bool,
}

impl<'de> DeserializeSeed<'de> for FeaturesSeqSeed<'_> {
    type Value = usize;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<usize, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for FeaturesSeqSeed<'_> {
    type Value = usize;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a sequence of stationary Point features")
    }

    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<usize, S::Error> {
        let mut index = 0_usize;
        for (expected, target_props) in self.expected_keys.iter().zip(self.scratch_props.iter_mut())
        {
            let ok = seq.next_element_seed(FeatureSeed {
                expected_id: &expected.0,
                expected_coord_bits: expected.1,
                target_props,
                promoted: self.promoted,
                generated: self.generate_id.then_some(index),
                inject_identity: self.inject_identity,
            })?;
            if ok.is_none() {
                return Err(de::Error::custom("fewer features than expected"));
            }
            index += 1;
        }
        if seq.next_element::<IgnoredAny>()?.is_some() {
            return Err(de::Error::custom("more features than expected"));
        }
        Ok(index)
    }
}

struct FeatureSeed<'a> {
    expected_id: &'a RawFeatureId,
    expected_coord_bits: [u64; 2],
    target_props: &'a mut FeatureProperties,
    promoted: Option<&'a str>,
    generated: Option<usize>,
    inject_identity: bool,
}

impl<'de> DeserializeSeed<'de> for FeatureSeed<'_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for FeatureSeed<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a GeoJSON Point Feature")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        let mut is_feature = false;
        let mut geom_ok = false;
        let mut raw_id = RawFeatureId::None;
        for val in self.target_props.values_mut() {
            *val = Value::Null;
        }
        let mut props_opt = Some(&mut *self.target_props);

        while let Some(key) = map.next_key::<Cow<'de, str>>()? {
            match key.as_ref() {
                "type" => {
                    let kind: Cow<'de, str> = map.next_value()?;
                    if kind.as_ref() != "Feature" {
                        return Err(de::Error::custom("not Feature"));
                    }
                    is_feature = true;
                }
                "id" => {
                    raw_id = map.next_value_seed(RawIdSeed)?;
                }
                "geometry" => {
                    let bits = map.next_value_seed(PointGeometrySeed)?;
                    if bits != self.expected_coord_bits {
                        return Err(de::Error::custom("point moved"));
                    }
                    geom_ok = true;
                }
                "properties" => {
                    let Some(target) = props_opt.take() else {
                        return Err(de::Error::custom("duplicate properties"));
                    };
                    map.next_value_seed(PropertiesSeed { target })?;
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }

        if !is_feature || !geom_ok || &raw_id != self.expected_id {
            return Err(de::Error::custom("feature mismatch"));
        }
        if self.inject_identity {
            apply_identity_tags(self.target_props, &raw_id, self.promoted, self.generated);
        }
        if self.target_props.values().any(|v| matches!(v, Value::Null)) {
            self.target_props.retain(|_, v| !matches!(v, Value::Null));
        }
        Ok(())
    }
}

struct PointGeometrySeed;

impl<'de> DeserializeSeed<'de> for PointGeometrySeed {
    type Value = [u64; 2];

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<[u64; 2], D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for PointGeometrySeed {
    type Value = [u64; 2];

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a GeoJSON Point geometry")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<[u64; 2], M::Error> {
        let mut is_point = false;
        let mut coords: Option<[u64; 2]> = None;
        while let Some(key) = map.next_key::<Cow<'de, str>>()? {
            match key.as_ref() {
                "type" => {
                    let kind: Cow<'de, str> = map.next_value()?;
                    if kind.as_ref() != "Point" {
                        return Err(de::Error::custom("not Point"));
                    }
                    is_point = true;
                }
                "coordinates" => {
                    coords = Some(map.next_value_seed(PointCoordsSeed)?);
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        match (is_point, coords) {
            (true, Some(bits)) => Ok(bits),
            _ => Err(de::Error::custom("invalid Point geometry")),
        }
    }
}

struct PointCoordsSeed;

impl<'de> DeserializeSeed<'de> for PointCoordsSeed {
    type Value = [u64; 2];

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<[u64; 2], D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for PointCoordsSeed {
    type Value = [u64; 2];

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[lon, lat] Point coordinates")
    }

    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<[u64; 2], S::Error> {
        let lon: f64 = seq
            .next_element()?
            .ok_or_else(|| de::Error::custom("missing lon"))?;
        let lat: f64 = seq
            .next_element()?
            .ok_or_else(|| de::Error::custom("missing lat"))?;
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        if !lon.is_finite() || !lat.is_finite() {
            return Err(de::Error::custom("non-finite coordinate"));
        }
        Ok([lon.to_bits(), lat.to_bits()])
    }
}
