//! Worker transport for symbol collision geometry and atlases.

use crate::{io::geometry_index::wire::read_u32, sdf::Feature};

mod codec;
#[cfg(any(test, not(target_arch = "wasm32")))]
mod json_legacy;
#[cfg(any(test, not(target_arch = "wasm32")))]
pub use json_legacy::{LineLabelWire, SymbolFeature};

const SYM_WIRE_MAGIC_V1: [u8; 4] = *b"SYM1";
const SYM_WIRE_MAGIC_V2: [u8; 4] = *b"SYM2";
const SYM_WIRE_MAGIC_V3: [u8; 4] = *b"SYM3";

/// Encodes symbol placement features in compact binary format, deduplicating shared polylines,
/// repeat keys, and `Arc<FeatureProperties>` maps without intermediate JSON allocations.
pub fn encode_symbol_features(features: &[Feature]) -> Vec<u8> {
    let tables = codec::collect_shared_tables(features);
    let mut out = Vec::with_capacity(32 + features.len() * 80);
    out.extend_from_slice(&SYM_WIRE_MAGIC_V3);
    codec::write_shared_tables(&tables, &mut out);
    out.extend_from_slice(&(features.len() as u32).to_le_bytes());
    for feature in features {
        codec::encode_feature(feature, &tables, &mut out);
    }
    out
}

/// Decodes symbol placement features from [`encode_symbol_features`] (falling back to JSON for
/// legacy payloads), sharing `Arc` polylines, repeat keys, and property maps across labels.
pub fn decode_symbol_features(bytes: &[u8]) -> Option<Vec<Feature>> {
    let is_v3 = bytes.starts_with(&SYM_WIRE_MAGIC_V3);
    let is_v2 = is_v3 || bytes.starts_with(&SYM_WIRE_MAGIC_V2);
    if !is_v2 && !bytes.starts_with(&SYM_WIRE_MAGIC_V1) {
        #[cfg(any(test, not(target_arch = "wasm32")))]
        return crate::util::json::from_json_slice::<Vec<SymbolFeature>>(bytes)
            .ok()
            .map(|items| items.into_iter().map(Feature::from).collect());
        #[cfg(all(target_arch = "wasm32", not(test)))]
        return None;
    }
    let mut buf = bytes.get(4..)?;
    let tables = codec::decode_tables(&mut buf, is_v2)?;
    let feat_count = read_u32(&mut buf)? as usize;
    let mut features = Vec::with_capacity(feat_count.min(16_384));
    for _ in 0..feat_count {
        features.push(codec::decode_feature(&mut buf, &tables, is_v2, is_v3)?);
    }
    Some(features)
}

#[cfg(test)]
mod tests;
