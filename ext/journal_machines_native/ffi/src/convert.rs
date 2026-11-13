//! Ruby value conversion and refusal names.
use std::{num::NonZeroU64, time::Duration};

use journal_machines::{Policy, Refusal};
use magnus::{Error, Integer, Ruby, Symbol, TryConvert, Value};

#[derive(Clone, Copy)]
pub struct Number(pub u64);

impl TryConvert for Number {
    fn try_convert(value: Value) -> Result<Self, Error> {
        if Integer::from_value(value).is_none() {
            return Err(Error::new(
                Ruby::get_with(value).exception_type_error(),
                "expected an Integer (unsigned 64-bit)",
            ));
        }
        u64::try_convert(value).map(Self)
    }
}

/// A scope: an Integer in `0..=65535`.
#[derive(Clone, Copy)]
pub struct Scope(pub u16);

impl TryConvert for Scope {
    fn try_convert(value: Value) -> Result<Self, Error> {
        let ruby = Ruby::get_with(value);
        if Integer::from_value(value).is_none() {
            return Err(Error::new(
                ruby.exception_type_error(),
                "scope must be an Integer",
            ));
        }
        u16::try_convert(value)
            .map(Self)
            .map_err(|_| Error::new(ruby.exception_range_error(), "scope must be in 0..=65535"))
    }
}

impl Number {
    /// A monotonic instant or span given as Integer nanoseconds.
    pub const fn nanos(self) -> Duration {
        Duration::from_nanos(self.0)
    }
}

/// Integer nanoseconds of an instant the binding was handed as nanoseconds.
pub fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn threshold(ruby: &Ruby, limit: Option<Number>) -> Result<Option<NonZeroU64>, Error> {
    limit.map_or(Ok(None), |limit| {
        NonZeroU64::new(limit.0).map(Some).ok_or_else(|| {
            Error::new(
                ruby.exception_range_error(),
                "a roll threshold must be positive, or nil to disable it",
            )
        })
    })
}

/// The core policy for the four `Policy` fields as Ruby passes them; raises
/// `RangeError` where the core constructor answers `None`.
pub fn policy(
    ruby: &Ruby,
    max_bytes: Option<Number>,
    max_records: Option<Number>,
    max_age: Option<Number>,
    min_dead_per_mille: Number,
) -> Result<Policy, Error> {
    let (max_bytes, max_records, max_age) = (
        threshold(ruby, max_bytes)?,
        threshold(ruby, max_records)?,
        threshold(ruby, max_age)?.map(|age| Number(age.get()).nanos()),
    );
    u16::try_from(min_dead_per_mille.0)
        .ok()
        .and_then(|per_mille| Policy::new(max_bytes, max_records, max_age, per_mille))
        .ok_or_else(|| {
            Error::new(
                ruby.exception_range_error(),
                "min_dead_per_mille must be within 0..=1000",
            )
        })
}

/// The Ruby symbol for a refusal; a done call answers `done` instead.
pub fn answer<T>(ruby: &Ruby, outcome: Result<T, Refusal>, done: &'static str) -> Symbol {
    ruby.to_symbol(outcome.map_or_else(refusal, |_| done))
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
