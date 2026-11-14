#![no_std]
#![doc = include_str!("../README.md")]

mod checked;
mod pins;
mod policy;
mod refusal;
mod replace;
mod segment;

pub use pins::{Advanced, Checkpointed, Pin, PinKind};
pub use policy::{Policy, RollReason};
pub use refusal::Refusal;
pub use replace::{Confirmed, Durable, Reclaimed, Replacement};
pub use segment::{Roll, Rolled, Segment, SegmentState};

use core::time::Duration;

use periscope_machines::{Issuer, scope_of};
use pins::PinSlot;

/// Journal-issued segment identity, ascending in offset order.
///
/// It carries the scope of the journal that issued it above a sequence
/// number. The journal draws segment ids, pin generations and attempt numbers
/// from one sequence, so ids ascend but are not consecutive, and no pin
/// generation or attempt of the same journal has the same raw value. A
/// journal of another scope knows no segment by it.
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

    /// The scope of the journal that issued this identity, see
    /// [`Journal::with_scope`].
    #[must_use]
    pub const fn scope(self) -> u16 {
        scope_of(self.0)
    }
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
    pub(crate) opened_at: Duration,
    pub(crate) phase: Phase,
}

impl Slot {
    const fn active(id: u64, base: u64, opened_at: Duration) -> Self {
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
    /// The one sequence every segment id, pin generation and replacement
    /// attempt is drawn from, so no two handles share a raw value.
    pub(crate) issuer: Issuer,
    pub(crate) pins: [Option<PinSlot>; PINS],
}

impl<const SEGMENTS: usize, const PINS: usize> Journal<SEGMENTS, PINS> {
    /// Segment slots, including the active one.
    pub const SEGMENT_CAPACITY: usize = SEGMENTS;
    /// Pin slots, readers and checkpoints together.
    pub const PIN_CAPACITY: usize = PINS;

    /// Start in scope zero with one active segment at `base_offset`, opened
    /// at `now`: monotonic time since an epoch the caller picks.
    #[must_use]
    pub const fn new(policy: Policy, base_offset: u64, now: Duration) -> Self {
        Self::with_scope(policy, base_offset, now, 0)
    }

    /// Start as [`Journal::new`] does, issuing segment ids, pin generations
    /// and replacement attempts that carry `scope`.
    ///
    /// All three come from one sequence counted from one, so no two handles
    /// of a journal share a raw value; the active segment takes the first.
    /// Two journals in the same scope issue the same values, and a handle
    /// given to the wrong journal names something real there. Give every
    /// journal whose handles could meet its own scope: an identity of another
    /// scope then names nothing, and every call that takes one refuses it
    /// before looking anything up. The journal cannot pick a scope itself;
    /// keeping them distinct is the caller's job.
    #[must_use]
    pub const fn with_scope(policy: Policy, base_offset: u64, now: Duration, scope: u16) -> Self {
        Self::opened(policy, base_offset, now, Issuer::new(scope)).0
    }

    /// Start as [`Journal::with_scope`] does after a restart, continuing the
    /// sequence of an earlier journal of `scope` that had issued `issued`
    /// identities (its [`Journal::issued`]). Segments and pins are not
    /// restored: the journal has one active segment at `base_offset`, and its
    /// id is the next of the sequence.
    ///
    /// A journal started again with [`Journal::with_scope`] reissues the
    /// identities of the one before it, so a handle that outlived the restart
    /// could name a new entry. Resume from the saved count, or give each
    /// incarnation a scope of its own. Resuming from a count lower than what
    /// was really issued reissues too: save [`Journal::issued`] before
    /// handing an identity out.
    ///
    /// `None` when `issued` is beyond what a scope holds, or leaves no
    /// identity for the active segment.
    #[must_use]
    pub const fn resumed(
        policy: Policy,
        base_offset: u64,
        now: Duration,
        scope: u16,
        issued: u64,
    ) -> Option<Self> {
        let Some(sequence) = Issuer::resumed(scope, issued) else {
            return None;
        };
        match Self::opened(policy, base_offset, now, sequence) {
            (journal, true) => Some(journal),
            (_, false) => None,
        }
    }

    /// A journal on `issuer`, and whether every segment slot it needs (one,
    /// or none at zero capacity) got its identity.
    const fn opened(policy: Policy, base: u64, now: Duration, mut issuer: Issuer) -> (Self, bool) {
        let mut slots = [None; SEGMENTS];
        let (mut len, mut complete) = (0, true);
        if let Some(first) = slots.first_mut() {
            match issuer.issue() {
                Ok(id) => {
                    *first = Some(Slot::active(id, base, now));
                    len = 1;
                }
                Err(periscope_machines::Refusal::Exhausted) => complete = false,
            }
        }
        let journal = Self {
            policy,
            slots,
            len,
            issuer,
            pins: [None; PINS],
        };
        (journal, complete)
    }

    /// The scope carried by every identity this journal issues.
    #[must_use]
    pub const fn scope(&self) -> u16 {
        self.issuer.scope()
    }

