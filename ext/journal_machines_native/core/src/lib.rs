#![no_std]
#![doc = include_str!("../README.md")]

mod pins;
mod policy;
mod replace;

pub use pins::{Pin, PinError, PinKind, PinUpdate, Release};
pub use policy::{Policy, RollReason};
pub use replace::{Abort, Confirm, Durable, ProposeError, ReclaimError, Reclaimed, Replacement};

use pins::PinSlot;

/// Journal-issued segment identity, ascending in offset order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SegmentId(u64);

impl SegmentId {
    /// Adopt an identity read back from storage or another process.
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Raw identity for file naming and persistence.
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Where a segment stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// Segment identity.
    pub id: SegmentId,
    /// Zero for the original file, otherwise the attempt that rewrote it.
    pub revision: u64,
    /// First offset of the range.
    pub base_offset: u64,
    /// Offsets in the range; zero while active.
    pub records: u64,
    /// Records physically held by the current revision.
    pub stored_records: u64,
    /// Records the caller last reported as still live.
    pub live_records: u64,
    /// Byte size of the current revision; zero while active.
    pub bytes: u64,
    /// Caller time at which the segment was opened.
    pub opened_at: u64,
    /// Lifecycle state.
    pub state: SegmentState,
    /// Whether any pin retains part of the range; always false while active.
    pub pinned: bool,
    /// The replacement attempt in flight, if any.
    pub replacement: Option<Replacement>,
}

impl Segment {
    /// One past the last offset of the range.
    #[must_use]
    pub const fn end_offset(&self) -> u64 {
        self.base_offset + self.records
    }
}

/// Answer of [`Journal::roll_decision`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Roll {
    /// Keep writing to the active segment.
    Keep,
    /// A threshold was reached.
    Due(RollReason),
}

/// Refusal to decide on or record a roll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RollError {
    /// The journal has no active segment (zero segment capacity).
    NoActive,
    /// An empty segment cannot be sealed.
    Empty,
    /// `now` is earlier than the active segment's opening time.
    TimeRegression,
    /// Every segment slot is taken; reclaim before rolling.
    Full,
    /// The offset range or the identity space would overflow.
    Overflow,
}

/// A recorded roll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rolled {
    /// The segment that was sealed.
    pub sealed: SegmentId,
    /// The new active segment.
    pub opened: SegmentId,
    /// First offset of the new active segment.
    pub next_offset: u64,
}

/// Outcome of [`Journal::observe_live`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Observe {
    /// The live count was recorded.
    Recorded,
    /// No such segment.
    Unknown,
    /// The active segment has no live count yet.
    Active,
    /// More live records than the segment stores.
    Exceeds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Active,
    Sealed,
    Proposed(u64),
    Durable(u64, Durable),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Slot {
    pub(crate) id: u64,
    pub(crate) revision: u64,
    pub(crate) base: u64,
    pub(crate) span: u64,
    pub(crate) stored: u64,
    pub(crate) live: u64,
    pub(crate) bytes: u64,
    pub(crate) opened_at: u64,
    pub(crate) phase: Phase,
}

impl Slot {
    const fn active(id: u64, base: u64, opened_at: u64) -> Self {
        Self {
            id,
            revision: 0,
            base,
            span: 0,
            stored: 0,
            live: 0,
            bytes: 0,
            opened_at,
            phase: Phase::Active,
        }
    }

    /// Never overflows: the range was checked when the segment was sealed.
    pub(crate) const fn end(&self) -> u64 {
        self.base + self.span
    }
}

/// Metadata for up to `SEGMENTS` log segments and `PINS` retention pins.
///
/// The journal writes nothing, compacts nothing and deletes nothing. It keeps
/// the bookkeeping that says when a segment should roll, which sealed
/// segments are worth compacting, and when an original may be deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Journal<const SEGMENTS: usize, const PINS: usize> {
    pub(crate) policy: Policy,
    pub(crate) slots: [Option<Slot>; SEGMENTS],
    pub(crate) len: usize,
    pub(crate) issued: u64,
    pub(crate) attempts: u64,
    pub(crate) pins: [Option<PinSlot>; PINS],
    pub(crate) pin_generations: u64,
}

