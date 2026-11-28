//! Screen-space collision lookup bounded by the viewport.

#[derive(Default)]
pub(in crate::layers::sdf) struct CollisionGrid {
    cells: Vec<Vec<u32>>,
    cols: usize,
    rows: usize,
    boxes: Vec<[f64; 4]>,
    /// Whether each box belongs to a symbol that lets other cooperative symbols overlap it.
    cooperative: Vec<bool>,
    limit: [i32; 2],
}

pub(in crate::layers::sdf) fn intersects(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] < b[2] && a[2] > b[0] && a[1] < b[3] && a[3] > b[1]
}

fn grid_dims(width: f64, height: f64) -> ([i32; 2], usize, usize) {
    let limit = [
        (width / 64.0).ceil().clamp(0.0, 255.0) as i32,
        (height / 64.0).ceil().clamp(0.0, 255.0) as i32,
    ];
    let cols = usize::try_from(limit[0] + 2).unwrap_or(0);
    let rows = usize::try_from(limit[1] + 2).unwrap_or(0);
    (limit, cols, rows)
}

impl CollisionGrid {
    pub fn new(width: f64, height: f64) -> Self {
        let (limit, cols, rows) = grid_dims(width, height);
        Self {
            cells: vec![Vec::new(); cols * rows],
            cols,
            rows,
            boxes: Vec::new(),
            cooperative: Vec::new(),
            limit,
        }
    }

    /// Clears placed boxes while keeping existing cell/box allocations for epoch reuse.
    pub fn reset(&mut self, width: f64, height: f64) {
        let (limit, cols, rows) = grid_dims(width, height);
        self.limit = limit;
        self.cols = cols;
        self.rows = rows;
        let needed = cols * rows;
        for bucket in self.cells.iter_mut().take(needed) {
            bucket.clear();
        }
        self.cells.resize_with(needed, Vec::new);
        if self.cells.capacity() > needed.saturating_mul(2).max(4096) {
            self.cells.shrink_to(needed);
        }
        self.boxes.clear();
        self.cooperative.clear();
    }

    #[inline]
    fn cell_bounds(&self, [l, t, r, b]: [f64; 4]) -> Option<(usize, usize, usize, usize)> {
        if self.cols == 0 || self.rows == 0 {
            return None;
        }
        let clamp_x = |coord: f64| {
            ((coord / 64.0).floor().clamp(-1.0, f64::from(self.limit[0])) as i32 + 1) as usize
        };
        let clamp_y = |coord: f64| {
            ((coord / 64.0).floor().clamp(-1.0, f64::from(self.limit[1])) as i32 + 1) as usize
        };
        Some((clamp_x(l), clamp_y(t), clamp_x(r), clamp_y(b)))
    }

    pub fn overlaps(&self, rect: [f64; 4]) -> bool {
        self.overlaps_where(rect, |_| true)
    }

    /// Whether `rect` meets a box that is not cooperative, which is all a cooperative symbol
    /// collides with.
    pub fn overlaps_non_cooperative(&self, rect: [f64; 4]) -> bool {
        self.overlaps_where(rect, |cooperative| !cooperative)
    }

    fn overlaps_where(&self, rect: [f64; 4], blocks: impl Fn(bool) -> bool) -> bool {
        let Some((left, top, right, bottom)) = self.cell_bounds(rect) else {
            return false;
        };
        for y in top..=bottom {
            let row = y * self.cols;
            for x in left..=right {
                let Some(indices) = self.cells.get(row + x) else {
                    continue;
                };
                for &index in indices {
                    let idx = index as usize;
                    if self
                        .boxes
                        .get(idx)
                        .is_some_and(|&boxed| intersects(rect, boxed))
                        && self.cooperative.get(idx).is_some_and(|&coop| blocks(coop))
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    #[cfg(test)]
    pub fn insert(&mut self, rect: [f64; 4]) {
        self.insert_as(rect, false);
    }

    pub fn insert_as(&mut self, rect: [f64; 4], cooperative: bool) {
        let Ok(index) = u32::try_from(self.boxes.len()) else {
            return;
        };
        let bounds = self.cell_bounds(rect);
        self.boxes.push(rect);
        self.cooperative.push(cooperative);
        let Some((left, top, right, bottom)) = bounds else {
            return;
        };
        for y in top..=bottom {
            let row = y * self.cols;
            for x in left..=right {
                if let Some(bucket) = self.cells.get_mut(row + x) {
                    bucket.push(index);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
