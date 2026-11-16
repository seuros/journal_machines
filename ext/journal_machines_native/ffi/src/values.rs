//! Private hooks the Ruby `Data` values validate with: each builds the core
//! value and raises where the core's constructor answers `None`.
use journal_machines::{Reclaimed, Rolled, Segment, SegmentId};
use magnus::{Error, Ruby, Value};
use matryona_machines::ruby::{
    Symbols, duration, exact_array_n, integer, optional, optional_typed,
};

use crate::{
    ReplacementHandle,
    answers::{Reasons, Retentions, States},
    classes::classes,
};

fn range(ruby: &Ruby, message: &'static str) -> Error {
    Error::new(ruby.exception_range_error(), message)
}

fn id(value: Value, name: &str) -> Result<SegmentId, Error> {
    integer(value, name).map(SegmentId::from_raw)
}

/// `Due.new`: the core's `Roll::Due(reason)` for one of its three reasons.
pub fn check_due(ruby: &Ruby, value: Value) -> Result<(), Error> {
    Reasons::parse(ruby, value).map(drop)
}

/// `Rolled.new`: the core's `Rolled::new`.
pub fn check_rolled(
    ruby: &Ruby,
    sealed: Value,
    opened: Value,
    next_offset: Value,
) -> Result<(), Error> {
    let (sealed, opened) = (id(sealed, "sealed")?, id(opened, "opened")?);
    Rolled::new(sealed, opened, integer(next_offset, "next_offset")?)
        .map(drop)
        .ok_or_else(|| range(ruby, "no journal records this roll"))
}

/// `Reclaimed.new`: the core's `Reclaimed::new`.
pub fn check_reclaimed(
    ruby: &Ruby,
    segment: Value,
    revision: Value,
    bytes: Value,
    replaced_by: Value,
) -> Result<(), Error> {
    let (segment, revision) = (id(segment, "segment")?, integer(revision, "revision")?);
    let bytes = integer(bytes, "bytes")?;
    let replaced_by = optional(replaced_by, "replaced_by")?;
    Reclaimed::new(segment, revision, bytes, replaced_by)
        .map(drop)
        .ok_or_else(|| range(ruby, "no journal answers this reclaim"))
}

/// `Segment.new`: the core's `Segment::checked`. `head` is id, revision,
/// base offset and records; `counts` stored and live records, bytes and the
/// opening instant in nanoseconds.
pub fn check_segment(
    ruby: &Ruby,
    head: Value,
    counts: Value,
    state_name: Value,
    retention: Value,
    replacement: Value,
) -> Result<(), Error> {
    let [id_value, revision, base_offset, records] =
        exact_array_n::<4>(ruby, head, "head must be an Array of four")?;
    let [stored_records, live_records, bytes, opened_at] =
        exact_array_n::<4>(ruby, counts, "counts must be an Array of four")?;
    let (id, revision) = (id(id_value, "id")?, integer(revision, "revision")?);
    let base_offset = integer(base_offset, "base_offset")?;
    let records = integer(records, "records")?;
    let stored_records = integer(stored_records, "stored_records")?;
    let live_records = integer(live_records, "live_records")?;
    let (bytes, opened_at) = (integer(bytes, "bytes")?, duration(opened_at, "opened_at")?);
    // After the Integers, as when magnus converted the arguments in order.
    let class = classes(ruby)?.replacement.get(ruby);
    let message = "replacement must be a JournalMachines::Replacement or nil";
    let replacement = optional_typed::<ReplacementHandle>(ruby, replacement, class, message)?;
    Segment::new(
        id,
        revision,
        base_offset,
        records,
        stored_records,
        live_records,
        bytes,
        opened_at,
        States::parse(ruby, state_name)?,
        Retentions::parse(ruby, retention)?,
        replacement.map(|handle| handle.replacement),
    )
    .map(drop)
    .ok_or_else(|| range(ruby, "no journal answers this segment"))
}
