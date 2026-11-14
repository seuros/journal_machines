//! Ruby value conversion and refusal names.
use std::num::NonZeroU64;

use journal_machines::{Policy, Refusal};
use magnus::{Error, Ruby, Symbol, Value};
use matryona_machines::ruby::{SymbolNames, exact_fields, integer_in, optional_positive, symbol};

use crate::classes::classes;

/// The core policy for the four `Policy` fields as Ruby passes them. Each is
/// converted first, in order: a threshold is `nil` or a positive Integer,
/// the ratio an Integer in `0..=1000`. The core's `Policy::new` then checks
/// them again (`RangeError` where it answers `None`).
pub fn policy(
    ruby: &Ruby,
    max_bytes: Value,
    max_records: Value,
    max_age: Value,
    min_dead_per_mille: Value,
) -> Result<Policy, Error> {
    let max_bytes = optional_positive::<NonZeroU64>(max_bytes, "max_bytes")?;
    let max_records = optional_positive::<NonZeroU64>(max_records, "max_records")?;
    let max_age = optional_positive::<NonZeroU64>(max_age, "max_age")?
        .map(|age| std::time::Duration::from_nanos(age.get()));
    let per_mille = integer_in(min_dead_per_mille, "min_dead_per_mille", 0..=1000)?;
    Policy::new(max_bytes, max_records, max_age, per_mille).ok_or_else(|| {
        Error::new(
            ruby.exception_range_error(),
            "no journal answers this policy",
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
    policy(ruby, max_bytes, max_records, max_age, per_mille)
}

matryona_machines::symbols! {
    /// The core's refusals as Symbols: the snake case of each variant.
    pub Refusals for Refusal as "a refusal" => names_only {
        NoActive => "no_active",
        Empty => "empty",
        TimeRegression => "time_regression",
        Overflow => "overflow",
        SegmentsFull => "segments_full",
        PinsFull => "pins_full",
        Exhausted => "exhausted",
        Unknown => "unknown",
        Active => "active",
        Busy => "busy",
        Exceeds => "exceeds",
        Mismatch => "mismatch",
        Stale => "stale",
        WrongKind => "wrong_kind",
        Unassigned => "unassigned",
        StaleAssignment => "stale_assignment",
        Regressed => "regressed",
        NotDurable => "not_durable",
        Pinned { .. } => "pinned",
    }
}

/// The Ruby symbol for a refusal, or the one `name` gives the honoured answer.
pub fn answer<T>(
    ruby: &Ruby,
    outcome: Result<T, Refusal>,
    name: impl FnOnce(T) -> &'static str,
) -> Symbol {
    symbol(
        ruby,
        outcome.map_or_else(|refusal| Refusals::name(&refusal), name),
    )
}
