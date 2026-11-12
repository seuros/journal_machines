//! Ruby value conversion and refusal names.
use std::num::NonZeroU64;

use journal_machines::{PinUpdate, RollError};
use magnus::{Error, Integer, Ruby, TryConvert, Value};

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

pub fn threshold(ruby: &Ruby, limit: Option<Number>) -> Result<Option<NonZeroU64>, Error> {
    limit.map_or(Ok(None), |limit| {
        NonZeroU64::new(limit.0).map(Some).ok_or_else(|| {
            Error::new(
                ruby.exception_range_error(),
                "a roll threshold must be positive, or nil to disable it",
            )
        })
    })
}

pub const fn roll_error(error: RollError) -> &'static str {
    match error {
        RollError::Empty => "empty",
        RollError::TimeRegression => "time_regression",
        RollError::Full => "full",
        RollError::Overflow => "overflow",
        _ => "no_active",
    }
}

pub const fn update(update: PinUpdate) -> &'static str {
    match update {
        PinUpdate::Advanced => "advanced",
        PinUpdate::Unchanged => "unchanged",
        PinUpdate::Reassigned => "reassigned",
        PinUpdate::Regressed => "regressed",
        PinUpdate::StaleAssignment => "stale_assignment",
        PinUpdate::Unassigned => "unassigned",
        PinUpdate::WrongKind => "wrong_kind",
        _ => "stale",
    }
}