impl<const SEGMENTS: usize, const PINS: usize> Journal<SEGMENTS, PINS> {
    /// Segment slots, including the active one.
    pub const SEGMENT_CAPACITY: usize = SEGMENTS;
    /// Pin slots, readers and checkpoints together.
    pub const PIN_CAPACITY: usize = PINS;

    /// Start with one active segment at `base_offset`, opened at `now`.
    #[must_use]
    pub const fn new(policy: Policy, base_offset: u64, now: u64) -> Self {
        let mut slots = [None; SEGMENTS];
        let len = if let Some(first) = slots.first_mut() {
            *first = Some(Slot::active(1, base_offset, now));
            1
        } else {
            0
        };
        Self {
            policy,
            slots,
            len,
            issued: len as u64,
            attempts: 0,
            pins: [None; PINS],
            pin_generations: 0,
        }
    }

    /// The thresholds this journal decides with.
    #[must_use]
    pub const fn policy(&self) -> Policy {
        self.policy
    }

    /// Segments currently tracked, including the active one.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// True only for a journal with zero segment capacity.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The segment being written.
    #[must_use]
    pub fn active(&self) -> Option<Segment> {
        self.active_slot().map(|slot| self.view(&slot))
    }

    /// One segment's metadata.
    #[must_use]
    pub fn segment(&self, id: SegmentId) -> Option<Segment> {
        let slot = self.slots[self.index_of(id)?]?;
        Some(self.view(&slot))
    }

    /// The segment whose range holds `offset`. The active segment answers
    /// for every offset at or after its base.
    #[must_use]
    pub fn covering(&self, offset: u64) -> Option<Segment> {
        self.slots[..self.len]
            .iter()
            .flatten()
            .find(|slot| {
                offset >= slot.base && (slot.phase == Phase::Active || offset < slot.end())
            })
            .map(|slot| self.view(slot))
    }

