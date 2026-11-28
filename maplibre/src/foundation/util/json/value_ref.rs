//! Shared borrowed [`Value`] deserializers avoiding per-type struct `visit_seq` codegen.

mod access;

use std::marker::PhantomData;

use access::{ArrSeqAccess, MapEnumAccess, ObjMapAccess, StrEnumAccess};
use serde::{
    de::{self, DeserializeOwned, DeserializeSeed, Unexpected, VariantAccess, Visitor},
    Deserializer,
};
use serde_json::Value;

/// Deserializes `T` from UTF-8 JSON bytes through a single shared [`Value`] parser.
pub fn from_json_slice<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, serde_json::Error> {
    let value: Value = serde_json::from_slice(bytes)?;
    from_json_value(&value)
}

/// Deserializes `T` from a JSON string through the shared slice parser.
pub fn from_json_str<T: DeserializeOwned>(json: &str) -> Result<T, serde_json::Error> {
    from_json_slice(json.as_bytes())
}

/// Deserializes `T` by borrowing from a parsed [`Value`], accepting maps only for structs.
pub fn from_json_value<T: DeserializeOwned>(value: &Value) -> Result<T, serde_json::Error> {
    T::deserialize(MapOnlyValueRef(value))
}

struct MapOnlyValueRef<'de>(&'de Value);

impl<'de> From<&'de Value> for MapOnlyValueRef<'de> {
    fn from(value: &'de Value) -> Self {
        Self(value)
    }
}

#[inline(never)]
fn invalid_type_err(value: &Value, expected: &'static str) -> serde_json::Error {
    de::Error::invalid_type(unexpected(value), &expected)
}

#[inline(never)]
fn expect_str<'a>(value: &'a Value, expected: &'static str) -> Result<&'a str, serde_json::Error> {
    match value {
        Value::String(text) => Ok(text.as_str()),
        other => Err(invalid_type_err(other, expected)),
    }
}

#[inline(never)]
fn expect_bool(value: &Value) -> Result<bool, serde_json::Error> {
    match value {
        Value::Bool(flag) => Ok(*flag),
        other => Err(invalid_type_err(other, "boolean")),
    }
}

#[inline(never)]
fn expect_f64(value: &Value) -> Result<f64, serde_json::Error> {
    match value {
        Value::Number(num) => num
            .as_f64()
            .ok_or_else(|| de::Error::custom("unsupported JSON float")),
        other => Err(invalid_type_err(other, "float")),
    }
}

#[inline(never)]
fn expect_i64(value: &Value) -> Result<i64, serde_json::Error> {
    match value {
        Value::Number(num) => num
            .as_i64()
            .ok_or_else(|| de::Error::custom("integer out of range")),
        other => Err(invalid_type_err(other, "integer")),
    }
}

#[inline(never)]
fn expect_u64(value: &Value) -> Result<u64, serde_json::Error> {
    match value {
        Value::Number(num) => num
            .as_u64()
            .ok_or_else(|| de::Error::custom("unsigned integer out of range")),
        other => Err(invalid_type_err(other, "unsigned integer")),
    }
}

#[inline(never)]
fn range_err(expected: &'static str) -> serde_json::Error {
    de::Error::custom(expected)
}

