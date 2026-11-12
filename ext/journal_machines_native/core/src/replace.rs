//! Replacement lifecycle: proposed, durable-confirmed, reclaimed.
use crate::{Journal, Phase, SegmentId};

/// Handle to one replacement attempt for one sealed segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Replacement {
    pub(crate) segment: SegmentId,
    pub(crate) attempt: u64,
}

impl Replacement {
    /// The segment being replaced.
    #[must_use]
    pub const fn segment(self) -> SegmentId {
        self.segment
    }

    /// Journal-unique attempt number; a rewrite becomes this revision.
    #[must_use]
    pub const fn attempt(self) -> u64 {
        self.attempt
    }
}

/// Refusal to open a replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProposeError {
    /// No such segment.
    Unknown,
    /// The active segment is still being written.
    Active,
    /// The segment already has a replacement in flight.
    Busy,
    /// The attempt space is exhausted.
    Exhausted,
}

/// What the caller made durable in place of the original segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Durable {
    /// A compacted copy of the same offset range holding only live records.
    Rewritten {
        /// Byte size of the compacted copy.
        bytes: u64,
        /// Records the copy holds.
        live_records: u64,
    },
    /// Something outside the journal (a snapshot, an archive, or nothing for
    /// an all-dead segment) supersedes the range; the segment leaves the journal.
    Superseded,
}

/// Outcome of [`Journal::confirm_durable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Confirm {
    /// The replacement is recorded as durable.
    Confirmed,
    /// The handle is not the segment's current attempt.
    Stale,
    /// This attempt was already confirmed; the first confirmation stands.
    AlreadyDurable,
    /// A rewrite cannot hold more records than the original stored.
    Invalid,
}

/// Refusal to reclaim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReclaimError {
    /// The handle is not the segment's current attempt.
    Stale,
    /// The replacement has not been confirmed durable.
    NotDurable,
    /// A pin still retains part of the segment.
    Pinned {
        /// Lowest pinned offset across all pins.
        offset: u64,
    },
}

/// Permission to delete one original segment file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reclaimed {
    /// The segment whose original may now be deleted.
    pub segment: SegmentId,
    /// Revision of the original to delete.
    pub revision: u64,
    /// Bytes the original occupied.
    pub bytes: u64,
    /// Revision now serving the range, or `None` when the segment left the journal.
    pub replaced_by: Option<u64>,
}

/// Outcome of [`Journal::abort`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Abort {
    /// The segment is sealed again; discard the replacement, keep the original.
    Aborted,
    /// The handle is not the segment's current attempt.
    Stale,
}

impl<const SEGMENTS: usize, const PINS: usize> Journal<SEGMENTS, PINS> {
    /// Open a replacement attempt for a sealed segment.
    ///
    /// # Errors
    /// See [`ProposeError`]; a refusal changes nothing.
    pub fn propose(&mut self, id: SegmentId) -> Result<Replacement, ProposeError> {
        let index = self.index_of(id).ok_or(ProposeError::Unknown)?;
        let slot = self.slots[index].as_mut().ok_or(ProposeError::Unknown)?;
        match slot.phase {
            Phase::Active => return Err(ProposeError::Active),
            Phase::Proposed(_) | Phase::Durable(..) => return Err(ProposeError::Busy),
            Phase::Sealed => {}
        }
        let attempt = self
            .attempts
            .checked_add(1)
            .ok_or(ProposeError::Exhausted)?;
        self.attempts = attempt;
        slot.phase = Phase::Proposed(attempt);
        Ok(Replacement {
            segment: id,
            attempt,
        })
    }

    /// Record the caller's confirmation that the replacement is durable.
    /// The journal cannot verify durability; it only refuses to reclaim without it.
    #[must_use]
    pub fn confirm_durable(&mut self, replacement: Replacement, durable: Durable) -> Confirm {
        let Some(slot) = self
            .index_of(replacement.segment)
            .and_then(|index| self.slots[index].as_mut())
        else {
            return Confirm::Stale;
        };
        match slot.phase {
            Phase::Proposed(attempt) if attempt == replacement.attempt => {}
            Phase::Durable(attempt, _) if attempt == replacement.attempt => {
                return Confirm::AlreadyDurable;
            }
            _ => return Confirm::Stale,
        }
        if let Durable::Rewritten { live_records, .. } = durable
            && live_records > slot.stored
        {
            return Confirm::Invalid;
        }
        slot.phase = Phase::Durable(replacement.attempt, durable);
        Confirm::Confirmed
    }

    /// Release the original once its replacement is durable and no pin
    /// retains any of its offsets. Delete the original only after `Ok`.
    ///
    /// # Errors
    /// See [`ReclaimError`]; a refusal changes nothing.
    pub fn reclaim(&mut self, replacement: Replacement) -> Result<Reclaimed, ReclaimError> {
        let index = self
            .index_of(replacement.segment)
            .ok_or(ReclaimError::Stale)?;
        let slot = self.slots[index].ok_or(ReclaimError::Stale)?;
        let durable = match slot.phase {
            Phase::Durable(attempt, durable) if attempt == replacement.attempt => durable,
            Phase::Proposed(attempt) if attempt == replacement.attempt => {
                return Err(ReclaimError::NotDurable);
            }
            _ => return Err(ReclaimError::Stale),
        };
        if let Some(offset) = self.low_pin()
            && offset < slot.end()
        {
            return Err(ReclaimError::Pinned { offset });
        }
        let mut reclaimed = Reclaimed {
            segment: replacement.segment,
            revision: slot.revision,
            bytes: slot.bytes,
            replaced_by: None,
        };
        if let Durable::Rewritten {
            bytes,
            live_records,
        } = durable
        {
            let mut rewritten = slot;
            rewritten.revision = replacement.attempt;
            rewritten.bytes = bytes;
            rewritten.stored = live_records;
            rewritten.live = slot.live.min(live_records);
            rewritten.phase = Phase::Sealed;
            self.slots[index] = Some(rewritten);
            reclaimed.replaced_by = Some(replacement.attempt);
        } else {
            self.slots.copy_within(index + 1..self.len, index);
            self.len -= 1;
            self.slots[self.len] = None;
        }
        Ok(reclaimed)
    }

    /// Abandon a proposed or durable replacement; the original stays.
    #[must_use]
    pub fn abort(&mut self, replacement: Replacement) -> Abort {
        let Some(slot) = self
            .index_of(replacement.segment)
            .and_then(|index| self.slots[index].as_mut())
        else {
            return Abort::Stale;
        };
        match slot.phase {
            Phase::Proposed(attempt) | Phase::Durable(attempt, _)
                if attempt == replacement.attempt =>
            {
                slot.phase = Phase::Sealed;
                Abort::Aborted
            }
            _ => Abort::Stale,
        }
    }
}
