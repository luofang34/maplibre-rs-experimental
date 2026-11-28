//! Unhides stepped `building` podium bases whose `building:part` pieces all start above ground.

use std::borrow::Cow;

use geozero::mvt::tile;

struct BaseCandidate {
    feature_index: usize,
    bbox: [i32; 4],
    rings: Vec<Vec<[i32; 2]>>,
}

struct PartInfo {
    bbox: [i32; 4],
    centroids: Vec<[f32; 2]>,
    min_height: f64,
}

/// Clears `hide_3d` on building footprints whose interior `building:part` pieces all start
/// above ground (`render_min_height > 0`), so stepped skyscrapers modeled with a `building`
/// base podium and elevated `building:part` tiers do not float in mid-air.
///
/// Operates on `&mut Cow<'_, tile::Layer>` so tiles without elevated stepped podiums avoid
/// cloning tens of thousands of `building` features.
pub(super) fn unhide_unreplaced_building_bases(layer: &mut Cow<'_, tile::Layer>) {
    let Some(hide_3d_key) = find_layer_key(layer, "hide_3d") else {
        return;
    };
    let Some(min_height_key) = find_layer_key(layer, "render_min_height") else {
        return;
    };
    let height_key = find_layer_key(layer, "render_height");
    let (candidates, Some(union_bbox)) =
        collect_base_candidates(layer, hide_3d_key, min_height_key)
    else {
        return;
    };
    let parts = collect_part_infos(layer, hide_3d_key, min_height_key, union_bbox);
    if parts.is_empty() {
        return;
    }
    apply_unhide_updates(layer, &candidates, &parts, hide_3d_key, height_key);
}

fn find_layer_key(layer: &tile::Layer, name: &str) -> Option<u32> {
    layer
        .keys
        .iter()
        .position(|key| key == name)
        .and_then(|idx| u32::try_from(idx).ok())
}

fn collect_base_candidates(
    layer: &tile::Layer,
    hide_3d_key: u32,
    min_height_key: u32,
) -> (Vec<BaseCandidate>, Option<[i32; 4]>) {
    let mut candidates = Vec::new();
    let mut union_bbox: Option<[i32; 4]> = None;
    for (feature_index, feature) in layer.features.iter().enumerate() {
        if feature.r#type != Some(3)
            || feature_bool_tag(feature, hide_3d_key, &layer.values) != Some(true)
            || feature_number_tag(feature, min_height_key, &layer.values).unwrap_or(0.0) > 0.0
        {
            continue;
        }
        let rings = decode_mvt_rings(&feature.geometry);
        let Some(bbox) = rings_bbox(&rings) else {
            continue;
        };
        union_bbox = Some(match union_bbox {
            None => bbox,
            Some(u) => [
                u[0].min(bbox[0]),
                u[1].min(bbox[1]),
                u[2].max(bbox[2]),
                u[3].max(bbox[3]),
            ],
        });
        candidates.push(BaseCandidate {
            feature_index,
            bbox,
            rings,
        });
    }
    (candidates, union_bbox)
}

fn collect_part_infos(
    layer: &tile::Layer,
    hide_3d_key: u32,
    min_height_key: u32,
    union_bbox: [i32; 4],
) -> Vec<PartInfo> {
    let mut parts = Vec::new();
    for feature in &layer.features {
        if feature.r#type != Some(3)
            || feature_bool_tag(feature, hide_3d_key, &layer.values) == Some(true)
        {
            continue;
        }
        let Some(bbox) = geometry_bbox(&feature.geometry) else {
            continue;
        };
        if bbox[2] < union_bbox[0]
            || bbox[0] > union_bbox[2]
            || bbox[3] < union_bbox[1]
            || bbox[1] > union_bbox[3]
        {
            continue;
        }
        let rings = decode_mvt_rings(&feature.geometry);
        let centroids: Vec<[f32; 2]> = rings
            .iter()
            .filter_map(|ring| {
                if ring.is_empty() {
                    return None;
                }
                let n = ring.len() as f32;
                let sum = ring.iter().fold([0.0_f32, 0.0_f32], |acc, pt| {
                    [acc[0] + pt[0] as f32, acc[1] + pt[1] as f32]
                });
                Some([sum[0] / n, sum[1] / n])
            })
            .collect();
        let min_height = feature_number_tag(feature, min_height_key, &layer.values).unwrap_or(0.0);
        parts.push(PartInfo {
            bbox,
            centroids,
            min_height,
        });
    }
    parts
}

fn apply_unhide_updates(
    layer: &mut Cow<'_, tile::Layer>,
    candidates: &[BaseCandidate],
    parts: &[PartInfo],
    hide_3d_key: u32,
    height_key: Option<u32>,
) {
    for candidate in candidates {
        let bbox = candidate.bbox;
        let mut min_part_base: Option<f64> = None;
        for part in parts {
            if part.bbox[2] < bbox[0]
                || part.bbox[0] > bbox[2]
                || part.bbox[3] < bbox[1]
                || part.bbox[1] > bbox[3]
            {
                continue;
            }
            let inside = part.centroids.iter().any(|&point| {
                candidate
                    .rings
                    .iter()
                    .filter(|ring| point_in_mvt_ring(ring, point))
                    .count()
                    % 2
                    == 1
            });
            if inside {
                min_part_base =
                    Some(min_part_base.map_or(part.min_height, |cur| cur.min(part.min_height)));
            }
        }
        let Some(min_base) = min_part_base.filter(|&base| base > 0.0) else {
            continue;
        };
        let owned = layer.to_mut();
        let Some(feat) = owned.features.get_mut(candidate.feature_index) else {
            continue;
        };
        remove_feature_tag(&mut feat.tags, hide_3d_key);
        if let Some(height_key) = height_key {
            if let Some(height) = feature_number_tag(feat, height_key, &owned.values) {
                if height > min_base {
                    let val_idx = u32::try_from(owned.values.len()).unwrap_or(u32::MAX);
                    owned.values.push(tile::Value {
                        double_value: Some(min_base),
                        ..Default::default()
                    });
                    if let Some(feat) = owned.features.get_mut(candidate.feature_index) {
                        set_feature_tag(&mut feat.tags, height_key, val_idx);
                    }
                }
            }
        }
    }
}

