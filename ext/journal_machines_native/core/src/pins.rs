//! Reader and checkpoint retention pins.
use offset_machines::Tracker;
use periscope_machines::scope_of;

use crate::{Journal, Refusal};

/// What a pin stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinKind {
    /// A reader positioned at the pinned offset.
    Reader,
    /// Recovery replays from the pinned offset.
    Checkpoint,
}

/// Handle to one pin acquisition. A released pin's handle never names a pin
/// again.
///
/// A pin means something only to the journal that issued it. Its generation
/// carries that journal's scope, so a journal of another scope refuses it;
/// two journals left in the same scope cannot tell each other's pins apart.
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

    /// Journal-issued acquisition generation, unique per journal and never
    /// reissued: the journal's scope above a number of the one sequence its
    /// segment ids and attempts are drawn from too.
    #[must_use]
    pub const fn generation(self) -> u64 {
        self.generation
    }

    /// The scope of the journal that issued this pin, see
    /// [`Journal::with_scope`].
    #[must_use]
    pub const fn scope(self) -> u16 {
        scope_of(self.generation)
    }
}

/// Honoured outcome of [`Journal::advance`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advanced {
    /// The reader pin moved forward.
    Moved,
    /// Idempotent repeat: the pin already sits at that offset. Nothing changed.
    Unchanged,
}

/// Honoured outcome of [`Journal::checkpoint`] and [`Journal::checkpoint_from`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checkpointed {
    /// The same assignment generation moved the pin forward.
    Advanced,
    /// A newer assignment generation took over the checkpoint at its own offset.
    Reassigned,
    /// Idempotent repeat: the same assignment generation reported the offset
    /// the pin already sits at. Nothing changed.
    Unchanged,
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
    /// [`Refusal::PinsFull`] or [`Refusal::Exhausted`]; nothing is pinned.
    pub fn pin_reader(&mut self, offset: u64) -> Result<Pin, Refusal> {
        self.acquire(PinKind::Reader, offset, 0)
    }

    /// Retain every offset at or after `offset` for recovery, on behalf of
    /// the consumer assignment generation `assignment`.
    ///
    /// # Errors
    /// [`Refusal::Unassigned`] for generation zero, otherwise as
    /// [`Journal::pin_reader`].
    pub fn pin_checkpoint(&mut self, assignment: u64, offset: u64) -> Result<Pin, Refusal> {
        if assignment == 0 {
            return Err(Refusal::Unassigned);
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
    ) -> Result<Pin, Refusal> {
        self.pin_checkpoint(tracker.generation(), tracker.base())
    }

    /// Move a reader pin forward. The offset the pin already sits at is an
    /// idempotent repeat: [`Advanced::Unchanged`].
    ///
    /// # Errors
    /// [`Refusal::Stale`] (a pin that is not held, or one of another scope),
    /// [`Refusal::WrongKind`] or [`Refusal::Regressed`]; the pin stays where
    /// it was.
    pub fn advance(&mut self, pin: Pin, offset: u64) -> Result<Advanced, Refusal> {
        let held = self.held_mut(pin).ok_or(Refusal::Stale)?;
        if held.kind != PinKind::Reader {
            return Err(Refusal::WrongKind);
        }
        if !forward(held.offset, offset)? {
            return Ok(Advanced::Unchanged);
        }
        held.offset = offset;
        Ok(Advanced::Moved)
    }

    /// Move a checkpoint pin to `offset` as confirmed under `assignment`.
    ///
    /// The same assignment generation only moves forward. A newer one takes
    /// over at whatever offset it reports, even a lower one; an older one is
    /// refused, so a fenced consumer cannot loosen retention. The same
    /// generation at the offset the pin already sits at is an idempotent
    /// repeat: [`Checkpointed::Unchanged`].
    ///
    /// # Errors
    /// [`Refusal::Stale`], [`Refusal::WrongKind`], [`Refusal::Unassigned`],
    /// [`Refusal::StaleAssignment`] or [`Refusal::Regressed`]; the pin stays
    /// where it was.
    pub fn checkpoint(
        &mut self,
        pin: Pin,
        assignment: u64,
        offset: u64,
    ) -> Result<Checkpointed, Refusal> {
        let held = self.held_mut(pin).ok_or(Refusal::Stale)?;
        if held.kind != PinKind::Checkpoint {
            return Err(Refusal::WrongKind);
        }
        if assignment == 0 {
            return Err(Refusal::Unassigned);
        }
        if assignment < held.assignment {
            return Err(Refusal::StaleAssignment);
        }
        if assignment > held.assignment {
            held.assignment = assignment;
            held.offset = offset;
            return Ok(Checkpointed::Reassigned);
        }
        if !forward(held.offset, offset)? {
            return Ok(Checkpointed::Unchanged);
        }
        held.offset = offset;
        Ok(Checkpointed::Advanced)
    }

    /// Move a checkpoint pin to a tracker's confirmed-committed base.
    ///
    /// # Errors
    /// As [`Journal::checkpoint`]; an unassigned tracker is `Unassigned`.
    pub fn checkpoint_from<const WORDS: usize>(
        &mut self,
        pin: Pin,
        tracker: &Tracker<WORDS>,
    ) -> Result<Checkpointed, Refusal> {
        self.checkpoint(pin, tracker.generation(), tracker.base())
    }

    /// Drop a pin. Only the handle of the current acquisition releases it.
    ///
    /// A second release is not an honoured repeat, because the journal cannot
    /// prove it. Pin generations share one sequence with segment ids and
    /// attempts, so "issued here and held in no slot" is as true of a segment
    /// id as of a released pin, and the journal keeps no record of which
    /// identities were pins or of the pins it released.
    ///
    /// # Errors
    /// [`Refusal::Stale`] for a pin that is not held: released already, never
    /// issued here (another scope, or a generation not reached), or held in
    /// another slot than the one the handle names. Nothing was released, and
    /// whoever holds the slot keeps it.
    pub fn release(&mut self, pin: Pin) -> Result<(), Refusal> {
        self.held_mut(pin).ok_or(Refusal::Stale)?;
        self.pins[pin.slot] = None;
        Ok(())
    }

    /// Offset a live pin retains from; `None` for a stale handle or one of
    /// another scope.
    #[must_use]
    pub fn pin_offset(&self, pin: Pin) -> Option<u64> {
        if !self.issuer.owns(pin.generation) {
            return None;
        }
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

    fn acquire(&mut self, kind: PinKind, offset: u64, assignment: u64) -> Result<Pin, Refusal> {
        let slot = self
            .pins
            .iter()
            .position(Option::is_none)
            .ok_or(Refusal::PinsFull)?;
        let generation = match self.issuer.issue() {
            Ok(generation) => generation,
            Err(periscope_machines::Refusal::Exhausted) => return Err(Refusal::Exhausted),
        };
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

    /// A pin this journal never issued, of another scope or not yet reached,
    /// names nothing here: no slot is read.
    fn held_mut(&mut self, pin: Pin) -> Option<&mut PinSlot> {
        if !self.issuer.owns(pin.generation) {
            return None;
        }
        self.pins
            .get_mut(pin.slot)?
            .as_mut()
            .filter(|held| held.generation == pin.generation)
    }
}

/// Whether moving a pin held at `held` to `offset` moves it forward; `false`
/// when it already sits there.
const fn forward(held: u64, offset: u64) -> Result<bool, Refusal> {
    if offset < held {
        Err(Refusal::Regressed)
    } else {
        Ok(offset > held)
    }
}