fn unexpected(value: &Value) -> Unexpected<'_> {
    match value {
        Value::Null => Unexpected::Unit,
        Value::Bool(flag) => Unexpected::Bool(*flag),
        Value::Number(num) => {
            if let Some(u) = num.as_u64() {
                Unexpected::Unsigned(u)
            } else if let Some(i) = num.as_i64() {
                Unexpected::Signed(i)
            } else {
                Unexpected::Float(num.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(text) => Unexpected::Str(text),
        Value::Array(_) => Unexpected::Seq,
        Value::Object(_) => Unexpected::Map,
    }
}

impl<'de> Deserializer<'de> for MapOnlyValueRef<'de> {
    type Error = serde_json::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.0 {
            Value::Null => visitor.visit_unit(),
            Value::Bool(flag) => visitor.visit_bool(*flag),
            Value::Number(num) => {
                if let Some(u) = num.as_u64() {
                    visitor.visit_u64(u)
                } else if let Some(i) = num.as_i64() {
                    visitor.visit_i64(i)
                } else if let Some(f) = num.as_f64() {
                    visitor.visit_f64(f)
                } else {
                    Err(de::Error::custom("unsupported JSON number"))
                }
            }
            Value::String(text) => visitor.visit_borrowed_str(text),
            Value::Array(items) => {
                visitor.visit_seq(ArrSeqAccess::<Self>(items.iter(), PhantomData))
            }
            Value::Object(map) => visitor.visit_map(ObjMapAccess::<Self> {
                iter: map.iter(),
                value: None,
                _marker: PhantomData,
            }),
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_bool(expect_bool(self.0)?)
    }

    fn deserialize_i8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let val = i8::try_from(expect_i64(self.0)?).map_err(|_| range_err("i8 out of range"))?;
        visitor.visit_i8(val)
    }

    fn deserialize_i16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let val = i16::try_from(expect_i64(self.0)?).map_err(|_| range_err("i16 out of range"))?;
        visitor.visit_i16(val)
    }

    fn deserialize_i32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let val = i32::try_from(expect_i64(self.0)?).map_err(|_| range_err("i32 out of range"))?;
        visitor.visit_i32(val)
    }

    fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_i64(expect_i64(self.0)?)
    }

    fn deserialize_i128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_i128(i128::from(expect_i64(self.0)?))
    }

    fn deserialize_u8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let val = u8::try_from(expect_u64(self.0)?).map_err(|_| range_err("u8 out of range"))?;
        visitor.visit_u8(val)
    }

    fn deserialize_u16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let val = u16::try_from(expect_u64(self.0)?).map_err(|_| range_err("u16 out of range"))?;
        visitor.visit_u16(val)
    }

    fn deserialize_u32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let val = u32::try_from(expect_u64(self.0)?).map_err(|_| range_err("u32 out of range"))?;
        visitor.visit_u32(val)
    }

    fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_u64(expect_u64(self.0)?)
    }

    fn deserialize_u128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_u128(u128::from(expect_u64(self.0)?))
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_f32(expect_f64(self.0)? as f32)
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_f64(expect_f64(self.0)?)
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let text = expect_str(self.0, "char")?;
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(ch), None) => visitor.visit_char(ch),
            _ => Err(invalid_type_err(self.0, "single character")),
        }
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_borrowed_str(expect_str(self.0, "string")?)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_borrowed_str(expect_str(self.0, "string")?)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.0 {
            Value::String(text) => visitor.visit_borrowed_str(text),
            Value::Array(_) => self.deserialize_seq(visitor),
            other => Err(invalid_type_err(other, "bytes")),
        }
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.0 {
            Value::Null => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.0 {
            Value::Null => visitor.visit_unit(),
            other => Err(invalid_type_err(other, "null")),
        }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.0 {
            Value::Array(items) => {
                visitor.visit_seq(ArrSeqAccess::<Self>(items.iter(), PhantomData))
            }
            other => Err(invalid_type_err(other, "sequence")),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.0 {
            Value::Object(map) => visitor.visit_map(ObjMapAccess::<Self> {
                iter: map.iter(),
                value: None,
                _marker: PhantomData,
            }),
            other => Err(invalid_type_err(other, "map")),
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        // Reject sequence syntax for named structs to omit per-struct `visit_seq` codegen.
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self.0 {
            Value::String(variant) => visitor.visit_enum(StrEnumAccess(variant)),
            Value::Object(map) if map.len() == 1 => {
                if let Some((variant, val)) = map.iter().next() {
                    visitor.visit_enum(MapEnumAccess::<Self> {
                        variant,
                        value: val,
                        _marker: PhantomData,
                    })
                } else {
                    Err(invalid_type_err(self.0, "enum"))
                }
            }
            other => Err(invalid_type_err(other, "enum")),
        }
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_borrowed_str(expect_str(self.0, "identifier")?)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
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