fn feature_bool_tag(feature: &tile::Feature, key: u32, values: &[tile::Value]) -> Option<bool> {
    for &[k, v] in feature.tags.as_chunks::<2>().0 {
        if k == key {
            return values.get(v as usize)?.bool_value;
        }
    }
    None
}

fn feature_number_tag(feature: &tile::Feature, key: u32, values: &[tile::Value]) -> Option<f64> {
    for &[k, v] in feature.tags.as_chunks::<2>().0 {
        if k == key {
            let v = values.get(v as usize)?;
            return v
                .double_value
                .or_else(|| v.float_value.map(f64::from))
                .or_else(|| v.int_value.map(|n| n as f64))
                .or_else(|| v.uint_value.map(|n| n as f64))
                .or_else(|| v.sint_value.map(|n| n as f64));
        }
    }
    None
}

fn remove_feature_tag(tags: &mut Vec<u32>, key: u32) {
    let mut kept = Vec::with_capacity(tags.len());
    for pair in tags.as_chunks::<2>().0 {
        if pair[0] != key {
            kept.extend_from_slice(pair);
        }
    }
    *tags = kept;
}

fn set_feature_tag(tags: &mut [u32], key: u32, value_index: u32) {
    for pair in tags.as_chunks_mut::<2>().0 {
        if pair[0] == key {
            pair[1] = value_index;
            return;
        }
    }
}

fn geometry_bbox(geometry: &[u32]) -> Option<[i32; 4]> {
    let (mut cx, mut cy) = (0_i32, 0_i32);
    let mut iter = geometry.iter().copied();
    let mut has_ring = false;
    let mut bbox: Option<[i32; 4]> = None;
    while let Some(cmd_int) = iter.next() {
        let (cmd, count) = (cmd_int & 7, (cmd_int >> 3) as usize);
        if cmd == 1 || cmd == 2 {
            for _ in 0..count {
                let (Some(raw_dx), Some(raw_dy)) = (iter.next(), iter.next()) else {
                    return bbox;
                };
                let dx = ((raw_dx >> 1) as i32) ^ -((raw_dx & 1) as i32);
                let dy = ((raw_dy >> 1) as i32) ^ -((raw_dy & 1) as i32);
                cx = cx.saturating_add(dx);
                cy = cy.saturating_add(dy);
                if cmd == 1 {
                    has_ring = true;
                }
                if has_ring {
                    bbox = Some(match bbox {
                        None => [cx, cy, cx, cy],
                        Some(b) => [b[0].min(cx), b[1].min(cy), b[2].max(cx), b[3].max(cy)],
                    });
                }
            }
        }
    }
    bbox
}

fn decode_mvt_rings(geometry: &[u32]) -> Vec<Vec<[i32; 2]>> {
    let mut rings: Vec<Vec<[i32; 2]>> = Vec::new();
    let (mut cx, mut cy) = (0_i32, 0_i32);
    let mut iter = geometry.iter().copied();
    while let Some(cmd_int) = iter.next() {
        let (cmd, count) = (cmd_int & 7, (cmd_int >> 3) as usize);
        if cmd == 1 || cmd == 2 {
            for _ in 0..count {
                let (Some(raw_dx), Some(raw_dy)) = (iter.next(), iter.next()) else {
                    return rings;
                };
                let dx = ((raw_dx >> 1) as i32) ^ -((raw_dx & 1) as i32);
                let dy = ((raw_dy >> 1) as i32) ^ -((raw_dy & 1) as i32);
                cx = cx.saturating_add(dx);
                cy = cy.saturating_add(dy);
                if cmd == 1 {
                    rings.push(vec![[cx, cy]]);
                } else if let Some(ring) = rings.last_mut() {
                    ring.push([cx, cy]);
                }
            }
        }
    }
    rings
}

fn rings_bbox(rings: &[Vec<[i32; 2]>]) -> Option<[i32; 4]> {
    let mut iter = rings.iter().flatten();
    let &[x0, y0] = iter.next()?;
    let mut bbox = [x0, y0, x0, y0];
    for &[x, y] in iter {
        bbox[0] = bbox[0].min(x);
        bbox[1] = bbox[1].min(y);
        bbox[2] = bbox[2].max(x);
        bbox[3] = bbox[3].max(y);
    }
    Some(bbox)
}

fn point_in_mvt_ring(ring: &[[i32; 2]], [x, y]: [f32; 2]) -> bool {
    let mut inside = false;
    for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
        let (ax, ay) = (a[0] as f32, a[1] as f32);
        let (bx, by) = (b[0] as f32, b[1] as f32);
        if (ay > y) != (by > y) && x < (bx - ax) * (y - ay) / (by - ay) + ax {
            inside = !inside;
        }
    }
    inside
}
