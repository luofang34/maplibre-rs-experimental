//! Bitset over [`GroupId`]: which state groups a panel consumes.

use indicate_instrument_state::GroupId;

/// A set of state groups, as a bitset keyed by [`GroupId::index`].
/// Const-constructible so descriptors can declare their needs in
/// `static` data. Dense indexing keeps capacity tied to the number of
/// defined groups rather than to sparse wire-tag allocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GroupSet(u32);

const _: () = assert!(
    GroupId::COUNT <= u32::BITS as usize,
    "GroupSet is too narrow"
);

impl GroupSet {
    /// The empty set.
    pub const EMPTY: GroupSet = GroupSet(0);

    /// The set containing exactly `groups`.
    pub const fn of(groups: &[GroupId]) -> GroupSet {
        let mut bits = 0u32;
        let mut i = 0;
        while i < groups.len() {
            bits |= 1 << groups[i].index();
            i += 1;
        }
        GroupSet(bits)
    }

    /// Whether `group` is in the set.
    pub const fn contains(&self, group: GroupId) -> bool {
        self.0 & (1 << group.index()) != 0
    }

    /// Number of groups in the set.
    pub const fn len(&self) -> u32 {
        self.0.count_ones()
    }

    /// Whether the set is empty.
    pub const fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// The raw bitset, bit position = [`GroupId::index`]. This is the
    /// wasm/FFI encoding of the set.
    pub const fn bits(&self) -> u32 {
        self.0
    }
}

#[cfg(test)]
mod tests;
