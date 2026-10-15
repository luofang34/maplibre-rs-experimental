//! Screen areas the host draws over the map itself, such as its own graphics and their
//! labels, where the map places no label: a label that would fall on one is not shown, and
//! fades out as any label that loses its place does.

/// Rectangles in the map's pixels, `[left, top, right, bottom]` from the top left, which the
/// host replaces whenever what it draws moves.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LabelObstacles {
    boxes: Vec<[f64; 4]>,
    /// Advances with every change, so placement runs again for new obstacles.
    revision: u64,
}

impl LabelObstacles {
    /// The obstacles in force.
    pub fn boxes(&self) -> &[[f64; 4]] {
        &self.boxes
    }

    /// Advances with every change of [`Self::boxes`].
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Replaces the obstacles; true when they changed. Rectangles with a side that is not a
    /// finite, non-negative extent are dropped.
    pub fn replace(&mut self, boxes: Vec<[f64; 4]>) -> bool {
        let boxes: Vec<_> = boxes
            .into_iter()
            .filter(|b| b.iter().all(|v| v.is_finite()) && b[2] >= b[0] && b[3] >= b[1])
            .collect();
        if boxes == self.boxes {
            return false;
        }
        self.boxes = boxes;
        self.revision = self.revision.wrapping_add(1);
        true
    }
}

#[cfg(test)]
mod tests;
