use serde::{
    de::{
        self, DeserializeSeed, EnumAccess, IntoDeserializer, MapAccess, SeqAccess, Unexpected,
        VariantAccess, Visitor,
    },
    Deserializer,
};
use serde_json::Value;

use super::MapOnlyValueRef;

pub(super) struct ArrSeqAccess<'de>(pub(super) std::slice::Iter<'de, Value>);

impl<'de> SeqAccess<'de> for ArrSeqAccess<'de> {
    type Error = serde_json::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Self::Error> {
        match self.0.next() {
            Some(val) => seed.deserialize(MapOnlyValueRef(val)).map(Some),
            None => Ok(None),
        }
    }
}

pub(super) struct ObjMapAccess<'de> {
    pub(super) iter: serde_json::map::Iter<'de>,
    pub(super) value: Option<&'de Value>,
}

impl<'de> MapAccess<'de> for ObjMapAccess<'de> {
    type Error = serde_json::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Self::Error> {
        match self.iter.next() {
            Some((k, v)) => {
                self.value = Some(v);
                seed.deserialize(k.as_str().into_deserializer()).map(Some)
            }
            None => Ok(None),
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, Self::Error> {
        let val = self
            .value
            .take()
            .ok_or_else(|| de::Error::custom("missing map value"))?;
        seed.deserialize(MapOnlyValueRef(val))
    }
}

pub(super) struct StrEnumAccess<'de>(pub(super) &'de str);

impl<'de> EnumAccess<'de> for StrEnumAccess<'de> {
    type Error = serde_json::Error;
    type Variant = UnitVariantAccess;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), Self::Error> {
        let variant = seed.deserialize(self.0.into_deserializer())?;
        Ok((variant, UnitVariantAccess))
    }
}

pub(super) struct UnitVariantAccess;

impl<'de> VariantAccess<'de> for UnitVariantAccess {
    type Error = serde_json::Error;

    fn unit_variant(self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(
        self,
        _seed: T,
    ) -> Result<T::Value, Self::Error> {
        Err(de::Error::invalid_type(
            Unexpected::UnitVariant,
            &"newtype variant",
        ))
    }

    fn tuple_variant<V: Visitor<'de>>(
        self,
        _len: usize,
        _visitor: V,
    ) -> Result<V::Value, Self::Error> {
        Err(de::Error::invalid_type(
            Unexpected::UnitVariant,
            &"tuple variant",
        ))
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error> {
        Err(de::Error::invalid_type(
            Unexpected::UnitVariant,
            &"struct variant",
        ))
    }
}

pub(super) struct MapEnumAccess<'de> {
    pub(super) variant: &'de str,
    pub(super) value: &'de Value,
}

impl<'de> EnumAccess<'de> for MapEnumAccess<'de> {
    type Error = serde_json::Error;
    type Variant = MapOnlyValueRef<'de>;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), Self::Error> {
        let variant = seed.deserialize(self.variant.into_deserializer())?;
        Ok((variant, MapOnlyValueRef(self.value)))
    }
}

impl<'de> VariantAccess<'de> for MapOnlyValueRef<'de> {
    type Error = serde_json::Error;

    fn unit_variant(self) -> Result<(), Self::Error> {
        serde::Deserialize::deserialize(self)
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(
        self,
        seed: T,
    ) -> Result<T::Value, Self::Error> {
        seed.deserialize(self)
    }

    fn tuple_variant<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_struct("", fields, visitor)
    }
}
