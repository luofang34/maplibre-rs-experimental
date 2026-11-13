//! Unhides stepped `building` podium bases whose `building:part` pieces all start above ground.

use geozero::mvt::tile;

/// Clears `hide_3d` on building footprints whose interior `building:part` pieces all start
/// above ground (`render_min_height > 0`), so stepped skyscrapers modeled with a `building`
/// base podium and elevated `building:part` tiers do not float in mid-air.
pub(super) fn unhide_unreplaced_building_bases(layer: &mut tile::Layer) {
    let Some(hide_3d_key) = layer
        .keys
        .iter()
        .position(|key| key == "hide_3d")
        .and_then(|idx| u32::try_from(idx).ok())
    else {
        return;
    };
    let Some(min_height_key) = layer
        .keys
        .iter()
        .position(|key| key == "render_min_height")
        .and_then(|idx| u32::try_from(idx).ok())
    else {
        return;
    };
    let height_key = layer
        .keys
        .iter()
        .position(|key| key == "render_height")
        .and_then(|idx| u32::try_from(idx).ok());

    struct PartInfo {
        bbox: [i32; 4],
        centroids: Vec<[f32; 2]>,
        min_height: f64,
    }

    let mut parts = Vec::new();
    let mut has_hide_3d = false;
    for feature in &layer.features {
        if feature.r#type != Some(3) {
            continue;
        }
        if feature_bool_tag(feature, hide_3d_key, &layer.values) == Some(true) {
            has_hide_3d = true;
            continue;
        }
        let rings = decode_mvt_rings(&feature.geometry);
        let Some(bbox) = rings_bbox(&rings) else {
            continue;
        };
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
    if !has_hide_3d || parts.is_empty() {
        return;
    }

    for feature_index in 0..layer.features.len() {
        let feature = &layer.features[feature_index];
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
        let mut min_part_base: Option<f64> = None;
        for part in &parts {
            if part.bbox[2] < bbox[0]
                || part.bbox[0] > bbox[2]
                || part.bbox[3] < bbox[1]
                || part.bbox[1] > bbox[3]
            {
                continue;
            }
            let inside = part.centroids.iter().any(|&point| {
                rings
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
        remove_feature_tag(&mut layer.features[feature_index].tags, hide_3d_key);
        if let Some(height_key) = height_key {
            if let Some(height) =
                feature_number_tag(&layer.features[feature_index], height_key, &layer.values)
            {
                if height > min_base {
                    let val_idx = u32::try_from(layer.values.len()).unwrap_or(u32::MAX);
                    layer.values.push(tile::Value {
                        double_value: Some(min_base),
                        ..Default::default()
                    });
                    set_feature_tag(&mut layer.features[feature_index].tags, height_key, val_idx);
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

fn decode_mvt_rings(geometry: &[u32]) -> Vec<Vec<[i32; 2]>> {
    let mut rings: Vec<Vec<[i32; 2]>> = Vec::new();
    let (mut cx, mut cy) = (0_i32, 0_i32);
    let mut idx = 0;
    while idx < geometry.len() {
        let cmd_int = geometry[idx];
        idx += 1;
        let (cmd, count) = (cmd_int & 7, (cmd_int >> 3) as usize);
        if cmd == 1 || cmd == 2 {
            for _ in 0..count {
                if idx + 1 >= geometry.len() {
                    return rings;
                }
                let raw_dx = geometry[idx];
                let raw_dy = geometry[idx + 1];
                idx += 2;
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
