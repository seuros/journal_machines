//! Thin binding to the bounded segment metadata journal.
mod answers;
mod classes;
mod convert;
mod handles;
mod init;
mod values;

use answers::reply;
use classes::classes;
use convert::{answer, policy, policy_from};
use handles::{PinHandle, ReplacementHandle};

use std::{cell::RefCell, num::NonZeroU64};

use journal_machines::{
    Advanced, Checkpointed, Confirmed, Durable, Pin, Refusal, Replacement, Roll, Segment, SegmentId,
};
use magnus::{Error, Integer, IntoValue, RArray, Ruby, Symbol, Value, prelude::*, typed_data::Obj};
use matryona_machines::ruby::{
    Symbols, duration, exact_array_n, exact_typed, integer, keep, nanoseconds, symbol,
};

/// The pin exactly a `Pin` handle names; `TypeError` for anything else, a
/// subclass instance included.
fn pin(ruby: &Ruby, value: Value) -> Result<Pin, Error> {
    let class = classes(ruby)?.pin.get(ruby);
    let message = "expected a JournalMachines::Pin";
    exact_typed::<PinHandle>(ruby, value, class, message).map(|handle| handle.pin)
}

/// The attempt exactly a `Replacement` handle names; `TypeError` for
/// anything else, a subclass instance included.
fn replacement(ruby: &Ruby, value: Value) -> Result<Replacement, Error> {
    let class = classes(ruby)?.replacement.get(ruby);
    let message = "expected a JournalMachines::Replacement";
    exact_typed::<ReplacementHandle>(ruby, value, class, message).map(|handle| handle.replacement)
}

/// A segment id given as an Integer named `name`.
fn id(value: Value, name: &str) -> Result<SegmentId, Error> {
    integer(value, name).map(SegmentId::from_raw)
}

const SEGMENTS: usize = 64;

/// Which of the core's three segment iterators a scan reads.
#[derive(Clone, Copy)]
enum Scan {
    All,
    Candidates,
    Reclaimable,
}
const PINS: usize = 32;

#[magnus::wrap(class = "JournalMachines::Journal", free_immediately, size)]
struct Journal {
    core: RefCell<journal_machines::Journal<SEGMENTS, PINS>>,
}

impl Journal {
    /// `Journal::resumed`; `Journal::with_scope` is the same with nothing
    /// issued. Raises `RangeError` where the core answers `None`.
    fn open(
        ruby: &Ruby,
        base_offset: Value,
        now: Value,
        pair: Value,
        policy: Value,
    ) -> Result<Self, Error> {
        let (base, now) = (integer(base_offset, "base_offset")?, duration(now, "now")?);
        let [scope, issued] =
            exact_array_n::<2>(ruby, pair, "scope and issued must be an Array of two")?;
        let (scope, issued) = (integer(scope, "scope")?, integer(issued, "issued")?);
        let policy = policy_from(ruby, policy)?;
        journal_machines::Journal::resumed(policy, base, now, scope, issued)
            .map(|core| Self {
                core: RefCell::new(core),
            })
            .ok_or_else(|| {
                Error::new(
                    ruby.exception_range_error(),
                    "issued must leave an identity for the active segment: 0..2**48-2",
                )
            })
    }

    fn scope(&self) -> u16 {
        self.core.borrow().scope()
    }

    fn issued(&self) -> u64 {
        self.core.borrow().issued()
    }

    /// Raises where `Policy::new` answers `None`; `Policy.new` calls it.
    fn check_policy(
        ruby: &Ruby,
        max_bytes: Option<Value>,
        max_records: Option<Value>,
        max_age: Option<Value>,
        min_dead_per_mille: Value,
    ) -> Result<(), Error> {
        policy(ruby, max_bytes, max_records, max_age, min_dead_per_mille).map(drop)
    }

    /// The four `Policy` fields, in order.
    fn policy(ruby: &Ruby, this: &Self) -> (Option<u64>, Option<u64>, Option<Integer>, u16) {
        let policy = this.core.borrow().policy();
        (
            policy.max_bytes().map(NonZeroU64::get),
            policy.max_records().map(NonZeroU64::get),
            policy.max_age().map(|age| nanoseconds(ruby, age)),
            policy.min_dead_per_mille(),
        )
    }

    fn size(&self) -> usize {
        self.core.borrow().len()
    }

    fn is_empty(&self) -> bool {
        self.core.borrow().is_empty()
    }

