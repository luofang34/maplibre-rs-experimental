//! Where text sits from its anchor: `text-offset`, `text-radial-offset` and the per-anchor
//! offsets of `text-variable-anchor` and `text-variable-anchor-offset`, in layout pixels.
use super::layout::{anchor_fractions, offset};
use crate::style::{expression::FeatureProperties, layer::SymbolPaint};

/// The glyphs appear to start at the baseline, which the text box edge sits above by this much.
const BASELINE: f32 = 7.0;

/// The anchors a label may take, best first: the entries of `text-variable-anchor-offset`, or
/// `text-variable-anchor`; empty for a label with one anchor.
pub(super) fn variable_anchors(paint: &SymbolPaint) -> Vec<String> {
    if let Some(pairs) = anchor_offsets(paint) {
        return pairs.into_iter().map(|(anchor, _)| anchor).collect();
    }
    paint
        .properties
        .get("text-variable-anchor")
        .and_then(|value| value.as_array())
        .map(|anchors| {
            anchors
                .iter()
                .filter_map(|anchor| anchor.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// `text-variable-anchor-offset` as written: each anchor with its offset in ems.
fn anchor_offsets(paint: &SymbolPaint) -> Option<Vec<(String, [f32; 2])>> {
    let items = paint
        .properties
        .get("text-variable-anchor-offset")?
        .as_array()?;
    let pairs = items
        .as_chunks::<2>()
        .0
        .iter()
        .filter_map(|pair| {
            let anchor = pair[0].as_str()?.to_owned();
            let offset = pair[1].as_array()?;
            let read = |index: usize| offset.get(index).and_then(serde_json::Value::as_f64);
            Some((anchor, [read(0)? as f32, read(1)? as f32]))
        })
        .collect::<Vec<_>>();
    (!pairs.is_empty()).then_some(pairs)
}

fn from_radial(anchor: &str, radial: f32) -> [f32; 2] {
    let radial = radial.max(0.0);
    let diagonal = radial / std::f32::consts::SQRT_2;
    let y = match anchor {
        "top-right" | "top-left" => diagonal - BASELINE,
        "bottom-right" | "bottom-left" => -diagonal + BASELINE,
        "bottom" => -radial + BASELINE,
        "top" => radial - BASELINE,
        _ => 0.0,
    };
    let x = match anchor {
        "top-right" | "bottom-right" => -diagonal,
        "top-left" | "bottom-left" => diagonal,
        "left" => radial,
        "right" => -radial,
        _ => 0.0,
    };
    [x, y]
}

fn from_text_offset(anchor: &str, [x, y]: [f32; 2]) -> [f32; 2] {
    let (x, y) = (x.abs(), y.abs());
    let y = match anchor {
        "top-right" | "top-left" | "top" => y - BASELINE,
        "bottom-right" | "bottom-left" | "bottom" => -y + BASELINE,
        _ => 0.0,
    };
    let x = match anchor {
        "top-right" | "bottom-right" | "right" => -x,
        "top-left" | "bottom-left" | "left" => x,
        _ => 0.0,
    };
    [x, y]
}

/// The text offset in layout pixels for an anchor. A label with one anchor applies
/// `text-offset` as written, or `text-radial-offset` along the anchor's direction when one is
/// set; a label with variable anchors turns either into the offset each anchor needs.
pub(super) fn anchored_offset(
    paint: &SymbolPaint,
    variable: &[String],
    anchor: &str,
    (properties, zoom): (&FeatureProperties, f64),
) -> [f32; 2] {
    let context = (properties, zoom);
    let base = if let Some(pairs) = anchor_offsets(paint) {
        let [x, y] = pairs
            .iter()
            .find(|(name, _)| name == anchor)
            .map_or([0.0; 2], |(_, offset)| offset.map(|em| em * 24.0));
        let shift = if anchor.starts_with("top") {
            -BASELINE
        } else if anchor.starts_with("bottom") {
            BASELINE
        } else {
            0.0
        };
        [x, y + shift]
    } else if variable.is_empty() {
        let radial = paint.number("text-radial-offset", properties, zoom, 0.0);
        if radial != 0.0 {
            from_radial(anchor, radial * 24.0)
        } else {
            offset(paint, "text-offset", 24.0, context)
        }
    } else if paint.properties.contains_key("text-radial-offset") {
        from_radial(
            anchor,
            paint.number("text-radial-offset", properties, zoom, 0.0) * 24.0,
        )
    } else {
        from_text_offset(anchor, offset(paint, "text-offset", 24.0, context))
    };
    if variable.is_empty() {
        return base;
    }
    // A label with variable anchors sits at its anchor by the box the collision test uses,
    // which `text-padding` grows on every side, so the text keeps that gap from the point.
    let size = paint
        .text_size
        .as_ref()
        .and_then(|value| value.evaluate_for(properties, zoom))
        .unwrap_or(16.0);
    let gap = paint.number("text-padding", properties, zoom, 2.0) * 24.0 / size.max(1.0);
    let fractions = anchor_fractions(anchor);
    [
        base[0] - (fractions[0] - 0.5) * 2.0 * gap,
        base[1] - (fractions[1] - 0.5) * 2.0 * gap,
    ]
}
