//! Roll and compaction thresholds.
use core::{num::NonZeroU64, time::Duration};

/// The whole of a per-mille ratio: every stored record dead.
const PER_MILLE: u16 = 1000;

/// Why the active segment is due to roll. Checked in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollReason {
    /// The observed byte size reached `max_bytes`.
    Size,
    /// The observed record count reached `max_records`.
    Records,
    /// The observed age reached `max_age`.
    Age,
}

/// Roll thresholds and the dead-record ratio that makes a segment worth
/// compacting. A `None` threshold never triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    max_bytes: Option<NonZeroU64>,
    max_records: Option<NonZeroU64>,
    max_age: Option<Duration>,
    min_dead_per_mille: u16,
}

impl Policy {
    /// Build a policy; `None` when `min_dead_per_mille` exceeds 1000 or
    /// `max_age` is a zero span (disable a threshold with `None`, not zero).
    ///
    /// `max_age` is a span of the same monotonic clock that supplies `now`.
    #[must_use]
    pub const fn new(
        max_bytes: Option<NonZeroU64>,
        max_records: Option<NonZeroU64>,
        max_age: Option<Duration>,
        min_dead_per_mille: u16,
    ) -> Option<Self> {
        if min_dead_per_mille > PER_MILLE || matches!(max_age, Some(age) if age.is_zero()) {
            return None;
        }
        Some(Self {
            max_bytes,
            max_records,
            max_age,
            min_dead_per_mille,
        })
    }

    /// Byte size at which the active segment is due.
    #[must_use]
    pub const fn max_bytes(&self) -> Option<NonZeroU64> {
        self.max_bytes
    }

    /// Record count at which the active segment is due.
    #[must_use]
    pub const fn max_records(&self) -> Option<NonZeroU64> {
        self.max_records
    }

    /// Age at which a non-empty active segment is due.
    #[must_use]
    pub const fn max_age(&self) -> Option<Duration> {
        self.max_age
    }

    /// Dead records per thousand stored records that make a compaction candidate.
    #[must_use]
    pub const fn min_dead_per_mille(&self) -> u16 {
        self.min_dead_per_mille
    }

    pub(crate) fn due(&self, bytes: u64, records: u64, age: Duration) -> Option<RollReason> {
        let reached = |limit: Option<NonZeroU64>, seen: u64| limit.is_some_and(|l| seen >= l.get());
        if reached(self.max_bytes, bytes) {
            Some(RollReason::Size)
        } else if reached(self.max_records, records) {
            Some(RollReason::Records)
        } else if self.max_age.is_some_and(|limit| age >= limit) {
            Some(RollReason::Age)
        } else {
            None
        }
    }

    /// At least one dead record, and the dead share meets the threshold.
    pub(crate) fn compactable(&self, stored: u64, live: u64) -> bool {
        let dead = stored.saturating_sub(live);
        dead > 0
            && u128::from(dead) * u128::from(PER_MILLE)
                >= u128::from(stored) * u128::from(self.min_dead_per_mille)
    }
}
