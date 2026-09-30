//! Screen-space collision lookup bounded by the viewport.
use std::collections::HashMap;

pub(super) struct CollisionGrid {
    cells: HashMap<(i32, i32), Vec<usize>>,
    boxes: Vec<[f64; 4]>,
    /// Whether each box belongs to a symbol that lets other cooperative symbols overlap it.
    cooperative: Vec<bool>,
    limit: [i32; 2],
}

pub(super) fn intersects(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] < b[2] && a[2] > b[0] && a[1] < b[3] && a[3] > b[1]
}

impl CollisionGrid {
    pub fn new(width: f64, height: f64) -> Self {
        Self {
            cells: HashMap::new(),
            boxes: Vec::new(),
            cooperative: Vec::new(),
            limit: [(width / 64.0).ceil() as i32, (height / 64.0).ceil() as i32],
        }
    }

    fn cells_for(&self, rect: [f64; 4]) -> impl Iterator<Item = (i32, i32)> {
        let [left, top, right, bottom] = std::array::from_fn(|i| {
            (rect[i] / 64.0)
                .floor()
                .clamp(-1.0, f64::from(self.limit[i % 2])) as i32
        });
        (top..=bottom).flat_map(move |y| (left..=right).map(move |x| (x, y)))
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
        self.cells_for(rect).any(|cell| {
            self.cells.get(&cell).is_some_and(|indices| {
                indices.iter().any(|&index| {
                    intersects(rect, self.boxes[index]) && blocks(self.cooperative[index])
                })
            })
        })
    }

    #[cfg(test)]
    pub fn insert(&mut self, rect: [f64; 4]) {
        self.insert_as(rect, false);
    }

    pub fn insert_as(&mut self, rect: [f64; 4], cooperative: bool) {
        let index = self.boxes.len();
        let cells: Vec<_> = self.cells_for(rect).collect();
        self.boxes.push(rect);
        self.cooperative.push(cooperative);
        for cell in cells {
            self.cells.entry(cell).or_default().push(index);
        }
    }
}

#[cfg(test)]
mod tests;
