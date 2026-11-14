//! Validating constructors for the public-field answers: a value exists only
//! when some journal could have answered it.
use core::time::Duration;

use periscope_machines::{scope_of, sequence_of};

use crate::{Reclaimed, Replacement, Rolled, Segment, SegmentId, SegmentState};

/// Whether `raw` is a number a journal issues: its sequence is not zero.
const fn issued(raw: u64) -> bool {
    sequence_of(raw) != 0
}

/// Whether `raw` is an identity drawn after segment `id` from the same
/// sequence: same scope, later number. Attempts, and so revisions, are.
const fn drawn_after(id: SegmentId, raw: u64) -> bool {
    scope_of(raw) == id.scope() && sequence_of(raw) > sequence_of(id.raw())
}

/// Whether `revision` is one a segment `id` can carry: zero for the
/// original, otherwise an attempt drawn after it.
const fn revision_of(id: SegmentId, revision: u64) -> bool {
    revision == 0 || drawn_after(id, revision)
}

/// Whether `attempt` can be a later attempt on segment `id` than the one that
/// wrote `revision`.
const fn attempt_on(id: SegmentId, revision: u64, attempt: u64) -> bool {
    drawn_after(id, attempt) && attempt > revision
}

impl Rolled {
    /// A roll some journal could have recorded, or `None`: both ids issued
    /// (a non-zero sequence), of one scope, the opened one drawn after the
    /// sealed one, and a next offset past at least one sealed record.
    #[must_use]
    pub const fn new(sealed: SegmentId, opened: SegmentId, next_offset: u64) -> Option<Self> {
        if issued(sealed.raw()) && drawn_after(sealed, opened.raw()) && next_offset > 0 {
            Some(Self {
                sealed,
                opened,
                next_offset,
            })
        } else {
            None
        }
    }
}

impl Reclaimed {
    /// A reclaim some journal could have answered, or `None`: an issued
    /// segment id; a `revision` that is zero or an attempt drawn after it;
    /// and a `replaced_by`, when present, that is a later attempt still.
    #[must_use]
    pub const fn new(
        segment: SegmentId,
        revision: u64,
        bytes: u64,
        replaced_by: Option<u64>,
    ) -> Option<Self> {
        let replaced = match replaced_by {
            Some(attempt) => attempt_on(segment, revision, attempt),
            None => true,
        };
        if issued(segment.raw()) && revision_of(segment, revision) && replaced {
            Some(Self {
                segment,
                revision,
                bytes,
                replaced_by,
            })
        } else {
            None
        }
    }
}

impl Segment {
    /// A view some journal could have answered, fields in reader order, or
    /// `None`.
    ///
    /// The id is issued and the revision is zero or an attempt drawn after
    /// it. An active segment holds nothing yet: no revision, records, stored
    /// or live records, bytes, pin or replacement. A closed one holds at least
    /// one record within `u64` offsets, stores no more than its range and has
    /// no more live records than it stores. `Sealed` has no replacement; the
    /// other three have one on this segment, drawn after its revision.
    /// `Durable` is pinned, `Reclaimable` is not.
    // One argument per field, in the order the readers and Ruby name them.
    #[expect(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        id: SegmentId,
        revision: u64,
        base_offset: u64,
        records: u64,
        stored_records: u64,
        live_records: u64,
        bytes: u64,
        opened_at: Duration,
        state: SegmentState,
        pinned: bool,
        replacement: Option<Replacement>,
    ) -> Option<Self> {
        Self {
            id,
            revision,
            base_offset,
            records,
            stored_records,
            live_records,
            bytes,
            opened_at,
            state,
            pinned,
            replacement,
        }
        .checked()
    }

    /// This view, when it upholds what [`Segment::new`] checks.
    pub(crate) const fn checked(self) -> Option<Self> {
        let attempt = match self.replacement {
            Some(replacement) => {
                replacement.segment.raw() == self.id.raw()
                    && attempt_on(self.id, self.revision, replacement.attempt)
            }
            None => true,
        };
        let replaced = self.replacement.is_some();
        let closed = self.records > 0
            && self.base_offset.checked_add(self.records).is_some()
            && self.stored_records <= self.records
            && self.live_records <= self.stored_records;
        let state = match self.state {
            SegmentState::Active => {
                self.revision == 0
                    && self.records == 0
                    && self.stored_records == 0
                    && self.live_records == 0
                    && self.bytes == 0
                    && !self.pinned
                    && !replaced
            }
            SegmentState::Sealed => closed && !replaced,
            SegmentState::Proposed => closed && replaced,
            SegmentState::Durable => closed && replaced && self.pinned,
            SegmentState::Reclaimable => closed && replaced && !self.pinned,
        };
        if issued(self.id.raw()) && revision_of(self.id, self.revision) && attempt && state {
            Some(self)
        } else {
            None
        }
    }
}
