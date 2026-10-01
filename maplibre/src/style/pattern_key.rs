//! The key a feature's pattern name travels under from tessellation to the draw that binds
//! its image.

/// Number of key bits, which a float's mantissa holds exactly.
const KEY_BITS: u32 = 23;
/// The exponent of one, so a key reads back as a normal float.
const ONE: u32 = 0x3F80_0000;

/// A stable key for a pattern name; a name the style does not hold has none.
pub fn pattern_key(name: &str) -> u32 {
    let mut hash: u32 = 0x811C_9DC5;
    for byte in name.bytes() {
        hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
    }
    // Zero is kept for "no pattern".
    ((hash >> 9) & ((1 << KEY_BITS) - 1)).max(1)
}

/// The float that carries the key of `name`, or zero for a feature without a pattern.
pub fn pattern_value(name: Option<&str>) -> f32 {
    name.filter(|name| !name.is_empty())
        .map_or(0.0, |name| f32::from_bits(ONE | pattern_key(name)))
}

/// The key a float carries, if it carries one.
pub fn key_of_value(value: f32) -> Option<u32> {
    let bits = value.to_bits();
    (bits & !((1 << KEY_BITS) - 1) == ONE).then_some(bits & ((1 << KEY_BITS) - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_survives_the_trip_through_a_float() {
        let value = pattern_value(Some("generic_icon"));
        assert_eq!(key_of_value(value), Some(pattern_key("generic_icon")));
        assert_eq!(key_of_value(pattern_value(None)), None);
        assert_ne!(pattern_key("a"), pattern_key("b"));
    }
}
