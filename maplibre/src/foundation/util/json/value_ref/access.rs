use std::marker::PhantomData;

use serde::{
    de::{
        self, DeserializeSeed, EnumAccess, IntoDeserializer, MapAccess, SeqAccess, Unexpected,
        VariantAccess, Visitor,
    },
    Deserializer,
};
use serde_json::Value;

pub(super) struct ArrSeqAccess<'de, D>(
    pub(super) std::slice::Iter<'de, Value>,
    pub(super) PhantomData<fn(&'de Value) -> D>,
);

impl<'de, D> SeqAccess<'de> for ArrSeqAccess<'de, D>
where
    D: From<&'de Value> + Deserializer<'de, Error = serde_json::Error>,
{
    type Error = serde_json::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Self::Error> {
        match self.0.next() {
            Some(val) => seed.deserialize(D::from(val)).map(Some),
            None => Ok(None),
        }
    }
}

pub(super) struct ObjMapAccess<'de, D> {
    pub(super) iter: serde_json::map::Iter<'de>,
    pub(super) value: Option<&'de Value>,
    pub(super) _marker: PhantomData<fn(&'de Value) -> D>,
}

impl<'de, D> MapAccess<'de> for ObjMapAccess<'de, D>
where
    D: From<&'de Value> + Deserializer<'de, Error = serde_json::Error>,
{
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
        seed.deserialize(D::from(val))
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

pub(super) struct MapEnumAccess<'de, D> {
    pub(super) variant: &'de str,
    pub(super) value: &'de Value,
    pub(super) _marker: PhantomData<fn(&'de Value) -> D>,
}

impl<'de, D> EnumAccess<'de> for MapEnumAccess<'de, D>
where
    D: From<&'de Value> + VariantAccess<'de, Error = serde_json::Error>,
{
    type Error = serde_json::Error;
    type Variant = D;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), Self::Error> {
        let variant = seed.deserialize(self.variant.into_deserializer())?;
        Ok((variant, D::from(self.value)))
    }
}