    /// Every segment after `after` in ascending identity order. Resume by
    /// passing the last identity yielded.
    #[must_use]
    pub fn segments(&self, after: Option<SegmentId>) -> Segments<'_, SEGMENTS, PINS> {
        self.scan(after, Filter::All)
    }

    /// Sealed segments with no replacement in flight whose dead-record share
    /// meets the policy, oldest first. Resume as with [`Journal::segments`].
    #[must_use]
    pub fn compaction_candidates(&self, after: Option<SegmentId>) -> Segments<'_, SEGMENTS, PINS> {
        self.scan(after, Filter::Candidates)
    }

    /// Segments whose replacement is durable and which no pin retains.
    /// Resume as with [`Journal::segments`].
    #[must_use]
    pub fn reclaimable(&self, after: Option<SegmentId>) -> Segments<'_, SEGMENTS, PINS> {
        self.scan(after, Filter::Reclaimable)
    }

    /// Decide whether the active segment should roll, from its observed
    /// byte size and record count and the caller's clock. Reads only.
    ///
    /// An empty segment is never due, whatever its age.
    ///
    /// # Errors
    /// [`RollError::NoActive`], [`RollError::TimeRegression`] or
    /// [`RollError::Overflow`] when the observation cannot be true.
    pub fn roll_decision(&self, bytes: u64, records: u64, now: u64) -> Result<Roll, RollError> {
        let active = self.active_slot().ok_or(RollError::NoActive)?;
        let age = now
            .checked_sub(active.opened_at)
            .ok_or(RollError::TimeRegression)?;
        active
            .base
            .checked_add(records)
            .ok_or(RollError::Overflow)?;
        if records == 0 {
            return Ok(Roll::Keep);
        }
        Ok(self
            .policy
            .due(bytes, records, age)
            .map_or(Roll::Keep, Roll::Due))
    }

    /// Record that the caller sealed the active segment at `bytes` and
    /// `records` and opened a new one at `now`. A roll need not be due.
    ///
    /// # Errors
    /// See [`RollError`]; a refusal changes nothing.
    pub fn roll(&mut self, bytes: u64, records: u64, now: u64) -> Result<Rolled, RollError> {
        let mut sealed = self.active_slot().ok_or(RollError::NoActive)?;
        if records == 0 {
            return Err(RollError::Empty);
        }
        if now < sealed.opened_at {
            return Err(RollError::TimeRegression);
        }
        let next_offset = sealed
            .base
            .checked_add(records)
            .ok_or(RollError::Overflow)?;
        if self.len == SEGMENTS {
            return Err(RollError::Full);
        }
        let opened = self.issued.checked_add(1).ok_or(RollError::Overflow)?;
        sealed.span = records;
        sealed.stored = records;
        sealed.live = records;
        sealed.bytes = bytes;
        sealed.phase = Phase::Sealed;
        self.slots[self.len - 1] = Some(sealed);
        self.slots[self.len] = Some(Slot::active(opened, next_offset, now));
        self.len += 1;
        self.issued = opened;
        Ok(Rolled {
            sealed: SegmentId(sealed.id),
            opened: SegmentId(opened),
            next_offset,
        })
    }

    /// Record how many of a sealed segment's stored records are still live.
    #[must_use]
    pub fn observe_live(&mut self, id: SegmentId, live_records: u64) -> Observe {
        let Some(slot) = self
            .index_of(id)
            .and_then(|index| self.slots[index].as_mut())
        else {
            return Observe::Unknown;
        };
        if slot.phase == Phase::Active {
            return Observe::Active;
        }
        if live_records > slot.stored {
            return Observe::Exceeds;
        }
        slot.live = live_records;
        Observe::Recorded
    }

    pub(crate) fn index_of(&self, id: SegmentId) -> Option<usize> {
        self.slots[..self.len]
            .iter()
            .position(|slot| slot.is_some_and(|slot| slot.id == id.0))
    }

    fn active_slot(&self) -> Option<Slot> {
        let last = self.slots[..self.len].last().copied().flatten()?;
        (last.phase == Phase::Active).then_some(last)
    }

    fn view(&self, slot: &Slot) -> Segment {
        let active = slot.phase == Phase::Active;
        let pinned = !active && self.low_pin().is_some_and(|offset| offset < slot.end());
        let id = SegmentId(slot.id);
        let (state, attempt) = match slot.phase {
            Phase::Active => (SegmentState::Active, None),
            Phase::Sealed => (SegmentState::Sealed, None),
            Phase::Proposed(attempt) => (SegmentState::Proposed, Some(attempt)),
            Phase::Durable(attempt, _) if pinned => (SegmentState::Durable, Some(attempt)),
            Phase::Durable(attempt, _) => (SegmentState::Reclaimable, Some(attempt)),
        };
        Segment {
            id,
            revision: slot.revision,
            base_offset: slot.base,
            records: slot.span,
            stored_records: slot.stored,
            live_records: slot.live,
            bytes: slot.bytes,
            opened_at: slot.opened_at,
            state,
            pinned,
            replacement: attempt.map(|attempt| Replacement {
                segment: id,
                attempt,
            }),
        }
    }

    fn scan(&self, after: Option<SegmentId>, filter: Filter) -> Segments<'_, SEGMENTS, PINS> {
        let index = after.map_or(0, |after| {
            self.slots[..self.len]
                .iter()
                .position(|slot| slot.is_some_and(|slot| slot.id > after.0))
                .unwrap_or(self.len)
        });
        Segments {
            journal: self,
            index,
            filter,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Filter {
    All,
    Candidates,
    Reclaimable,
}

/// Resumable iterator over segment views, bounded by the segment capacity.
#[derive(Debug, Clone)]
pub struct Segments<'a, const SEGMENTS: usize, const PINS: usize> {
    journal: &'a Journal<SEGMENTS, PINS>,
    index: usize,
    filter: Filter,
}

impl<const SEGMENTS: usize, const PINS: usize> Iterator for Segments<'_, SEGMENTS, PINS> {
    type Item = Segment;

    fn next(&mut self) -> Option<Segment> {
        while self.index < self.journal.len {
            let slot = self.journal.slots[self.index];
            self.index += 1;
            let Some(view) = slot.map(|slot| self.journal.view(&slot)) else {
                continue;
            };
            let wanted = match self.filter {
                Filter::All => true,
                Filter::Candidates => {
                    view.state == SegmentState::Sealed
                        && self
                            .journal
                            .policy
                            .compactable(view.stored_records, view.live_records)
                }
                Filter::Reclaimable => view.state == SegmentState::Reclaimable,
            };
            if wanted {
                return Some(view);
            }
        }
        None
    }
}

#[cfg(test)]
extern crate std;
#[cfg(test)]
mod tests;
