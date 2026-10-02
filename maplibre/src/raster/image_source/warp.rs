//! How an image source's picture is warped onto its corners, as GL JS `calculateImageWarp` and
//! its raster vertex shader warp it.
//!
//! The warp maps the unit square of the picture onto the corners: projectively, as the view of a
//! plane, while the corners plausibly describe one, blending towards a bilinear rubber sheet as
//! they stop doing so. GL JS draws it as a mesh whose vertices it places with the warp and whose
//! texture coordinates it interpolates with the warp's homogeneous weight: two triangles where
//! that is exact, a parallelogram or a purely projective warp, and a grid of cells otherwise.
//! The corners are first rounded to the integer units of the tile the picture is centred in, as
//! GL JS rounds them, which decides whether a near-rectangle counts as a parallelogram.

/// Cells per side of the grid a warp that is neither affine nor projective is drawn with.
const SUBDIVIDED_QUAD_GRANULARITY: usize = 16;

/// The foreshortening up to which a warp stays purely projective.
const PROJECTIVE_FORESHORTENING: f64 = 4.0;

/// The foreshortening at which a warp is taken to be purely bilinear.
const BILINEAR_FORESHORTENING: f64 = 512.0;

/// Units per side of a tile, in which GL JS rounds the corners.
const EXTENT: f64 = 8192.0;

/// One vertex of the warped mesh: its Mercator position and its texture coordinates multiplied by
/// its homogeneous weight, with the weight.
pub(super) type Vertex = ([f64; 2], [f64; 3]);

/// The perspective terms and blend of a warp, GL JS's `RasterImageWarp`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Terms {
    perspective: [f64; 2],
    blend: f64,
}

const BILINEAR: Terms = Terms {
    perspective: [0.0, 0.0],
    blend: 1.0,
};

/// The picture's corners, top left, top right, bottom right and bottom left, in Mercator units
/// rounded as GL JS rounds them to the tile they are centred in.
fn rounded_corners(corners: [[f64; 2]; 4]) -> [[f64; 2]; 4] {
    let fold = |axis: usize| {
        corners.iter().fold((f64::MAX, f64::MIN), |(lo, hi), c| {
            (lo.min(c[axis]), hi.max(c[axis]))
        })
    };
    let ((west, east), (north, south)) = (fold(0), fold(1));
    let largest = (east - west).max(south - north);
    let zoom = (-largest.log2()).floor().max(0.0);
    let tiles = 2_f64.powf(zoom);
    let tile = [
        ((west + east) / 2.0 * tiles).floor(),
        ((north + south) / 2.0 * tiles).floor(),
    ];
    corners.map(|[x, y]| {
        let units = [
            ((x * tiles - tile[0]) * EXTENT).round(),
            ((y * tiles - tile[1]) * EXTENT).round(),
        ];
        [
            (units[0] / EXTENT + tile[0]) / tiles,
            (units[1] / EXTENT + tile[1]) / tiles,
        ]
    })
}

fn is_parallelogram([top_left, top_right, bottom_right, bottom_left]: [[f64; 2]; 4]) -> bool {
    top_left[0] + bottom_right[0] == top_right[0] + bottom_left[0]
        && top_left[1] + bottom_right[1] == top_right[1] + bottom_left[1]
}

/// GL JS `calculateImageWarp` with its default `auto` warp.
fn terms(corners: [[f64; 2]; 4]) -> Terms {
    if is_parallelogram(corners) {
        return BILINEAR;
    }
    let [top_left, top_right, bottom_right, bottom_left] = corners;
    let sum = [
        top_left[0] - top_right[0] + bottom_right[0] - bottom_left[0],
        top_left[1] - top_right[1] + bottom_right[1] - bottom_left[1],
    ];
    let right = [
        top_right[0] - bottom_right[0],
        top_right[1] - bottom_right[1],
    ];
    let down = [
        bottom_left[0] - bottom_right[0],
        bottom_left[1] - bottom_right[1],
    ];
    let determinant = right[0] * down[1] - down[0] * right[1];
    let perspective = [
        (sum[0] * down[1] - down[0] * sum[1]) / determinant,
        (right[0] * sum[1] - sum[0] * right[1]) / determinant,
    ];
    let denominators = [
        1.0,
        1.0 + perspective[0],
        1.0 + perspective[0] + perspective[1],
        1.0 + perspective[1],
    ];
    let foreshortening = denominators.iter().copied().fold(f64::MIN, f64::max)
        / denominators.iter().copied().fold(f64::MAX, f64::min);
    let blend = ((1.0 - PROJECTIVE_FORESHORTENING / foreshortening)
        / (1.0 - PROJECTIVE_FORESHORTENING / BILINEAR_FORESHORTENING))
        .max(0.0);
    // A degenerate determinant reaches here as an infinite or not-a-number spread.
    if !(1.0..=BILINEAR_FORESHORTENING).contains(&foreshortening) || blend >= 1.0 {
        return BILINEAR;
    }
    Terms { perspective, blend }
}

/// The vertex at `[s, t]` of the picture's unit square, as the raster vertex shader places it.
fn vertex(corners: [[f64; 2]; 4], terms: Terms, [s, t]: [f64; 2]) -> Vertex {
    let [top_left, top_right, bottom_right, bottom_left] = corners;
    let mix = |a: f64, b: f64, by: f64| a + (b - a) * by;
    let bilinear = [0, 1].map(|axis| {
        mix(
            mix(top_left[axis], top_right[axis], s),
            mix(bottom_left[axis], bottom_right[axis], s),
            t,
        )
    });
    let [px, py] = terms.perspective;
    let denominator = px * s + py * t + 1.0;
    let projective = [0, 1].map(|axis| {
        let across_top = top_right[axis] - top_left[axis] + px * top_right[axis];
        let down_left = bottom_left[axis] - top_left[axis] + py * bottom_left[axis];
        (across_top * s + down_left * t + top_left[axis]) / denominator
    });
    let position = [0, 1].map(|axis| mix(projective[axis], bilinear[axis], terms.blend));
    let weight = mix(1.0 / denominator, 1.0, terms.blend);
    (position, [s * weight, t * weight, weight])
}

/// The triangles the picture is drawn with, each split along its cell's top right to bottom
/// left diagonal as GL JS's tile mesh splits it.
pub(super) fn triangles(corners: [[f64; 2]; 4]) -> Vec<[Vertex; 3]> {
    let corners = rounded_corners(corners);
    let terms = terms(corners);
    let cells = if terms.blend > 0.0 && !is_parallelogram(corners) {
        SUBDIVIDED_QUAD_GRANULARITY
    } else {
        1
    };
    let at = |x: usize, y: usize| {
        vertex(
            corners,
            terms,
            [x as f64 / cells as f64, y as f64 / cells as f64],
        )
    };
    let mut triangles = Vec::with_capacity(cells * cells * 2);
    for y in 0..cells {
        for x in 0..cells {
            let (v0, v1, v2, v3) = (at(x, y), at(x + 1, y), at(x, y + 1), at(x + 1, y + 1));
            triangles.push([v0, v2, v1]);
            triangles.push([v1, v2, v3]);
        }
    }
    triangles
}

#[cfg(test)]
mod tests;
