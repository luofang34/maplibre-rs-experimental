//! Selective MVT property decoder that skips unreferenced `tile::Value` string decoding and
//! retains only referenced `(key, value)` slots when style layers use fixed attribute names.

use geozero::mvt::{tile, Message as _};
use rustc_hash::FxHashSet;

use super::{read_varint, scan_mvt_layer_name, skip_wire_value, strip_zero_padding};
use crate::{
    coords::WorldTileCoords,
    io::vector_tile::rescale::{into_descendant_layer, DescendantFrame},
};

/// Scans raw MVT protobuf wire tags (`Tile.layers = 3`, `Layer.name = 1`) and decodes only layers
/// accepted by `layer_keys`:
/// - `None`: skip the layer completely;
/// - `Some(None)`: decode the full layer including all properties;
/// - `Some(Some(keys))`: decode geometries and only the MVT keys present in `keys`.
///
/// When `rescale` is `Some((from, to))`, each decoded feature is clipped into `to`'s descendant
/// extent before its referenced properties are materialized. Returns `None` when `data` is empty,
/// non-MVT (e.g. MLT), or fails wire validation so the caller can fall back cleanly.
pub(crate) fn try_decode_mvt_filtered_with_keys_rescaled<'a>(
    data: &[u8],
    rescale: Option<(WorldTileCoords, WorldTileCoords)>,
    mut layer_keys: impl FnMut(&str) -> Option<Option<&'a FxHashSet<String>>>,
) -> Option<geozero::mvt::Tile> {
    let bytes = strip_zero_padding(data);
    if bytes.is_empty() {
        return None;
    }
    let mut pos = 0;
    let mut saw_mvt_layer = false;
    let mut layers = Vec::new();
    while pos < bytes.len() {
        let tag = read_varint(bytes, &mut pos)?;
        let field_num = tag >> 3;
        let wire_type = tag & 0x07;
        if field_num == 3 && wire_type == 2 {
            let len = usize::try_from(read_varint(bytes, &mut pos)?).ok()?;
            let slice_end = pos.checked_add(len)?;
            let layer_bytes = bytes.get(pos..slice_end)?;
            pos = slice_end;
            let name = scan_mvt_layer_name(layer_bytes)?;
            saw_mvt_layer = true;
            if let Some(needed) = layer_keys(name) {
                let layer = match needed {
                    Some(keys) => decode_layer_pruned(layer_bytes, keys, rescale)?,
                    None => {
                        let mut layer = tile::Layer::decode(layer_bytes).ok()?;
                        if let Some((from, to)) = rescale {
                            let _changed = into_descendant_layer(&mut layer, from, to);
                        }
                        layer
                    }
                };
                layers.push(layer);
            }
        } else if field_num >= 16 {
            skip_wire_value(bytes, &mut pos, wire_type)?;
        } else {
            return None;
        }
    }
    saw_mvt_layer.then_some(geozero::mvt::Tile { layers })
}

#[cfg(test)]
pub(crate) fn try_decode_mvt_filtered_with_keys<'a>(
    data: &[u8],
    layer_keys: impl FnMut(&str) -> Option<Option<&'a FxHashSet<String>>>,
) -> Option<geozero::mvt::Tile> {
    try_decode_mvt_filtered_with_keys_rescaled(data, None, layer_keys)
}

fn read_bytes<'a>(bytes: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
    let len = usize::try_from(read_varint(bytes, pos)?).ok()?;
    let end = pos.checked_add(len)?;
    let slice = bytes.get(*pos..end)?;
    *pos = end;
    Some(slice)
}

fn scan_mvt_layer_extent(layer_bytes: &[u8]) -> Option<u32> {
    let mut pos = 0;
    let mut extent = None;
    while pos < layer_bytes.len() {
        let tag = read_varint(layer_bytes, &mut pos)?;
        let field_num = tag >> 3;
        let wire_type = tag & 0x07;
        if field_num == 0 {
            return None;
        }
        if field_num == 5 && wire_type == 0 {
            extent = Some(u32::try_from(read_varint(layer_bytes, &mut pos)?).ok()?);
        } else {
            skip_wire_value(layer_bytes, &mut pos, wire_type)?;
        }
    }
    Some(extent.unwrap_or(4096))
}

