//! Reader and checkpoint retention pins.
use offset_machines::Tracker;

use crate::Journal;

/// What a pin stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PinKind {
    /// A reader positioned at the pinned offset.
    Reader,
    /// Recovery replays from the pinned offset.
    Checkpoint,
}

/// Handle to one pin acquisition. A released pin's handle is stale forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin {
    pub(crate) slot: usize,
    pub(crate) generation: u64,
    pub(crate) kind: PinKind,
}

impl Pin {
    /// Reader or checkpoint.
    #[must_use]
    pub const fn kind(self) -> PinKind {
        self.kind
    }

    /// Journal-issued acquisition generation, unique per journal.
    #[must_use]
    pub const fn generation(self) -> u64 {
        self.generation
    }
}

/// Refusal to acquire a pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PinError {
    /// Every pin slot is held.
    Full,
    /// The pin generation space is exhausted.
    Exhausted,
    /// Assignment generation zero is never a valid checkpoint source.
    Unassigned,
}

/// Outcome of moving a pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PinUpdate {
    /// The pin moved forward.
    Advanced,
    /// The pin already sits at that offset.
    Unchanged,
    /// A newer assignment generation took over the checkpoint at its own offset.
    Reassigned,
    /// The handle was released or never belonged to this slot.
    Stale,
    /// The offset is behind the pin; pins never move back within a generation.
    Regressed,
    /// The assignment generation is older than the one the checkpoint holds.
    StaleAssignment,
    /// Assignment generation zero is never a valid checkpoint source.
    Unassigned,
    /// A reader call on a checkpoint pin, or the reverse.
    WrongKind,
}

/// Outcome of [`Journal::release`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Release {
    /// The pin no longer retains anything.
    Released,
    /// The handle's generation does not match the slot; nothing was released.
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinSlot {
    pub generation: u64,
    pub kind: PinKind,
    pub offset: u64,
    pub assignment: u64,
}

impl<const SEGMENTS: usize, const PINS: usize> Journal<SEGMENTS, PINS> {
    /// Retain every offset at or after `offset` for a reader.
    ///
    /// # Errors
    /// [`PinError::Full`] or [`PinError::Exhausted`]; nothing is pinned.
    pub fn pin_reader(&mut self, offset: u64) -> Result<Pin, PinError> {
        self.acquire(PinKind::Reader, offset, 0)
    }

    /// Retain every offset at or after `offset` for recovery, on behalf of
    /// the consumer assignment generation `assignment`.
    ///
    /// # Errors
    /// [`PinError::Unassigned`] for generation zero, otherwise as
    /// [`Journal::pin_reader`].
    pub fn pin_checkpoint(&mut self, assignment: u64, offset: u64) -> Result<Pin, PinError> {
        if assignment == 0 {
            return Err(PinError::Unassigned);
        }
        self.acquire(PinKind::Checkpoint, offset, assignment)
    }

    /// Pin a checkpoint at a tracker's confirmed-committed base, under its
    /// assignment generation. Acknowledged but unconfirmed offsets stay retained.
    ///
    /// # Errors
    /// As [`Journal::pin_checkpoint`]; an unassigned tracker is `Unassigned`.
    pub fn pin_checkpoint_from<const WORDS: usize>(
        &mut self,
        tracker: &Tracker<WORDS>,
    ) -> Result<Pin, PinError> {
        self.pin_checkpoint(tracker.generation(), tracker.base())
    }

    /// Move a reader pin forward.
    #[must_use]
    pub fn advance(&mut self, pin: Pin, offset: u64) -> PinUpdate {
        let Some(held) = self.held_mut(pin) else {
            return PinUpdate::Stale;
        };
        if held.kind != PinKind::Reader {
            return PinUpdate::WrongKind;
        }
        if offset < held.offset {
            return PinUpdate::Regressed;
        }
        if offset == held.offset {
            return PinUpdate::Unchanged;
        }
        held.offset = offset;
        PinUpdate::Advanced
    }

    /// Move a checkpoint pin to `offset` as confirmed under `assignment`.
    ///
    /// The same assignment generation only moves forward. A newer one takes
    /// over at whatever offset it reports, even a lower one; an older one is
    /// refused, so a fenced consumer cannot loosen retention.
    #[must_use]
    pub fn checkpoint(&mut self, pin: Pin, assignment: u64, offset: u64) -> PinUpdate {
        let Some(held) = self.held_mut(pin) else {
            return PinUpdate::Stale;
        };
        if held.kind != PinKind::Checkpoint {
            return PinUpdate::WrongKind;
        }
        if assignment == 0 {
            return PinUpdate::Unassigned;
        }
        if assignment < held.assignment {
            return PinUpdate::StaleAssignment;
        }
        if assignment > held.assignment {
            held.assignment = assignment;
            held.offset = offset;
            return PinUpdate::Reassigned;
        }
        if offset < held.offset {
            return PinUpdate::Regressed;
        }
        if offset == held.offset {
            return PinUpdate::Unchanged;
        }
        held.offset = offset;
        PinUpdate::Advanced
    }

    /// Move a checkpoint pin to a tracker's confirmed-committed base.
    #[must_use]
    pub fn checkpoint_from<const WORDS: usize>(
        &mut self,
        pin: Pin,
        tracker: &Tracker<WORDS>,
    ) -> PinUpdate {
        self.checkpoint(pin, tracker.generation(), tracker.base())
    }

    /// Drop a pin. Only the handle of the current acquisition releases it.
    #[must_use]
    pub fn release(&mut self, pin: Pin) -> Release {
        if self.held_mut(pin).is_none() {
            return Release::Stale;
        }
        self.pins[pin.slot] = None;
        Release::Released
    }

    /// Offset a live pin retains from; `None` for a stale handle.
    #[must_use]
    pub fn pin_offset(&self, pin: Pin) -> Option<u64> {
        self.pins
            .get(pin.slot)
            .copied()
            .flatten()
            .filter(|held| held.generation == pin.generation)
            .map(|held| held.offset)
    }

    /// Lowest retained offset across all pins; `None` when nothing is pinned.
    #[must_use]
    pub fn low_pin(&self) -> Option<u64> {
        self.pins.iter().flatten().map(|held| held.offset).min()
    }

    /// Number of pins currently held.
    #[must_use]
    pub fn pins(&self) -> usize {
        self.pins.iter().flatten().count()
    }

    fn acquire(&mut self, kind: PinKind, offset: u64, assignment: u64) -> Result<Pin, PinError> {
        let slot = self
            .pins
            .iter()
            .position(Option::is_none)
            .ok_or(PinError::Full)?;
        let generation = self
            .pin_generations
            .checked_add(1)
            .ok_or(PinError::Exhausted)?;
        self.pin_generations = generation;
        self.pins[slot] = Some(PinSlot {
            generation,
            kind,
            offset,
            assignment,
        });
        Ok(Pin {
            slot,
            generation,
            kind,
        })
    }

    fn held_mut(&mut self, pin: Pin) -> Option<&mut PinSlot> {
        self.pins
            .get_mut(pin.slot)?
            .as_mut()
            .filter(|held| held.generation == pin.generation)
    }
}
