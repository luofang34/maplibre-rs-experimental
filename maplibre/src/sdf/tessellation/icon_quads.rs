//! The quads of an icon, split along the stretch zones of its sprite when `icon-text-fit`
//! fits it to text, as GL JS `getIconQuads` does.

use crate::sdf::assets::{AtlasEntry, IconStretch, TextFit};

/// A piece of an icon: its corners as `[left, top, right, bottom]` in layout pixels and the
/// atlas rectangle it shows.
pub(super) struct IconQuad {
    pub bounds: [f32; 4],
    pub rect: [u32; 4],
}

/// One cut through the image: pixels that keep their size before it, and stretchable
/// pixels before it.
#[derive(Clone, Copy)]
struct Cut {
    fixed: f32,
    stretch: f32,
}

fn total(ranges: &[[f32; 2]]) -> f32 {
    ranges.iter().map(|range| range[1] - range[0]).sum()
}

/// How much of the ranges lies between `min` and `max`.
fn within(ranges: &[[f32; 2]], min: f32, max: f32) -> f32 {
    ranges
        .iter()
        .map(|range| range[1].clamp(min, max) - range[0].clamp(min, max))
        .sum()
}

fn cuts(zones: &[[f32; 2]], fixed_size: f32, stretch_size: f32) -> Vec<Cut> {
    let mut cuts = vec![Cut {
        fixed: 0.0,
        stretch: 0.0,
    }];
    for [start, end] in zones {
        let last = cuts[cuts.len() - 1];
        cuts.push(Cut {
            fixed: start - last.stretch,
            stretch: last.stretch,
        });
        cuts.push(Cut {
            fixed: start - last.stretch,
            stretch: last.stretch + (end - start),
        });
    }
    cuts.push(Cut {
        fixed: fixed_size,
        stretch: stretch_size,
    });
    cuts
}

/// The icon box after `textFitWidth` and `textFitHeight` have pulled it towards the aspect
/// ratio of the content area.
fn apply_text_fit(icon: [f32; 4], stretch: &IconStretch, content: [f32; 4]) -> [f32; 4] {
    let [mut left, mut top, right, bottom] = icon;
    let (mut width, mut height) = (right - left, bottom - top);
    let aspect = (content[2] - content[0]) / (content[3] - content[1]);
    let fit_width = stretch.text_fit_width.unwrap_or(TextFit::StretchOrShrink);
    let fit_height = stretch.text_fit_height.unwrap_or(TextFit::StretchOrShrink);
    if fit_height == TextFit::Proportional {
        if (fit_width == TextFit::StretchOnly && width / height < aspect)
            || fit_width == TextFit::Proportional
        {
            let wider = (height * aspect).ceil();
            left *= wider / width;
            width = wider;
        }
    } else if fit_width == TextFit::Proportional
        && fit_height == TextFit::StretchOnly
        && aspect != 0.0
        && width / height > aspect
    {
        let taller = (width / aspect).ceil();
        top *= taller / height;
        height = taller;
    }
    [left, top, left + width, top + height]
}

/// The pieces of `icon` for a box `[left, top, right, bottom]` in layout pixels. Without
/// `fitted` the icon is one piece. With it, the stretch zones grow to the box and the rest
/// keeps its size, and a content area stands for the box itself.
pub(super) fn icon_quads(icon: &AtlasEntry, boxed: [f32; 4], fitted: bool) -> Vec<IconQuad> {
    let ratio = icon.metrics[3];
    let [x, y, width, height] = icon.rect;
    let (image_width, image_height) = (width as f32, height as f32);
    let Some(stretch) = icon.stretch.as_deref().filter(|_| fitted) else {
        return vec![IconQuad {
            bounds: boxed,
            rect: icon.rect,
        }];
    };
    let stretch_x = if stretch.stretch_x.is_empty() {
        vec![[0.0, image_width]]
    } else {
        stretch.stretch_x.clone()
    };
    let stretch_y = if stretch.stretch_y.is_empty() {
        vec![[0.0, image_height]]
    } else {
        stretch.stretch_y.clone()
    };
    let (stretch_width, stretch_height) = (total(&stretch_x), total(&stretch_y));
    let (fixed_width, fixed_height) = (image_width - stretch_width, image_height - stretch_height);

    let mut boxed = boxed;
    let mut offsets = ([0.0, 0.0], [stretch_width, stretch_height]);
    let mut fixed_offsets = ([0.0, 0.0], [fixed_width, fixed_height]);
    if let Some(content) = stretch.content {
        if stretch.text_fit_width.is_some() || stretch.text_fit_height.is_some() {
            boxed = apply_text_fit(boxed, stretch, content);
        }
        let stretch_before = [
            within(&stretch_x, 0.0, content[0]),
            within(&stretch_y, 0.0, content[1]),
        ];
        let stretch_inside = [
            within(&stretch_x, content[0], content[2]),
            within(&stretch_y, content[1], content[3]),
        ];
        offsets = (stretch_before, stretch_inside);
        fixed_offsets = (
            [
                content[0] - stretch_before[0],
                content[1] - stretch_before[1],
            ],
            [
                content[2] - content[0] - stretch_inside[0],
                content[3] - content[1] - stretch_inside[1],
            ],
        );
    }
    let [left, top, right, bottom] = boxed;
    let (icon_width, icon_height) = (right - left, bottom - top);

    // A position along an axis: the stretched share of the box plus the fixed pixels.
    let place = |cut: Cut, axis: usize| {
        let (origin, span) = if axis == 0 {
            (left, icon_width)
        } else {
            (top, icon_height)
        };
        let stretch_total = if axis == 0 {
            stretch_width
        } else {
            stretch_height
        };
        let em = (cut.stretch - offsets.0[axis]) / offsets.1[axis] * span + origin;
        let px = (cut.fixed - fixed_offsets.0[axis])
            - fixed_offsets.1[axis] * cut.stretch / stretch_total;
        em + px / ratio
    };
    let x_cuts = cuts(&stretch_x, fixed_width, stretch_width);
    let y_cuts = cuts(&stretch_y, fixed_height, stretch_height);
    let mut quads = Vec::new();
    for columns in x_cuts.windows(2) {
        for rows in y_cuts.windows(2) {
            let (x1, x2) = (
                columns[0].stretch + columns[0].fixed,
                columns[1].stretch + columns[1].fixed,
            );
            let (y1, y2) = (
                rows[0].stretch + rows[0].fixed,
                rows[1].stretch + rows[1].fixed,
            );
            if x2 <= x1 || y2 <= y1 {
                continue;
            }
            quads.push(IconQuad {
                bounds: [
                    place(columns[0], 0),
                    place(rows[0], 1),
                    place(columns[1], 0),
                    place(rows[1], 1),
                ],
                rect: [
                    x + x1 as u32,
                    y + y1 as u32,
                    (x2 - x1) as u32,
                    (y2 - y1) as u32,
                ],
            });
        }
    }
    quads
}

#[cfg(test)]
mod tests;