struct PrunedLayerParts {
    name: String,
    version: u32,
    extent: Option<u32>,
    features: Vec<tile::Feature>,
    compact_keys: Vec<String>,
    key_remap: Vec<u32>,
    value_spans: Vec<(usize, usize)>,
}

fn decode_layer_pruned(
    layer_bytes: &[u8],
    needed: &FxHashSet<String>,
    rescale: Option<(WorldTileCoords, WorldTileCoords)>,
) -> Option<tile::Layer> {
    let mut parts = parse_pruned_layer_parts(layer_bytes, needed, rescale)?;
    let (compact_values, val_remap) = compact_layer_tags_and_values(layer_bytes, &mut parts)?;
    for feature in &mut parts.features {
        for [_, v_slot] in feature.tags.as_chunks_mut::<2>().0 {
            *v_slot = *val_remap.get(*v_slot as usize)?;
        }
    }
    Some(tile::Layer {
        version: parts.version,
        name: parts.name,
        features: parts.features,
        keys: parts.compact_keys,
        values: compact_values,
        extent: parts.extent,
    })
}

fn parse_pruned_layer_parts(
    layer_bytes: &[u8],
    needed: &FxHashSet<String>,
    rescale: Option<(WorldTileCoords, WorldTileCoords)>,
) -> Option<PrunedLayerParts> {
    let frame = match rescale {
        Some((from, to)) => {
            let layer_extent = scan_mvt_layer_extent(layer_bytes)?;
            DescendantFrame::for_extent(from, to, layer_extent)
        }
        None => None,
    };
    let mut name: Option<String> = None;
    let mut version: Option<u32> = None;
    let mut extent: Option<u32> = None;
    let mut features: Vec<tile::Feature> = Vec::new();
    let mut compact_keys: Vec<String> = Vec::with_capacity(needed.len());
    let mut key_remap: Vec<u32> = Vec::new();
    let mut value_spans: Vec<(usize, usize)> = Vec::new();

    let mut pos = 0_usize;
    while pos < layer_bytes.len() {
        let tag = read_varint(layer_bytes, &mut pos)?;
        let field_num = tag >> 3;
        let wire_type = tag & 0x07;
        match (field_num, wire_type) {
            (1, 2) => {
                let slice = read_bytes(layer_bytes, &mut pos)?;
                name = Some(std::str::from_utf8(slice).ok()?.to_owned());
            }
            (2, 2) => {
                let slice = read_bytes(layer_bytes, &mut pos)?;
                let mut feature = tile::Feature::decode(slice).ok()?;
                if !feature.tags.len().is_multiple_of(2) {
                    return None;
                }
                if let Some(ref frame) = frame {
                    if !frame.cut(&mut feature) {
                        continue;
                    }
                }
                features.push(feature);
            }
            (3, 2) => {
                let slice = read_bytes(layer_bytes, &mut pos)?;
                let key = std::str::from_utf8(slice).ok()?;
                if needed.contains(key) {
                    let mapped = u32::try_from(compact_keys.len()).ok()?;
                    compact_keys.push(key.to_owned());
                    key_remap.push(mapped);
                } else {
                    key_remap.push(u32::MAX);
                }
            }
            (4, 2) => {
                let len = usize::try_from(read_varint(layer_bytes, &mut pos)?).ok()?;
                let start = pos;
                let end = pos.checked_add(len)?;
                if end > layer_bytes.len() {
                    return None;
                }
                pos = end;
                value_spans.push((start, end));
            }
            (5, 0) => extent = Some(u32::try_from(read_varint(layer_bytes, &mut pos)?).ok()?),
            (15, 0) => version = Some(u32::try_from(read_varint(layer_bytes, &mut pos)?).ok()?),
            (1 | 2 | 3 | 4 | 5 | 15, _) | (0, _) => return None,
            _ => skip_wire_value(layer_bytes, &mut pos, wire_type)?,
        }
    }
    Some(PrunedLayerParts {
        name: name?,
        version: version?,
        extent,
        features,
        compact_keys,
        key_remap,
        value_spans,
    })
}

