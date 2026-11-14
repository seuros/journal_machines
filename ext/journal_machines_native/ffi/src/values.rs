//! Private hooks the Ruby `Data` values validate with: each builds the core
//! value and raises where the core's constructor answers `None`.
use journal_machines::{Reclaimed, RollReason, Rolled, Segment, SegmentId, SegmentState};
use magnus::{Error, Ruby, Symbol, Value, prelude::*, typed_data::Obj};

use crate::{ReplacementHandle, convert::Number};

/// The name of a Symbol; `TypeError` for anything else.
fn symbol(ruby: &Ruby, value: Value) -> Result<String, Error> {
    let Some(symbol) = Symbol::from_value(value) else {
        return Err(wrong_type(ruby, "expected a Symbol"));
    };
    Ok(symbol.name()?.into_owned())
}

/// `true` or `false`; `TypeError` for anything else, `nil` included.
fn flag(ruby: &Ruby, value: Value) -> Result<bool, Error> {
    if value.is_kind_of(ruby.class_true_class()) {
        Ok(true)
    } else if value.is_kind_of(ruby.class_false_class()) {
        Ok(false)
    } else {
        Err(Error::new(
            ruby.exception_type_error(),
            "expected true or false",
        ))
    }
}

pub fn wrong_type(ruby: &Ruby, message: &'static str) -> Error {
    Error::new(ruby.exception_type_error(), message)
}

fn range(ruby: &Ruby, message: &'static str) -> Error {
    Error::new(ruby.exception_range_error(), message)
}

/// The core reason a symbol names; `RangeError` for any other Symbol.
fn reason(ruby: &Ruby, value: Value) -> Result<RollReason, Error> {
    match symbol(ruby, value)?.as_str() {
        "size" => Ok(RollReason::Size),
        "records" => Ok(RollReason::Records),
        "age" => Ok(RollReason::Age),
        _ => Err(range(ruby, "a roll reason is :size, :records or :age")),
    }
}

fn state(ruby: &Ruby, value: Value) -> Result<SegmentState, Error> {
    match symbol(ruby, value)?.as_str() {
        "active" => Ok(SegmentState::Active),
        "sealed" => Ok(SegmentState::Sealed),
        "proposed" => Ok(SegmentState::Proposed),
        "durable" => Ok(SegmentState::Durable),
        "reclaimable" => Ok(SegmentState::Reclaimable),
        _ => Err(range(
            ruby,
            "a segment state is :active, :sealed, :proposed, :durable or :reclaimable",
        )),
    }
}

/// `Due.new`: the core's `Roll::Due(reason)` for one of its three reasons.
pub fn check_due(ruby: &Ruby, value: Value) -> Result<(), Error> {
    reason(ruby, value).map(|_| ())
}

/// `Rolled.new`: the core's `Rolled::new`.
pub fn check_rolled(
    ruby: &Ruby,
    sealed: Number,
    opened: Number,
    next_offset: Number,
) -> Result<(), Error> {
    let (sealed, opened) = (SegmentId::from_raw(sealed.0), SegmentId::from_raw(opened.0));
    Rolled::new(sealed, opened, next_offset.0)
        .map(|_| ())
        .ok_or_else(|| range(ruby, "no journal records this roll"))
}

/// `Reclaimed.new`: the core's `Reclaimed::new`.
pub fn check_reclaimed(
    ruby: &Ruby,
    segment: Number,
    revision: Number,
    bytes: Number,
    replaced_by: Option<Number>,
) -> Result<(), Error> {
    let segment = SegmentId::from_raw(segment.0);
    Reclaimed::new(segment, revision.0, bytes.0, replaced_by.map(|by| by.0))
        .map(|_| ())
        .ok_or_else(|| range(ruby, "no journal answers this reclaim"))
}

/// `Segment.new`: the core's `Segment::checked`. `head` is id, revision,
/// base offset and records; `counts` stored and live records, bytes and the
/// opening instant in nanoseconds.
pub fn check_segment(
    ruby: &Ruby,
    head: (Number, Number, Number, Number),
    counts: (Number, Number, Number, Number),
    state_name: Value,
    pinned: Value,
    replacement: Option<Obj<ReplacementHandle>>,
) -> Result<(), Error> {
    let (id, revision, base_offset, records) = head;
    let (stored_records, live_records, bytes, opened_at) = counts;
    Segment::new(
        SegmentId::from_raw(id.0),
        revision.0,
        base_offset.0,
        records.0,
        stored_records.0,
        live_records.0,
        bytes.0,
        opened_at.nanos(),
        state(ruby, state_name)?,
        flag(ruby, pinned)?,
        replacement.map(|handle| handle.replacement),
    )
    .map(|_| ())
    .ok_or_else(|| range(ruby, "no journal answers this segment"))
}
