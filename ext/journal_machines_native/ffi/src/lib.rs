//! Thin binding to the bounded segment metadata journal.
mod answers;
mod classes;
mod convert;
mod handles;
mod init;
mod values;

use answers::reply;
use classes::{classes, keep};
use convert::{Number, Scope, answer, nanos, policy, policy_from};
use handles::{PinHandle, ReplacementHandle};

use std::{cell::RefCell, num::NonZeroU64};

use journal_machines::{
    Advanced, Checkpointed, Confirmed, Durable, Pin, Refusal, Roll, Segment, SegmentId,
};
use magnus::{Error, IntoValue, RArray, Ruby, Symbol, Value, prelude::*, typed_data::Obj};

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
        base_offset: Number,
        now: Number,
        (scope, issued): (Scope, Number),
        policy: Value,
    ) -> Result<Self, Error> {
        let policy = policy_from(ruby, policy)?;
        let (base, now) = (base_offset.0, now.nanos());
        journal_machines::Journal::resumed(policy, base, now, scope.0, issued.0)
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
        max_bytes: Option<Number>,
        max_records: Option<Number>,
        max_age: Option<Number>,
        min_dead_per_mille: Number,
    ) -> Result<(), Error> {
        policy(ruby, max_bytes, max_records, max_age, min_dead_per_mille).map(|_| ())
    }

    /// The four `Policy` fields, in order.
    fn policy(&self) -> (Option<u64>, Option<u64>, Option<u64>, u16) {
        let policy = self.core.borrow().policy();
        (
            policy.max_bytes().map(NonZeroU64::get),
            policy.max_records().map(NonZeroU64::get),
            policy.max_age().map(nanos),
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
        bytes: Number,
        records: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let due = classes(ruby)?.due;
        let decision = this
            .core
            .borrow()
            .roll_decision(bytes.0, records.0, now.nanos());
        reply(ruby, decision, |roll| match roll {
            Roll::Keep => Ok(ruby.to_symbol("keep").as_value()),
            Roll::Due(reason) => due.funcall("new", (ruby.to_symbol(answers::reason(reason)),)),
        })
    }

    fn roll(
        ruby: &Ruby,
        this: Obj<Self>,
        bytes: Number,
        records: Number,
        now: Number,
    ) -> Result<Value, Error> {
        let class = classes(ruby)?.rolled;
        keep(ruby, this.as_value(), &this.core, |core| {
            let rolled = core.roll(bytes.0, records.0, now.nanos());
            reply(ruby, rolled, |rolled| answers::rolled(class, rolled))
        })
    }

    fn observe_live(
        ruby: &Ruby,
        this: Obj<Self>,
        id: Number,
        live: Number,
    ) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let outcome = this
            .core
            .borrow_mut()
            .observe_live(SegmentId::from_raw(id.0), live.0);
        Ok(answer(ruby, outcome, |()| "recorded"))
    }

    /// One read of the journal answered as a `Segment` or `nil`.
    fn view(
        ruby: &Ruby,
        this: &Self,
        read: impl FnOnce(&journal_machines::Journal<SEGMENTS, PINS>) -> Option<Segment>,
    ) -> Result<Option<Value>, Error> {
        let class = classes(ruby)?.segment;
        let segment = read(&this.core.borrow());
        segment
            .map(|segment| answers::segment(ruby, class, segment))
            .transpose()
    }

    fn active(ruby: &Ruby, this: &Self) -> Result<Option<Value>, Error> {
        Self::view(ruby, this, journal_machines::Journal::active)
    }

    fn segment(ruby: &Ruby, this: &Self, id: Number) -> Result<Option<Value>, Error> {
        Self::view(ruby, this, |core| core.segment(SegmentId::from_raw(id.0)))
    }

    fn covering(ruby: &Ruby, this: &Self, offset: Number) -> Result<Option<Value>, Error> {
        Self::view(ruby, this, |core| core.covering(offset.0))
    }

    /// At most `limit` rows (clamped to the capacity) of one scan after
    /// segment `after`. The views are read into a Rust buffer first, so no
    /// Ruby value is built while the journal is borrowed.
    fn scan(
        ruby: &Ruby,
        this: &Self,
        after: Option<Number>,
        limit: Number,
        scan: Scan,
    ) -> Result<RArray, Error> {
        let take = usize::try_from(limit.0).map_or(SEGMENTS, |limit| limit.min(SEGMENTS));
        let after = after.map(|after| SegmentId::from_raw(after.0));
        let class = classes(ruby)?.segment;
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
        after: Option<Number>,
        limit: Number,
    ) -> Result<RArray, Error> {
        Self::scan(ruby, this, after, limit, Scan::All)
    }

    fn compaction_candidates(
        ruby: &Ruby,
        this: &Self,
        after: Option<Number>,
        limit: Number,
    ) -> Result<RArray, Error> {
        Self::scan(ruby, this, after, limit, Scan::Candidates)
    }

    fn reclaimable(
        ruby: &Ruby,
        this: &Self,
        after: Option<Number>,
        limit: Number,
    ) -> Result<RArray, Error> {
        Self::scan(ruby, this, after, limit, Scan::Reclaimable)
    }

    fn pinned(ruby: &Ruby, pin: Result<Pin, Refusal>) -> Result<Value, Error> {
        reply(ruby, pin, |pin| Ok(PinHandle { pin }.into_value_with(ruby)))
    }

    fn pin_reader(ruby: &Ruby, this: Obj<Self>, offset: Number) -> Result<Value, Error> {
        this.check_frozen()?;
        let pin = this.core.borrow_mut().pin_reader(offset.0);
        Self::pinned(ruby, pin)
    }

    fn pin_checkpoint(
        ruby: &Ruby,
        this: Obj<Self>,
        assignment: Number,
        offset: Number,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let pin = this
            .core
            .borrow_mut()
            .pin_checkpoint(assignment.0, offset.0);
        Self::pinned(ruby, pin)
    }

    fn advance(
        ruby: &Ruby,
        this: Obj<Self>,
        handle: Obj<PinHandle>,
        offset: Number,
    ) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let outcome = this.core.borrow_mut().advance(handle.pin, offset.0);
        Ok(answer(ruby, outcome, |advanced| match advanced {
            Advanced::Unchanged => "unchanged",
            Advanced::Moved => "moved",
        }))
    }

    fn checkpoint(
        ruby: &Ruby,
        this: Obj<Self>,
        handle: Obj<PinHandle>,
        assignment: Number,
        offset: Number,
    ) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let outcome = this
            .core
            .borrow_mut()
            .checkpoint(handle.pin, assignment.0, offset.0);
        Ok(answer(ruby, outcome, |checkpointed| match checkpointed {
            Checkpointed::Reassigned => "reassigned",
            Checkpointed::Unchanged => "unchanged",
            Checkpointed::Advanced => "advanced",
        }))
    }

    fn release(ruby: &Ruby, this: Obj<Self>, handle: Obj<PinHandle>) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let outcome = this.core.borrow_mut().release(handle.pin);
        Ok(answer(ruby, outcome, |()| "released"))
    }

    fn pin_offset(&self, handle: Obj<PinHandle>) -> Option<u64> {
        self.core.borrow().pin_offset(handle.pin)
    }

    fn low_pin(&self) -> Option<u64> {
        self.core.borrow().low_pin()
    }

    fn pins(&self) -> usize {
        self.core.borrow().pins()
    }

    fn propose(ruby: &Ruby, this: Obj<Self>, id: Number) -> Result<Value, Error> {
        this.check_frozen()?;
        let proposed = this.core.borrow_mut().propose(SegmentId::from_raw(id.0));
        reply(ruby, proposed, |replacement| {
            Ok(ReplacementHandle { replacement }.into_value_with(ruby))
        })
    }

    fn confirm(
        ruby: &Ruby,
        this: Obj<Self>,
        handle: Obj<ReplacementHandle>,
        durable: Durable,
    ) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let outcome = this
            .core
            .borrow_mut()
            .confirm_durable(handle.replacement, durable);
        Ok(answer(ruby, outcome, |confirmed| match confirmed {
            Confirmed::AlreadyDurable => "already_durable",
            Confirmed::Recorded => "recorded",
        }))
    }

    fn confirm_rewritten(
        ruby: &Ruby,
        this: Obj<Self>,
        handle: Obj<ReplacementHandle>,
        bytes: Number,
        live_records: Number,
    ) -> Result<Symbol, Error> {
        let durable = Durable::Rewritten {
            bytes: bytes.0,
            live_records: live_records.0,
        };
        Self::confirm(ruby, this, handle, durable)
    }

    fn confirm_superseded(
        ruby: &Ruby,
        this: Obj<Self>,
        handle: Obj<ReplacementHandle>,
    ) -> Result<Symbol, Error> {
        Self::confirm(ruby, this, handle, Durable::Superseded)
    }

    fn reclaim(
        ruby: &Ruby,
        this: Obj<Self>,
        handle: Obj<ReplacementHandle>,
    ) -> Result<Value, Error> {
        let class = classes(ruby)?.reclaimed;
        keep(ruby, this.as_value(), &this.core, |core| {
            let outcome = core.reclaim(handle.replacement);
            reply(ruby, outcome, |reclaimed| {
                answers::reclaimed(class, reclaimed)
            })
        })
    }

    fn abort(
        ruby: &Ruby,
        this: Obj<Self>,
        handle: Obj<ReplacementHandle>,
    ) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let outcome = this.core.borrow_mut().abort(handle.replacement);
        Ok(answer(ruby, outcome, |()| "aborted"))
    }
}