    /// How many identities (segment ids, pin generations and attempts
    /// together) this journal's sequence has issued; what
    /// [`Journal::resumed`] continues from.
    #[must_use]
    pub const fn issued(&self) -> u64 {
        self.issuer.issued()
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

    /// One segment's metadata; `None` for an identity of another scope.
    #[must_use]
    pub fn segment(&self, id: SegmentId) -> Option<Segment> {
        self.find(id).map(|slot| self.view(&slot))
    }

    /// The segment whose range holds `offset`. The active segment answers
    /// for every offset at or after its base.
    #[must_use]
    pub fn covering(&self, offset: u64) -> Option<Segment> {
        self.tracked()
            .find(|slot| {
                offset >= slot.base && (slot.phase == Phase::Active || offset < slot.end())
            })
            .map(|slot| self.view(slot))
    }

    /// Every segment after `after` in ascending identity order. Resume by
    /// passing the last identity yielded.
    ///
    /// `after` is an identity this journal issued: one it never issued (of
    /// another scope, a zero sequence, or one not reached) is no position in
    /// this journal, and the iterator is empty. `None` is the one way to
    /// begin. An issued identity is a position whether or not it still names
    /// a segment, so resuming after a segment that has since left the journal
    /// works, and so does any pin generation or attempt: all three come from
    /// the one sequence that orders segment ids.
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
    /// byte size and record count and the caller's monotonic clock. Reads only.
    ///
    /// An empty segment is never due, whatever its age.
    ///
    /// # Errors
    /// [`Refusal::NoActive`], [`Refusal::TimeRegression`] or
    /// [`Refusal::Overflow`] when the observation cannot be true.
    pub fn roll_decision(&self, bytes: u64, records: u64, now: Duration) -> Result<Roll, Refusal> {
        let active = self.active_slot().ok_or(Refusal::NoActive)?;
        let age = now
            .checked_sub(active.opened_at)
            .ok_or(Refusal::TimeRegression)?;
        active.base.checked_add(records).ok_or(Refusal::Overflow)?;
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
    /// [`Refusal::NoActive`], [`Refusal::Empty`], [`Refusal::TimeRegression`],
    /// [`Refusal::Overflow`], [`Refusal::SegmentsFull`] or
    /// [`Refusal::Exhausted`]; a refusal changes nothing.
    pub fn roll(&mut self, bytes: u64, records: u64, now: Duration) -> Result<Rolled, Refusal> {
        let mut sealed = self.active_slot().ok_or(Refusal::NoActive)?;
        if records == 0 {
            return Err(Refusal::Empty);
        }
        if now < sealed.opened_at {
            return Err(Refusal::TimeRegression);
        }
        let next_offset = sealed.base.checked_add(records).ok_or(Refusal::Overflow)?;
        // The active slot and the one after it; a full journal has no second.
        let Some([active, next]) = self
            .len
            .checked_sub(1)
            .and_then(|last| self.slots.get_mut(last..))
            .and_then(<[_]>::first_chunk_mut)
        else {
            return Err(Refusal::SegmentsFull);
        };
        let opened = self.issuer.issue()?;
        sealed.span = records;
        sealed.stored = records;
        sealed.live = records;
        sealed.bytes = bytes;
        sealed.phase = Phase::Sealed;
        *active = Some(sealed);
        *next = Some(Slot::active(opened, next_offset, now));
        self.len += 1;
        Ok(Rolled {
            sealed: SegmentId(sealed.id),
            opened: SegmentId(opened),
            next_offset,
        })
    }

    /// Record how many of a sealed segment's stored records are still live.
    ///
    /// # Errors
    /// [`Refusal::Unknown`] (which includes an identity of another scope),
    /// [`Refusal::Active`] or [`Refusal::Exceeds`]; a refusal changes nothing.
    pub fn observe_live(&mut self, id: SegmentId, live_records: u64) -> Result<(), Refusal> {
        let (_, slot) = self.find_mut(id).ok_or(Refusal::Unknown)?;
        if slot.phase == Phase::Active {
            return Err(Refusal::Active);
        }
        if live_records > slot.stored {
            return Err(Refusal::Exceeds);
        }
        slot.live = live_records;
        Ok(())
    }

    /// The tracked segments, in slot order.
    pub(crate) fn tracked(&self) -> impl Iterator<Item = &Slot> {
        self.slots.iter().take(self.len).flatten()
    }

    /// A copy of the segment `id` names. An identity this journal never
    /// issued, of another scope or not yet reached, names nothing here: no
    /// slot is read.
    pub(crate) fn find(&self, id: SegmentId) -> Option<Slot> {
        if !self.issuer.owns(id.0) {
            return None;
        }
        self.tracked().find(|slot| slot.id == id.0).copied()
    }

    /// The slot index and the segment `id` names, as [`Journal::find`].
    pub(crate) fn find_mut(&mut self, id: SegmentId) -> Option<(usize, &mut Slot)> {
        locate(&self.issuer, &mut self.slots, self.len, id)
    }

    fn active_slot(&self) -> Option<Slot> {
        let last = self.len.checked_sub(1)?;
        let slot = self.slots.get(last).copied().flatten()?;
        (slot.phase == Phase::Active).then_some(slot)
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
        let index = match after {
            None => 0,
            Some(after) if !self.issuer.owns(after.0) => self.len,
            Some(after) => self
                .slots
                .iter()
                .take(self.len)
                .position(|slot| slot.is_some_and(|slot| slot.id > after.0))
                .unwrap_or(self.len),
        };
        Segments {
            journal: self,
            index,
            filter,
        }
    }
}

/// The slot index and the segment `id` names among the first `len` of
/// `slots`, borrowing only `slots` so the caller may still issue from
/// `issuer`. An identity `issuer` never issued names nothing: no slot is read.
fn locate<'a>(
    issuer: &Issuer,
    slots: &'a mut [Option<Slot>],
    len: usize,
    id: SegmentId,
) -> Option<(usize, &'a mut Slot)> {
    if !issuer.owns(id.0) {
        return None;
    }
    slots
        .iter_mut()
        .take(len)
        .enumerate()
        .find_map(|(index, slot)| {
            slot.as_mut()
                .filter(|slot| slot.id == id.0)
                .map(|slot| (index, slot))
        })
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
            let slot = self.journal.slots.get(self.index).copied().flatten();
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