    /// `:keep`, a `Due` naming the threshold reached, or a refusal.
    fn roll_decision(
        ruby: &Ruby,
        this: &Self,
        bytes: Value,
        records: Value,
        now: Value,
    ) -> Result<Value, Error> {
        let (bytes, records) = (integer(bytes, "bytes")?, integer(records, "records")?);
        let now = duration(now, "now")?;
        let due = classes(ruby)?.due.get(ruby);
        let decision = this.core.borrow().roll_decision(bytes, records, now);
        reply(ruby, decision, |roll| match roll {
            Roll::Keep => Ok(symbol(ruby, "keep").as_value()),
            Roll::Due(reason) => due.funcall("new", (answers::Reasons::symbol(ruby, reason),)),
        })
    }

    fn roll(
        ruby: &Ruby,
        this: Obj<Self>,
        bytes: Value,
        records: Value,
        now: Value,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let (bytes, records) = (integer(bytes, "bytes")?, integer(records, "records")?);
        let now = duration(now, "now")?;
        let class = classes(ruby)?.rolled.get(ruby);
        keep(
            ruby,
            this,
            &this.core,
            |core| core.roll(bytes, records, now),
            |rolled| reply(ruby, rolled, |rolled| answers::rolled(class, rolled)),
        )
    }

    fn observe_live(ruby: &Ruby, this: Obj<Self>, id: Value, live: Value) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let (id, live) = (self::id(id, "id")?, integer(live, "live_records")?);
        let outcome = this.core.borrow_mut().observe_live(id, live);
        Ok(answer(ruby, outcome, |()| "recorded"))
    }

    /// One read of the journal answered as a `Segment` or `nil`.
    fn view(
        ruby: &Ruby,
        this: &Self,
        read: impl FnOnce(&journal_machines::Journal<SEGMENTS, PINS>) -> Option<Segment>,
    ) -> Result<Option<Value>, Error> {
        let class = classes(ruby)?.segment.get(ruby);
        let segment = read(&this.core.borrow());
        segment
            .map(|segment| answers::segment(ruby, class, segment))
            .transpose()
    }

    fn active(ruby: &Ruby, this: &Self) -> Result<Option<Value>, Error> {
        Self::view(ruby, this, journal_machines::Journal::active)
    }

    fn segment(ruby: &Ruby, this: &Self, id: Value) -> Result<Option<Value>, Error> {
        let id = self::id(id, "id")?;
        Self::view(ruby, this, |core| core.segment(id))
    }

    fn covering(ruby: &Ruby, this: &Self, offset: Value) -> Result<Option<Value>, Error> {
        let offset = integer(offset, "offset")?;
        Self::view(ruby, this, |core| core.covering(offset))
    }

    /// At most `limit` rows (clamped to the capacity) of one scan after
    /// segment `after`. The views are read into a Rust buffer first, so no
    /// Ruby value is built while the journal is borrowed.
    fn scan(
        ruby: &Ruby,
        this: &Self,
        after: Option<Value>,
        limit: Value,
        scan: Scan,
    ) -> Result<RArray, Error> {
        let after = after.map(|after| id(after, "after")).transpose()?;
        let limit: u64 = integer(limit, "limit")?;
        let take = usize::try_from(limit).map_or(SEGMENTS, |limit| limit.min(SEGMENTS));
        let class = classes(ruby)?.segment.get(ruby);
        let views: Vec<Segment> = {
            let core = this.core.borrow();
            match scan {
                Scan::All => core.segments(after).take(take).collect(),
                Scan::Candidates => core.compaction_candidates(after).take(take).collect(),
                Scan::Reclaimable => core.reclaimable(after).take(take).collect(),
            }
        };
        let rows = ruby.ary_new_capa(views.len());
        for segment in views {
            rows.push(answers::segment(ruby, class, segment)?)?;
        }
        Ok(rows)
    }

    fn segments(
        ruby: &Ruby,
        this: &Self,
        after: Option<Value>,
        limit: Value,
    ) -> Result<RArray, Error> {
        Self::scan(ruby, this, after, limit, Scan::All)
    }

    fn compaction_candidates(
        ruby: &Ruby,
        this: &Self,
        after: Option<Value>,
        limit: Value,
    ) -> Result<RArray, Error> {
        Self::scan(ruby, this, after, limit, Scan::Candidates)
    }

    fn reclaimable(
        ruby: &Ruby,
        this: &Self,
        after: Option<Value>,
        limit: Value,
    ) -> Result<RArray, Error> {
        Self::scan(ruby, this, after, limit, Scan::Reclaimable)
    }

    fn pinned(ruby: &Ruby, pin: Result<Pin, Refusal>) -> Result<Value, Error> {
        reply(ruby, pin, |pin| Ok(PinHandle { pin }.into_value_with(ruby)))
    }

    fn pin_reader(ruby: &Ruby, this: Obj<Self>, offset: Value) -> Result<Value, Error> {
        this.check_frozen()?;
        let offset = integer(offset, "offset")?;
        let pin = this.core.borrow_mut().pin_reader(offset);
        Self::pinned(ruby, pin)
    }

    fn pin_checkpoint(
        ruby: &Ruby,
        this: Obj<Self>,
        assignment: Value,
        offset: Value,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let assignment = integer(assignment, "assignment")?;
        let offset = integer(offset, "offset")?;
        let pin = this.core.borrow_mut().pin_checkpoint(assignment, offset);
        Self::pinned(ruby, pin)
    }

    fn advance(
        ruby: &Ruby,
        this: Obj<Self>,
        handle: Value,
        offset: Value,
    ) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let (pin, offset) = (pin(ruby, handle)?, integer(offset, "offset")?);
        let outcome = this.core.borrow_mut().advance(pin, offset);
        Ok(answer(ruby, outcome, |advanced| match advanced {
            Advanced::Unchanged => "unchanged",
            Advanced::Moved => "moved",
        }))
    }

    fn checkpoint(
        ruby: &Ruby,
        this: Obj<Self>,
        handle: Value,
        assignment: Value,
        offset: Value,
    ) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let (pin, assignment) = (pin(ruby, handle)?, integer(assignment, "assignment")?);
        let offset = integer(offset, "offset")?;
        let outcome = this.core.borrow_mut().checkpoint(pin, assignment, offset);
        Ok(answer(ruby, outcome, |checkpointed| match checkpointed {
            Checkpointed::Reassigned => "reassigned",
            Checkpointed::Unchanged => "unchanged",
            Checkpointed::Advanced => "advanced",
        }))
    }

    fn release(ruby: &Ruby, this: Obj<Self>, handle: Value) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let pin = pin(ruby, handle)?;
        let outcome = this.core.borrow_mut().release(pin);
        Ok(answer(ruby, outcome, |()| "released"))
    }

    fn pin_offset(ruby: &Ruby, this: &Self, handle: Value) -> Result<Option<u64>, Error> {
        let pin = pin(ruby, handle)?;
        Ok(this.core.borrow().pin_offset(pin))
    }

    fn low_pin(&self) -> Option<u64> {
        self.core.borrow().low_pin()
    }

    fn pins(&self) -> usize {
        self.core.borrow().pins()
    }

    fn propose(ruby: &Ruby, this: Obj<Self>, id: Value) -> Result<Value, Error> {
        this.check_frozen()?;
        let id = self::id(id, "id")?;
        let proposed = this.core.borrow_mut().propose(id);
        reply(ruby, proposed, |replacement| {
            Ok(ReplacementHandle { replacement }.into_value_with(ruby))
        })
    }

    /// Called once the receiver is known mutable and the arguments read.
    fn confirm(ruby: &Ruby, this: Obj<Self>, handle: Replacement, durable: Durable) -> Symbol {
        let outcome = this.core.borrow_mut().confirm_durable(handle, durable);
        answer(ruby, outcome, |confirmed| match confirmed {
            Confirmed::AlreadyDurable => "already_durable",
            Confirmed::Recorded => "recorded",
        })
    }

    fn confirm_rewritten(
        ruby: &Ruby,
        this: Obj<Self>,
        handle: Value,
        bytes: Value,
        live_records: Value,
    ) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let handle = replacement(ruby, handle)?;
        let durable = Durable::Rewritten {
            bytes: integer(bytes, "bytes")?,
            live_records: integer(live_records, "live_records")?,
        };
        Ok(Self::confirm(ruby, this, handle, durable))
    }

    fn confirm_superseded(ruby: &Ruby, this: Obj<Self>, handle: Value) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let handle = replacement(ruby, handle)?;
        Ok(Self::confirm(ruby, this, handle, Durable::Superseded))
    }

    fn reclaim(ruby: &Ruby, this: Obj<Self>, handle: Value) -> Result<Value, Error> {
        this.check_frozen()?;
        let handle = replacement(ruby, handle)?;
        let class = classes(ruby)?.reclaimed.get(ruby);
        keep(
            ruby,
            this,
            &this.core,
            |core| core.reclaim(handle),
            |outcome| {
                reply(ruby, outcome, |reclaimed| {
                    answers::reclaimed(class, reclaimed)
                })
            },
        )
    }

    fn abort(ruby: &Ruby, this: Obj<Self>, handle: Value) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let handle = replacement(ruby, handle)?;
        let outcome = this.core.borrow_mut().abort(handle);
        Ok(answer(ruby, outcome, |()| "aborted"))
    }
}
