//! The binding's answers as the gem's own `Data` values, built from what the
//! core returned with the classes resolved at init.
use journal_machines::{
    Reclaimed, Refusal, Retention, Roll, RollReason, Rolled, Segment, SegmentState,
};
use magnus::{Error, IntoValue, RClass, Ruby, Value, prelude::*};
use matryona_machines::ruby::{SymbolNames, Symbols, nanoseconds};

use crate::{ReplacementHandle, convert::Refusals};

/// The refusal's symbol, or what `build` makes of the honoured answer.
pub fn reply<T>(
    ruby: &Ruby,
    outcome: Result<T, Refusal>,
    build: impl FnOnce(T) -> Result<Value, Error>,
) -> Result<Value, Error> {
    outcome.map_or_else(
        |refusal| Ok(Refusals::symbol(ruby, &refusal).as_value()),
        build,
    )
}

matryona_machines::symbols! {
    /// The core's roll reasons as Symbols, in the order they are checked.
    pub Reasons for RollReason as "reason" {
        Size => "size",
        Records => "records",
        Age => "age",
    }
}

matryona_machines::symbols! {
    /// The core's segment states as Symbols.
    pub States for SegmentState as "state" {
        Active => "active",
        Sealed => "sealed",
        Proposed => "proposed",
        Durable => "durable",
        Reclaimable => "reclaimable",
    }
}

matryona_machines::symbols! {
    /// The core's retentions as Symbols.
    pub Retentions for Retention as "retention" {
        Unpinned => "unpinned",
        Pinned => "pinned",
    }
}

matryona_machines::symbols! {
    /// The core's roll decisions by name: `Keep` answers its Symbol, `Due`
    /// answers a `Due` value instead and its name is never sent.
    pub Rolls for Roll as "roll decision" => names_only {
        Keep => "keep",
        Due(..) => "due",
    }
}

pub fn rolled(class: RClass, rolled: Rolled) -> Result<Value, Error> {
    let fields = (
        rolled.sealed().raw(),
        rolled.opened().raw(),
        rolled.next_offset(),
    );
    class.funcall("new", fields)
}

pub fn reclaimed(class: RClass, reclaimed: Reclaimed) -> Result<Value, Error> {
    let fields = (
        reclaimed.segment().raw(),
        reclaimed.revision(),
        reclaimed.bytes(),
        reclaimed.replaced_by(),
    );
    class.funcall("new", fields)
}

pub fn segment(ruby: &Ruby, class: RClass, segment: Segment) -> Result<Value, Error> {
    let replacement = segment
        .replacement()
        .map(|replacement| ReplacementHandle { replacement });
    let state = States::symbol(ruby, segment.state());
    let fields: [Value; 11] = [
        segment.id().raw().into_value_with(ruby),
        segment.revision().into_value_with(ruby),
        segment.base_offset().into_value_with(ruby),
        segment.records().into_value_with(ruby),
        segment.stored_records().into_value_with(ruby),
        segment.live_records().into_value_with(ruby),
        segment.bytes().into_value_with(ruby),
        nanoseconds(ruby, segment.opened_at()).as_value(),
        state.as_value(),
        Retentions::symbol(ruby, segment.retention()).as_value(),
        replacement.into_value_with(ruby),
    ];
    class.funcall("new", fields)
}
