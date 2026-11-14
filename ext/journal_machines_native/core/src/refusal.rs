//! The one answer for every call that changed nothing.
use core::fmt;

/// Why a call was not honoured. Every refusal leaves the journal exactly as
/// it was; no operation reports "not honoured" any other way.
///
/// An idempotent repeat the journal can prove is not a refusal: moving a
/// pin to where it already sits or confirming an attempt again with the same
/// payload is honoured as [`Advanced::Unchanged`](crate::Advanced),
/// [`Checkpointed::Unchanged`](crate::Checkpointed) or
/// [`Confirmed::AlreadyDurable`](crate::Confirmed). A repeat it cannot prove
/// is refused: a second `release`, `reclaim` or `abort` is
/// [`Refusal::Stale`], because the journal keeps neither the pins it released
/// nor how an attempt ended.
///
/// Every identity the journal issues carries its scope and a number of its
/// one sequence. One it never issued, of another scope or not yet reached,
/// names nothing here and is refused before anything is looked up: a segment
/// id is [`Refusal::Unknown`], a pin or a replacement is [`Refusal::Stale`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The journal has no active segment (zero segment capacity).
    NoActive,
    /// An empty active segment cannot be sealed.
    Empty,
    /// `now` is earlier than the active segment's opening time.
    TimeRegression,
    /// The offset range would pass `u64::MAX`.
    Overflow,
    /// Every segment slot is taken; reclaim before rolling.
    SegmentsFull,
    /// Every pin slot is held; release before pinning.
    PinsFull,
    /// The journal's identity sequence is spent: a scope holds 2^48 - 1
    /// segment ids, pin generations and replacement attempts together.
    Exhausted,
    /// No such segment, which includes an identity of another scope.
    Unknown,
    /// The segment is the active one, still being written.
    Active,
    /// The segment already has a replacement in flight.
    Busy,
    /// More live records than the segment stores.
    Exceeds,
    /// The attempt is already confirmed durable with a different payload
    /// than this call carries; the recorded confirmation stands.
    Mismatch,
    /// The handle is not current: a pin that is not held (a released one
    /// included), a replacement that is not the segment's current attempt, or
    /// a handle this journal never issued.
    Stale,
    /// A reader call on a checkpoint pin, or the reverse.
    WrongKind,
    /// Assignment generation zero is never a valid checkpoint source.
    Unassigned,
    /// The assignment generation is older than the one the checkpoint holds.
    StaleAssignment,
    /// The offset is behind the pin; pins never move back within a generation.
    Regressed,
    /// The replacement has not been confirmed durable.
    NotDurable,
    /// A pin still retains part of the segment.
    Pinned {
        /// Lowest pinned offset across all pins.
        offset: u64,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoActive => "the journal has no active segment",
            Self::Empty => "an empty segment cannot be sealed",
            Self::TimeRegression => "time is earlier than the active segment's opening",
            Self::Overflow => "the offset range would overflow",
            Self::SegmentsFull => "every segment slot is in use",
            Self::PinsFull => "every pin slot is in use",
            Self::Exhausted => "identity space exhausted",
            Self::Unknown => "no such segment",
            Self::Active => "the segment is still being written",
            Self::Busy => "the segment already has a replacement in flight",
            Self::Exceeds => "more live records than the segment stores",
            Self::Mismatch => "the attempt is already durable with a different payload",
            Self::Stale => "the handle is not current",
            Self::WrongKind => "the pin is of the other kind",
            Self::Unassigned => "assignment generation zero is not an assignment",
            Self::StaleAssignment => "the assignment generation is older than the pin's",
            Self::Regressed => "the offset is behind the pin",
            Self::NotDurable => "the replacement is not confirmed durable",
            Self::Pinned { .. } => "a pin still retains part of the segment",
        })
    }
}

impl core::error::Error for Refusal {}

/// The journal's identity sequence refused to issue: its one refusal is
/// [`Refusal::Exhausted`].
impl From<periscope_machines::Refusal> for Refusal {
    fn from(refusal: periscope_machines::Refusal) -> Self {
        match refusal {
            periscope_machines::Refusal::Exhausted => Self::Exhausted,
        }
    }
}
