//! Compact binary wire encoding of [`BucketFeatureEntry`] slices for single-threaded worker
//! transport.

use std::sync::Arc;

use rustc_hash::FxHashMap;

use super::{BucketEntryKind, BucketFeatureEntry};
use crate::{
    io::geometry_index::wire::{decode_properties_from, encode_properties, read_u32, read_u8},
    style::expression::FeatureProperties,
};

const KIND_FEATURE: u8 = 0;
const KIND_LINE: u8 = 1;
const KIND_FILL_OUTLINE: u8 = 2;

/// Serializes `entries` into a flat byte buffer, deduplicating shared `Arc<FeatureProperties>`
/// allocations across entries.
pub fn encode_feature_paint(entries: &[BucketFeatureEntry]) -> Vec<u8> {
    let mut prop_blobs = Vec::new();
    let mut prop_map: FxHashMap<usize, u32> = FxHashMap::default();
    let mut prop_count = 0_u32;

    let mut entry_bytes = Vec::with_capacity(entries.len() * 8);
    for entry in entries {
        let ptr = Arc::as_ptr(&entry.properties) as usize;
        let group_idx = *prop_map.entry(ptr).or_insert_with(|| {
            let idx = prop_count;
            prop_count += 1;
            encode_properties(&entry.properties, &mut prop_blobs);
            idx
        });
        entry_bytes.extend_from_slice(&group_idx.to_le_bytes());
        let kind_bits = match entry.kind {
            BucketEntryKind::Feature => KIND_FEATURE,
            BucketEntryKind::Line { .. } => KIND_LINE,
            BucketEntryKind::FillOutline => KIND_FILL_OUTLINE,
        };
        let flags = (u8::from(entry.raw_id.is_some())) | (kind_bits << 1);
        entry_bytes.push(flags);
        if let Some(id) = entry.raw_id {
            entry_bytes.extend_from_slice(&id.to_le_bytes());
        }
        if let BucketEntryKind::Line { length } = entry.kind {
            entry_bytes.extend_from_slice(&length.to_le_bytes());
        }
    }

    let mut out = Vec::with_capacity(8 + prop_blobs.len() + entry_bytes.len());
    out.extend_from_slice(&prop_count.to_le_bytes());
    out.extend_from_slice(&prop_blobs);
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend_from_slice(&entry_bytes);
    out
}

/// Reconstructs [`BucketFeatureEntry`] records from bytes produced by [`encode_feature_paint`],
/// preserving shared `Arc<FeatureProperties>` identity across entries.
pub fn decode_feature_paint(mut buf: &[u8]) -> Option<Vec<BucketFeatureEntry>> {
    let prop_count = read_u32(&mut buf)? as usize;
    let mut groups: Vec<Arc<FeatureProperties>> = Vec::with_capacity(prop_count.min(4096));
    for _ in 0..prop_count {
        groups.push(Arc::new(decode_properties_from(&mut buf)?));
    }
    let entry_count = read_u32(&mut buf)? as usize;
    let mut entries = Vec::with_capacity(entry_count.min(16_384));
    for _ in 0..entry_count {
        let group_idx = read_u32(&mut buf)? as usize;
        let properties = Arc::clone(groups.get(group_idx)?);
        let flags = read_u8(&mut buf)?;
        let raw_id = if flags & 1 != 0 {
            if buf.len() < 8 {
                return None;
            }
            let (head, rest) = buf.split_at(8);
            buf = rest;
            Some(u64::from_le_bytes(head.try_into().ok()?))
        } else {
            None
        };
        let kind = match flags >> 1 {
            KIND_FEATURE => BucketEntryKind::Feature,
            KIND_LINE => {
                if buf.len() < 4 {
                    return None;
                }
                let (head, rest) = buf.split_at(4);
                buf = rest;
                BucketEntryKind::Line {
                    length: f32::from_le_bytes(head.try_into().ok()?),
                }
            }
            KIND_FILL_OUTLINE => BucketEntryKind::FillOutline,
            _ => return None,
        };
        entries.push(BucketFeatureEntry {
            raw_id,
            properties,
            kind,
        });
    }
    Some(entries)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{decode_feature_paint, encode_feature_paint};
    use crate::{
        style::expression::{FeatureProperties, Value},
        vector::feature_paint::{BucketEntryKind, BucketFeatureEntry},
    };

    #[test]
    fn wire_round_trips_entries_and_preserves_shared_arcs() {
        let mut props_a = FeatureProperties::new();
        props_a.insert("name".into(), Value::String("A".into()));
        props_a.insert("\u{1}state:hover".into(), Value::Bool(true));
        let shared = Arc::new(props_a);

        let mut props_b = FeatureProperties::new();
        props_b.insert("rank".into(), Value::Number(3.5));
        let single = Arc::new(props_b);

        let input = vec![
            BucketFeatureEntry {
                raw_id: Some(42),
                properties: Arc::clone(&shared),
                kind: BucketEntryKind::Feature,
            },
            BucketFeatureEntry {
                raw_id: Some(42),
                properties: Arc::clone(&shared),
                kind: BucketEntryKind::FillOutline,
            },
            BucketFeatureEntry {
                raw_id: None,
                properties: single,
                kind: BucketEntryKind::Line { length: 128.75 },
            },
        ];

        let bytes = encode_feature_paint(&input);
        let decoded = decode_feature_paint(&bytes).expect("valid wire payload");
        assert_eq!(decoded.len(), 3);
        assert!(Arc::ptr_eq(&decoded[0].properties, &decoded[1].properties));
        assert!(!Arc::ptr_eq(&decoded[0].properties, &decoded[2].properties));
        assert_eq!(decoded[0].raw_id, Some(42));
        assert_eq!(decoded[0].kind, BucketEntryKind::Feature);
        assert_eq!(decoded[1].kind, BucketEntryKind::FillOutline);
        assert_eq!(decoded[2].raw_id, None);
        assert_eq!(decoded[2].kind, BucketEntryKind::Line { length: 128.75 });
        assert_eq!(decoded[0].properties, input[0].properties);
        assert_eq!(decoded[2].properties, input[2].properties);
    }
}
