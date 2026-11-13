//! Safe multi-element borrowing from a mutable slice without `unsafe` or `RefCell`.

use smallvec::SmallVec;

/// Splits a mutable slice on demand into disjoint mutable and repeatable shared element borrows.
///
/// Shared borrows downgrade their split-out `&'a mut T` element into `&'a T` and cache that
/// copyable shared reference, so requesting `&A` multiple times in the same tuple query
/// (such as `(&A, &A)`) yields identical references while rejecting `(&A, &mut A)` or
/// `(&mut A, &A)`.
pub struct DisjointSliceBorrow<'a, T> {
    segments: SmallVec<[(usize, &'a mut [T]); 8]>,
    shared: SmallVec<[(usize, &'a T); 6]>,
}

impl<'a, T> DisjointSliceBorrow<'a, T> {
    /// Wraps `slice` so its elements can be borrowed disjointly by index.
    pub fn new(slice: &'a mut [T]) -> Self {
        let mut segments = SmallVec::new();
        if !slice.is_empty() {
            segments.push((0, slice));
        }
        Self {
            segments,
            shared: SmallVec::new(),
        }
    }

    /// Borrows element `index` shared, allowing repeated shared borrows of the same `index`
    /// while rejecting indices already borrowed mutably or out of bounds.
    pub fn borrow_shared(&mut self, index: usize) -> Option<&'a T> {
        if let Some(&(_, existing)) = self.shared.iter().find(|&&(i, _)| i == index) {
            return Some(existing);
        }
        let elem = self.take_slot(index)?;
        let shared_ref: &'a T = elem;
        self.shared.push((index, shared_ref));
        Some(shared_ref)
    }

    /// Borrows element `index` exclusively, returning `None` if `index` is out of bounds or was
    /// already borrowed (either shared or mutably).
    pub fn borrow_mut(&mut self, index: usize) -> Option<&'a mut T> {
        if self.shared.iter().any(|&(i, _)| i == index) {
            return None;
        }
        self.take_slot(index)
    }

    fn take_slot(&mut self, index: usize) -> Option<&'a mut T> {
        let pos = self
            .segments
            .iter()
            .position(|(base, seg)| *base <= index && index < base + seg.len())?;
        let (base, seg) = self.segments.swap_remove(pos);
        let offset = index - base;
        let (left, rest) = seg.split_at_mut(offset);
        let (elem, right) = rest.split_first_mut()?;
        if !left.is_empty() {
            self.segments.push((base, left));
        }
        if !right.is_empty() {
            self.segments.push((index + 1, right));
        }
        Some(elem)
    }
}

#[cfg(test)]
mod tests {
    use super::DisjointSliceBorrow;

    #[test]
    fn disjoint_mutable_and_repeated_shared_borrows_coexist_and_reject_conflicts() {
        let mut data = [10_u32, 20, 30, 40, 50];
        let mut borrow = DisjointSliceBorrow::new(&mut data);

        let middle_a = borrow.borrow_shared(2).expect("shared 2");
        let middle_b = borrow.borrow_shared(2).expect("repeated shared 2");
        assert!(std::ptr::eq(middle_a, middle_b));
        assert!(borrow.borrow_mut(2).is_none());

        let first = borrow.borrow_mut(0).expect("mut 0");
        let last = borrow.borrow_mut(4).expect("mut 4");
        let third = borrow.borrow_mut(3).expect("mut 3");
        assert!(borrow.borrow_mut(0).is_none());
        assert!(borrow.borrow_shared(0).is_none());
        assert!(borrow.borrow_mut(5).is_none());
        assert!(borrow.borrow_shared(5).is_none());

        *first = 100;
        *third = 400;
        *last = 500;
        assert_eq!((*first, *middle_a, *third, *last), (100, 30, 400, 500));
        drop(borrow);
        assert_eq!(data, [100, 20, 30, 400, 500]);
    }

    #[test]
    fn empty_slice_rejects_every_borrow() {
        let mut empty: [u8; 0] = [];
        let mut borrow = DisjointSliceBorrow::new(&mut empty);
        assert!(borrow.borrow_shared(0).is_none());
        assert!(borrow.borrow_mut(0).is_none());
    }
}
