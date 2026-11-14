//! The answers about segments: views, roll decisions and recorded rolls.
use core::time::Duration;

use crate::{Replacement, RollReason, SegmentId};

/// Where a segment stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentState {
    /// The tail segment still being written.
    Active,
    /// Closed, with no replacement in flight.
    Sealed,
    /// A replacement is being built.
    Proposed,
    /// The replacement is confirmed durable, but a pin still retains the original.
    Durable,
    /// The replacement is durable and no pin retains the original.
    Reclaimable,
}

/// Point-in-time view of one segment's metadata.
///
/// A value exists only when some journal could have answered it: build one
/// with [`Segment::new`], read it with the accessors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub(crate) id: SegmentId,
    pub(crate) revision: u64,
    pub(crate) base_offset: u64,
    pub(crate) records: u64,
    pub(crate) stored_records: u64,
    pub(crate) live_records: u64,
    pub(crate) bytes: u64,
    pub(crate) opened_at: Duration,
    pub(crate) state: SegmentState,
    pub(crate) pinned: bool,
    pub(crate) replacement: Option<Replacement>,
}

impl Segment {
    /// Segment identity.
    #[must_use]
    pub const fn id(&self) -> SegmentId {
        self.id
    }

    /// Zero for the original file, otherwise the attempt that rewrote it.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// First offset of the range.
    #[must_use]
    pub const fn base_offset(&self) -> u64 {
        self.base_offset
    }

    /// Offsets in the range; zero while active.
    #[must_use]
    pub const fn records(&self) -> u64 {
        self.records
    }

    /// Records physically held by the current revision.
    #[must_use]
    pub const fn stored_records(&self) -> u64 {
        self.stored_records
    }

    /// Records the caller last reported as still live.
    #[must_use]
    pub const fn live_records(&self) -> u64 {
        self.live_records
    }

    /// Byte size of the current revision; zero while active.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// When the segment was opened, on the caller's monotonic clock.
    #[must_use]
    pub const fn opened_at(&self) -> Duration {
        self.opened_at
    }

    /// Lifecycle state.
    #[must_use]
    pub const fn state(&self) -> SegmentState {
        self.state
    }

    /// Whether any pin retains part of the range; always false while active.
    #[must_use]
    pub const fn pinned(&self) -> bool {
        self.pinned
    }

    /// The replacement attempt in flight, if any.
    #[must_use]
    pub const fn replacement(&self) -> Option<Replacement> {
        self.replacement
    }

    /// One past the last offset of the range.
    #[must_use]
    pub const fn end_offset(&self) -> u64 {
        self.base_offset + self.records
    }
}

/// Answer of [`Journal::roll_decision`]./// Answer of [`Journal::roll_decision`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Roll {
    /// Keep writing to the active segment.
    Keep,
    /// A threshold was reached.
    Due(RollReason),
}

/// A recorded roll; build one with [`Rolled::new`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rolled {
    pub(crate) sealed: SegmentId,
    pub(crate) opened: SegmentId,
    pub(crate) next_offset: u64,
}

impl Rolled {
    /// The segment that was sealed.
    #[must_use]
    pub const fn sealed(&self) -> SegmentId {
        self.sealed
    }

    /// The new active segment.
    #[must_use]
    pub const fn opened(&self) -> SegmentId {
        self.opened
    }

    /// First offset of the new active segment.
    #[must_use]
    pub const fn next_offset(&self) -> u64 {
        self.next_offset
    }
}
