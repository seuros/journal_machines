//! Ruby value conversion and refusal names.
use std::num::NonZeroU64;

use journal_machines::{Policy, Refusal};
use magnus::{Error, Ruby, Symbol, Value, prelude::*};
use matryona_machines::ruby::{exact_fields, integer, positive, symbol};

use crate::classes::classes;

/// A roll threshold: `nil` disables it, anything else is a positive Integer.
fn threshold(value: Option<Value>, name: &str) -> Result<Option<NonZeroU64>, Error> {
    value.map(|value| positive(value, name)).transpose()
}

/// The core policy for the four `Policy` fields as Ruby passes them. Each is
/// converted first, in order; then `RangeError` where the core constructor
/// answers `None`.
pub fn policy(
    ruby: &Ruby,
    max_bytes: Option<Value>,
    max_records: Option<Value>,
    max_age: Option<Value>,
    min_dead_per_mille: Value,
) -> Result<Policy, Error> {
    let (max_bytes, max_records) = (
        threshold(max_bytes, "max_bytes")?,
        threshold(max_records, "max_records")?,
    );
    let max_age =
        threshold(max_age, "max_age")?.map(|age| std::time::Duration::from_nanos(age.get()));
    let per_mille: u64 = integer(min_dead_per_mille, "min_dead_per_mille")?;
    u16::try_from(per_mille)
        .ok()
        .and_then(|per_mille| Policy::new(max_bytes, max_records, max_age, per_mille))
        .ok_or_else(|| {
            Error::new(
                ruby.exception_range_error(),
                "min_dead_per_mille must be within 0..=1000",
            )
        })
}

/// A `Policy` the journal is opened with: exactly a `JournalMachines::Policy`
/// (`TypeError` otherwise, a subclass included), its four fields read
/// from the struct and checked again by the core's `Policy::new`
/// (`RangeError`), so a forged one never reaches the core.
pub fn policy_from(ruby: &Ruby, value: Value) -> Result<Policy, Error> {
    let class = classes(ruby)?.policy.get(ruby);
    let [max_bytes, max_records, max_age, per_mille] =
        exact_fields::<4>(ruby, value, class, "expected a JournalMachines::Policy")?;
    let optional = |value: Value| (!value.is_nil()).then_some(value);
    policy(
        ruby,
        optional(max_bytes),
        optional(max_records),
        optional(max_age),
        per_mille,
    )
}

/// The Ruby symbol for a refusal, or the one `name` gives the honoured answer.
pub fn answer<T>(
    ruby: &Ruby,
    outcome: Result<T, Refusal>,
    name: impl FnOnce(T) -> &'static str,
) -> Symbol {
    symbol(ruby, outcome.map_or_else(refusal, name))
}

/// Ruby symbol name of a refusal: the snake case of the core variant.
pub const fn refusal(refusal: Refusal) -> &'static str {
    match refusal {
        Refusal::NoActive => "no_active",
        Refusal::Empty => "empty",
        Refusal::TimeRegression => "time_regression",
        Refusal::Overflow => "overflow",
        Refusal::SegmentsFull => "segments_full",
        Refusal::PinsFull => "pins_full",
        Refusal::Exhausted => "exhausted",
        Refusal::Unknown => "unknown",
        Refusal::Active => "active",
        Refusal::Busy => "busy",
        Refusal::Exceeds => "exceeds",
        Refusal::Mismatch => "mismatch",
        Refusal::Stale => "stale",
        Refusal::WrongKind => "wrong_kind",
        Refusal::Unassigned => "unassigned",
        Refusal::StaleAssignment => "stale_assignment",
        Refusal::Regressed => "regressed",
        Refusal::NotDurable => "not_durable",
        Refusal::Pinned { .. } => "pinned",
    }
}