fn compact_layer_tags_and_values(
    layer_bytes: &[u8],
    parts: &mut PrunedLayerParts,
) -> Option<(Vec<tile::Value>, Vec<u32>)> {
    let mut val_remap = vec![u32::MAX; parts.value_spans.len()];
    for feature in &mut parts.features {
        if !feature.tags.len().is_multiple_of(2) {
            return None;
        }
        let mut kept = Vec::with_capacity(feature.tags.len());
        for &[raw_k, orig_v_idx] in feature.tags.as_chunks::<2>().0 {
            let &mapped_k = parts.key_remap.get(raw_k as usize)?;
            let used_slot = val_remap.get_mut(orig_v_idx as usize)?;
            if mapped_k != u32::MAX {
                *used_slot = 0;
                kept.push(mapped_k);
                kept.push(orig_v_idx);
            }
        }
        feature.tags = kept;
    }

    let mut compact_values = Vec::new();
    for (remap, &(start, end)) in val_remap.iter_mut().zip(&parts.value_spans) {
        if *remap == 0 {
            *remap = u32::try_from(compact_values.len()).ok()?;
            let raw = layer_bytes.get(start..end)?;
            compact_values.push(tile::Value::decode(raw).ok()?);
        }
    }
    Some((compact_values, val_remap))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_test_tile() -> Vec<u8> {
        let place = tile::Layer {
            version: 2,
            name: "place".to_owned(),
            extent: Some(4096),
            keys: vec![
                "class".to_owned(),
                "name:en".to_owned(),
                "name:fr".to_owned(),
                "name:ja".to_owned(),
            ],
            values: vec![
                tile::Value {
                    string_value: Some("city".to_owned()),
                    ..Default::default()
                },
                tile::Value {
                    string_value: Some("Paris".to_owned()),
                    ..Default::default()
                },
                tile::Value {
                    string_value: Some("Paris FR".to_owned()),
                    ..Default::default()
                },
                tile::Value {
                    string_value: Some("パリ".to_owned()),
                    ..Default::default()
                },
            ],
            features: vec![tile::Feature {
                id: Some(7),
                tags: vec![0, 0, 1, 1, 2, 2, 3, 3],
                r#type: Some(1),
                geometry: vec![9, 100, 100],
            }],
        };
        let unused = tile::Layer {
            version: 2,
            name: "water".to_owned(),
            extent: Some(4096),
            keys: vec!["kind".to_owned()],
            values: vec![tile::Value {
                string_value: Some("ocean".to_owned()),
                ..Default::default()
            }],
            features: vec![],
        };
        geozero::mvt::Tile {
            layers: vec![place, unused],
        }
        .encode_to_vec()
    }

    #[test]
    fn selective_decode_prunes_unused_keys_and_values() {
        let bytes = encode_test_tile();
        let mut needed = FxHashSet::default();
        needed.insert("name:en".to_owned());

        let decoded = try_decode_mvt_filtered_with_keys(&bytes, |name| match name {
            "place" => Some(Some(&needed)),
            _ => None,
        })
        .expect("selective decode succeeds");

        assert_eq!(decoded.layers.len(), 1);
        let layer = &decoded.layers[0];
        assert_eq!(layer.name, "place");
        assert_eq!(layer.keys, vec!["name:en"]);
        assert_eq!(layer.values.len(), 1);
        assert_eq!(layer.values[0].string_value.as_deref(), Some("Paris"));
        assert_eq!(layer.features[0].tags, vec![0, 0]);
    }

    #[test]
    fn selective_decode_none_keyset_retains_all_properties() {
        let bytes = encode_test_tile();
        let decoded = try_decode_mvt_filtered_with_keys(&bytes, |name| match name {
            "place" => Some(None),
            _ => None,
        })
        .expect("full layer decode succeeds");

        assert_eq!(decoded.layers.len(), 1);
        assert_eq!(decoded.layers[0].keys.len(), 4);
        assert_eq!(decoded.layers[0].values.len(), 4);
    }

    #[test]
    fn odd_tags_fall_back_to_none() {
        let bad = geozero::mvt::Tile {
            layers: vec![tile::Layer {
                version: 2,
                name: "place".to_owned(),
                keys: vec!["k".to_owned()],
                values: vec![tile::Value::default()],
                features: vec![tile::Feature {
                    tags: vec![0],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
        .encode_to_vec();
        let needed = FxHashSet::default();
        assert!(try_decode_mvt_filtered_with_keys(&bad, |_| Some(Some(&needed))).is_none());
    }

    #[test]
    fn selective_rescaled_decode_matches_full_decode_then_into_descendant() {
        use crate::{coords::ZoomLevel, io::vector_tile::rescale::into_descendant};
        let mut inside_city = tile::Feature {
            id: Some(1),
            tags: vec![0, 0, 1, 1, 2, 2],
            r#type: Some(1),
            geometry: vec![9, 1000, 1000],
        };
        // Wait: MVT zigzag encoding: unsigned n -> (n >> 1) ^ -(n & 1). (500 << 1) = 1000 -> (500, 500) in NW quadrant (0..2048).
        inside_city.geometry = vec![9, 1000, 1000];
        let outside_point = tile::Feature {
            id: Some(2),
            tags: vec![0, 3, 1, 4, 2, 5],
            r#type: Some(1),
            // (3500, 3500) -> zigzag (7000, 7000), strictly SE quadrant, clipped out of NW descendant.
            geometry: vec![9, 7000, 7000],
        };
        let crossing_line = tile::Feature {
            id: Some(3),
            tags: vec![0, 3, 1, 1],
            r#type: Some(2),
            // MoveTo(0, 500) -> LineTo(+3000, 0) in zigzag: (9, 0, 1000, 10, 6000, 0)
            geometry: vec![9, 0, 1000, 10, 6000, 0],
        };
        let bytes = geozero::mvt::Tile {
            layers: vec![tile::Layer {
                version: 2,
                name: "place".to_owned(),
                features: vec![inside_city, outside_point, crossing_line],
                keys: vec![
                    "class".to_owned(),
                    "name:en".to_owned(),
                    "name:ja".to_owned(),
                ],
                values: vec![
                    tile::Value {
                        string_value: Some("city".to_owned()),
                        ..Default::default()
                    },
                    tile::Value {
                        string_value: Some("In".to_owned()),
                        ..Default::default()
                    },
                    tile::Value {
                        string_value: Some("日本語".to_owned()),
                        ..Default::default()
                    },
                    tile::Value {
                        string_value: Some("town".to_owned()),
                        ..Default::default()
                    },
                    tile::Value {
                        string_value: Some("Out".to_owned()),
                        ..Default::default()
                    },
                    tile::Value {
                        string_value: Some("外".to_owned()),
                        ..Default::default()
                    },
                ],
                // Encode extent after features to exercise pre-scanning field 5.
                extent: Some(4096),
            }],
        }
        .encode_to_vec();

        let from = WorldTileCoords {
            x: 0,
            y: 0,
            z: ZoomLevel::new(0),
        };
        let to = WorldTileCoords {
            x: 0,
            y: 0,
            z: ZoomLevel::new(1),
        };
        let mut needed = FxHashSet::default();
        needed.insert("name:en".to_owned());

        let actual = try_decode_mvt_filtered_with_keys_rescaled(&bytes, Some((from, to)), |name| {
            (name == "place").then_some(Some(&needed))
        })
        .expect("rescaled decode succeeds");

        let mut expected = try_decode_mvt_filtered_with_keys(&bytes, |name| {
            (name == "place").then_some(Some(&needed))
        })
        .expect("unrescaled decode succeeds");
        into_descendant(&mut expected, from, to);

        // `expected` retained "Out" in `values` because key pruning ran before clipping;
        // verify retained features and their resolved property pairs match identically.
        assert_eq!(actual.layers.len(), 1);
        assert_eq!(actual.layers[0].features.len(), 2);
        assert_eq!(
            actual.layers[0].features[0].id,
            expected.layers[0].features[0].id
        );
        assert_eq!(
            actual.layers[0].features[0].geometry,
            expected.layers[0].features[0].geometry
        );
        assert_eq!(
            actual.layers[0].features[1].id,
            expected.layers[0].features[1].id
        );
        assert_eq!(
            actual.layers[0].features[1].geometry,
            expected.layers[0].features[1].geometry
        );
        // Outside-only value ("Out") must not be decoded/stored in `actual.values`.
        assert_eq!(actual.layers[0].values.len(), 1);
        assert_eq!(
            actual.layers[0].values[0].string_value.as_deref(),
            Some("In")
        );
    }
}
