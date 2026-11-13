//! Replacement lifecycle: proposed, durable-confirmed, reclaimed.
use crate::{Journal, Phase, Refusal, SegmentId};

/// Handle to one replacement attempt for one sealed segment.
///
/// A handle means something only to the journal that issued it. It is a
/// plain segment id and attempt number, so a journal cannot tell a foreign
/// handle from its own.
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

/// What the caller made durable in place of the original segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Honoured outcome of [`Journal::confirm_durable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmed {
    /// The confirmation was recorded; the replacement is now durable.
    Recorded,
    /// Idempotent repeat: this attempt was confirmed before with exactly
    /// this [`Durable`]. Nothing changed.
    AlreadyDurable,
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

impl<const SEGMENTS: usize, const PINS: usize> Journal<SEGMENTS, PINS> {
    /// Open a replacement attempt for a sealed segment.
    ///
    /// # Errors
    /// [`Refusal::Unknown`], [`Refusal::Active`], [`Refusal::Busy`] or
    /// [`Refusal::Exhausted`] (attempts); a refusal changes nothing.
    pub fn propose(&mut self, id: SegmentId) -> Result<Replacement, Refusal> {
        let index = self.index_of(id).ok_or(Refusal::Unknown)?;
        let slot = self.slots[index].as_mut().ok_or(Refusal::Unknown)?;
        match slot.phase {
            Phase::Active => return Err(Refusal::Active),
            Phase::Proposed(_) | Phase::Durable(..) => return Err(Refusal::Busy),
            Phase::Sealed => {}
        }
        let attempt = self.attempts.checked_add(1).ok_or(Refusal::Exhausted)?;
        self.attempts = attempt;
        slot.phase = Phase::Proposed(attempt);
        Ok(Replacement {
            segment: id,
            attempt,
        })
    }

    /// Record the caller's confirmation that the replacement is durable.
    /// The journal cannot verify durability; it only refuses to reclaim without it.
    ///
    /// Confirming an attempt that is already durable with the `durable` it
    /// was confirmed with is an idempotent repeat,
    /// [`Confirmed::AlreadyDurable`]. Any other `durable` was never recorded
    /// and is refused: the first confirmation stands.
    ///
    /// # Errors
    /// [`Refusal::Stale`], [`Refusal::Mismatch`] for an attempt already
    /// durable with a different `durable`, or, for a rewrite holding more
    /// records than the original stored, [`Refusal::Exceeds`]; a refusal
    /// changes nothing.
    pub fn confirm_durable(
        &mut self,
        replacement: Replacement,
        durable: Durable,
    ) -> Result<Confirmed, Refusal> {
        let slot = self
            .index_of(replacement.segment)
            .and_then(|index| self.slots[index].as_mut())
            .ok_or(Refusal::Stale)?;
        match slot.phase {
            Phase::Proposed(attempt) if attempt == replacement.attempt => {}
            Phase::Durable(attempt, recorded) if attempt == replacement.attempt => {
                return if recorded == durable {
                    Ok(Confirmed::AlreadyDurable)
                } else {
                    Err(Refusal::Mismatch)
                };
            }
            Phase::Active | Phase::Sealed | Phase::Proposed(_) | Phase::Durable(..) => {
                return Err(Refusal::Stale);
            }
        }
        match durable {
            Durable::Rewritten { live_records, .. } if live_records > slot.stored => {
                return Err(Refusal::Exceeds);
            }
            Durable::Rewritten { .. } | Durable::Superseded => {}
        }
        slot.phase = Phase::Durable(replacement.attempt, durable);
        Ok(Confirmed::Recorded)
    }

    /// Release the original once its replacement is durable and no pin
    /// retains any of its offsets. Delete the original only after `Ok`.
    ///
    /// # Errors
    /// [`Refusal::Stale`], [`Refusal::NotDurable`] or [`Refusal::Pinned`];
    /// a refusal changes nothing.
    pub fn reclaim(&mut self, replacement: Replacement) -> Result<Reclaimed, Refusal> {
        let index = self.index_of(replacement.segment).ok_or(Refusal::Stale)?;
        let slot = self.slots[index].ok_or(Refusal::Stale)?;
        let durable = match slot.phase {
            Phase::Durable(attempt, durable) if attempt == replacement.attempt => durable,
            Phase::Proposed(attempt) if attempt == replacement.attempt => {
                return Err(Refusal::NotDurable);
            }
            Phase::Active | Phase::Sealed | Phase::Proposed(_) | Phase::Durable(..) => {
                return Err(Refusal::Stale);
            }
        };
        if let Some(offset) = self.low_pin()
            && offset < slot.end()
        {
            return Err(Refusal::Pinned { offset });
        }
        let mut reclaimed = Reclaimed {
            segment: replacement.segment,
            revision: slot.revision,
            bytes: slot.bytes,
            replaced_by: None,
        };
        match durable {
            Durable::Rewritten {
                bytes,
                live_records,
            } => {
                let mut rewritten = slot;
                rewritten.revision = replacement.attempt;
                rewritten.bytes = bytes;
                rewritten.stored = live_records;
                rewritten.live = slot.live.min(live_records);
                rewritten.phase = Phase::Sealed;
                self.slots[index] = Some(rewritten);
                reclaimed.replaced_by = Some(replacement.attempt);
            }
            Durable::Superseded => {
                self.slots.copy_within(index + 1..self.len, index);
                self.len -= 1;
                self.slots[self.len] = None;
            }
        }
        Ok(reclaimed)
    }

    /// Abandon a proposed or durable replacement; the original stays.
    ///
    /// # Errors
    /// [`Refusal::Stale`]; nothing was abandoned.
    pub fn abort(&mut self, replacement: Replacement) -> Result<(), Refusal> {
        let slot = self
            .index_of(replacement.segment)
            .and_then(|index| self.slots[index].as_mut())
            .ok_or(Refusal::Stale)?;
        match slot.phase {
            Phase::Proposed(attempt) | Phase::Durable(attempt, _)
                if attempt == replacement.attempt =>
            {
                slot.phase = Phase::Sealed;
                Ok(())
            }
            Phase::Active | Phase::Sealed | Phase::Proposed(_) | Phase::Durable(..) => {
                Err(Refusal::Stale)
            }
        }
    }
}
