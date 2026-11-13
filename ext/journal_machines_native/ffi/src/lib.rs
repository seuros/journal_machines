//! Thin binding to the bounded segment metadata journal.
mod convert;
mod init;

use convert::{Number, answer, nanos, policy, refusal};

use std::{cell::RefCell, num::NonZeroU64, sync::Arc};

use journal_machines::{
    Advanced, Checkpointed, Confirmed, Durable, Pin, PinKind, Refusal, Replacement, Roll,
    RollReason, Segment, SegmentId, SegmentState,
};
use magnus::{Error, IntoValue, RArray, Ruby, Symbol, Value, prelude::*, typed_data::Obj};

const SEGMENTS: usize = 64;
const PINS: usize = 32;

#[magnus::wrap(class = "JournalMachines::Pin", free_immediately, size)]
struct PinHandle {
    owner: Arc<()>,
    pin: Pin,
}

impl PinHandle {
    fn kind(ruby: &Ruby, this: &Self) -> Symbol {
        ruby.to_symbol(match this.pin.kind() {
            PinKind::Reader => "reader",
            PinKind::Checkpoint => "checkpoint",
        })
    }

    const fn generation(&self) -> u64 {
        self.pin.generation()
    }
}

#[magnus::wrap(class = "JournalMachines::Replacement", free_immediately, size)]
struct ReplacementHandle {
    owner: Arc<()>,
    replacement: Replacement,
}

impl ReplacementHandle {
    const fn segment(&self) -> u64 {
        self.replacement.segment().raw()
    }

    const fn attempt(&self) -> u64 {
        self.replacement.attempt()
    }
}

#[magnus::wrap(class = "JournalMachines::Journal", free_immediately, size)]
struct Journal {
    owner: Arc<()>,
    core: RefCell<journal_machines::Journal<SEGMENTS, PINS>>,
}

impl Journal {
    fn open(
        ruby: &Ruby,
        base_offset: Number,
        now: Number,
        max_bytes: Option<Number>,
        max_records: Option<Number>,
        max_age: Option<Number>,
        min_dead_per_mille: Number,
    ) -> Result<Self, Error> {
        let policy = policy(ruby, max_bytes, max_records, max_age, min_dead_per_mille)?;
        Ok(Self {
            owner: Arc::new(()),
            core: RefCell::new(journal_machines::Journal::new(
                policy,
                base_offset.0,
                now.nanos(),
            )),
        })
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

    fn roll_decision(
        ruby: &Ruby,
        this: &Self,
        bytes: Number,
        records: Number,
        now: Number,
    ) -> Symbol {
        let decision = this
            .core
            .borrow()
            .roll_decision(bytes.0, records.0, now.nanos());
        ruby.to_symbol(match decision {
            Ok(Roll::Due(RollReason::Size)) => "size",
            Ok(Roll::Due(RollReason::Records)) => "records",
            Ok(Roll::Due(RollReason::Age)) => "age",
            Ok(Roll::Keep) => "keep",
            Err(error) => refusal(error),
        })
    }

    fn roll(
        ruby: &Ruby,
        this: Obj<Self>,
        bytes: Number,
        records: Number,
        now: Number,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let rolled = this.core.borrow_mut().roll(bytes.0, records.0, now.nanos());
        Ok(match rolled {
            Ok(rolled) => {
                (rolled.sealed.raw(), rolled.opened.raw(), rolled.next_offset).into_value_with(ruby)
            }
            Err(error) => ruby.to_symbol(refusal(error)).as_value(),
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
        Ok(answer(ruby, outcome, "recorded"))
    }

    fn row(&self, ruby: &Ruby, segment: Segment) -> Result<RArray, Error> {
        let row = ruby.ary_new_capa(11);
        row.push(segment.id.raw())?;
        row.push(segment.revision)?;
        row.push(segment.base_offset)?;
        row.push(segment.records)?;
        row.push(segment.stored_records)?;
        row.push(segment.live_records)?;
        row.push(segment.bytes)?;
        row.push(nanos(segment.opened_at))?;
        row.push(ruby.to_symbol(match segment.state {
            SegmentState::Active => "active",
            SegmentState::Sealed => "sealed",
            SegmentState::Proposed => "proposed",
            SegmentState::Durable => "durable",
            SegmentState::Reclaimable => "reclaimable",
        }))?;
        row.push(segment.pinned)?;
        row.push(segment.replacement.map(|replacement| ReplacementHandle {
            owner: Arc::clone(&self.owner),
            replacement,
        }))?;
        Ok(row)
    }

    fn optional(&self, ruby: &Ruby, segment: Option<Segment>) -> Result<Option<RArray>, Error> {
        segment.map(|segment| self.row(ruby, segment)).transpose()
    }

    fn active(ruby: &Ruby, this: &Self) -> Result<Option<RArray>, Error> {
        let segment = this.core.borrow().active();
        this.optional(ruby, segment)
    }

    fn segment(ruby: &Ruby, this: &Self, id: Number) -> Result<Option<RArray>, Error> {
        let segment = this.core.borrow().segment(SegmentId::from_raw(id.0));
        this.optional(ruby, segment)
    }

    fn covering(ruby: &Ruby, this: &Self, offset: Number) -> Result<Option<RArray>, Error> {
        let segment = this.core.borrow().covering(offset.0);
        this.optional(ruby, segment)
    }

    /// At most `limit` rows (clamped to the capacity) after segment `after`.
    /// `which` is one of the three private selectors the Ruby side passes.
    fn scan(
        ruby: &Ruby,
        this: &Self,
        which: Symbol,
        after: Option<Number>,
        limit: Number,
    ) -> Result<RArray, Error> {
        let take = usize::try_from(limit.0).map_or(SEGMENTS, |limit| limit.min(SEGMENTS));
        let after = after.map(|after| SegmentId::from_raw(after.0));
        let core = this.core.borrow();
        let segments = match which.name()?.as_ref() {
            "all" => core.segments(after),
            "candidates" => core.compaction_candidates(after),
            "reclaimable" => core.reclaimable(after),
            _ => return Err(Error::new(ruby.exception_arg_error(), "unknown scan")),
        };
        let rows = ruby.ary_new_capa(take);
        for segment in segments.take(take) {
            rows.push(this.row(ruby, segment)?)?;
        }
        Ok(rows)
    }

    fn pinned(ruby: &Ruby, this: Obj<Self>, pin: Result<Pin, Refusal>) -> Value {
        match pin {
            Ok(pin) => PinHandle {
                owner: Arc::clone(&this.owner),
                pin,
            }
            .into_value_with(ruby),
            Err(error) => ruby.to_symbol(refusal(error)).as_value(),
        }
    }

    fn pin_reader(ruby: &Ruby, this: Obj<Self>, offset: Number) -> Result<Value, Error> {
        this.check_frozen()?;
        let pin = this.core.borrow_mut().pin_reader(offset.0);
        Ok(Self::pinned(ruby, this, pin))
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
        Ok(Self::pinned(ruby, this, pin))
    }

    /// A handle issued by another journal is stale here.
    fn own_pin(&self, handle: &PinHandle) -> Result<Pin, Refusal> {
        Arc::ptr_eq(&self.owner, &handle.owner)
            .then_some(handle.pin)
            .ok_or(Refusal::Stale)
    }

    fn advance(
        ruby: &Ruby,
        this: Obj<Self>,
        handle: Obj<PinHandle>,
        offset: Number,
    ) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let outcome = this
            .own_pin(&handle)
            .and_then(|pin| this.core.borrow_mut().advance(pin, offset.0));
        Ok(ruby.to_symbol(match outcome {
            Ok(Advanced::Unchanged) => "unchanged",
            Ok(Advanced::Moved) => "moved",
            Err(error) => refusal(error),
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
        let outcome = this.own_pin(&handle).and_then(|pin| {
            this.core
                .borrow_mut()
                .checkpoint(pin, assignment.0, offset.0)
        });
        Ok(ruby.to_symbol(match outcome {
            Ok(Checkpointed::Reassigned) => "reassigned",
            Ok(Checkpointed::Unchanged) => "unchanged",
            Ok(Checkpointed::Advanced) => "advanced",
            Err(error) => refusal(error),
        }))
    }

    fn release(ruby: &Ruby, this: Obj<Self>, handle: Obj<PinHandle>) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let outcome = this
            .own_pin(&handle)
            .and_then(|pin| this.core.borrow_mut().release(pin));
        Ok(answer(ruby, outcome, "released"))
    }

    fn pin_offset(&self, handle: Obj<PinHandle>) -> Option<u64> {
        self.own_pin(&handle)
            .ok()
            .and_then(|pin| self.core.borrow().pin_offset(pin))
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
        Ok(match proposed {
            Ok(replacement) => ReplacementHandle {
                owner: Arc::clone(&this.owner),
                replacement,
            }
            .into_value_with(ruby),
            Err(error) => ruby.to_symbol(refusal(error)).as_value(),
        })
    }

    /// A handle issued by another journal is stale here.
    fn own_replacement(&self, handle: &ReplacementHandle) -> Result<Replacement, Refusal> {
        Arc::ptr_eq(&self.owner, &handle.owner)
            .then_some(handle.replacement)
            .ok_or(Refusal::Stale)
    }

    fn confirm(
        ruby: &Ruby,
        this: Obj<Self>,
        handle: Obj<ReplacementHandle>,
        durable: Durable,
    ) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let outcome = this
            .own_replacement(&handle)
            .and_then(|replacement| this.core.borrow_mut().confirm_durable(replacement, durable));
        Ok(ruby.to_symbol(match outcome {
            Ok(Confirmed::AlreadyDurable) => "already_durable",
            Ok(Confirmed::Recorded) => "recorded",
            Err(error) => refusal(error),
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
        this.check_frozen()?;
        let outcome = this
            .own_replacement(&handle)
            .and_then(|replacement| this.core.borrow_mut().reclaim(replacement));
        Ok(match outcome {
            Ok(reclaimed) => (
                reclaimed.segment.raw(),
                reclaimed.revision,
                reclaimed.bytes,
                reclaimed.replaced_by,
            )
                .into_value_with(ruby),
            Err(error) => ruby.to_symbol(refusal(error)).as_value(),
        })
    }

    fn abort(
        ruby: &Ruby,
        this: Obj<Self>,
        handle: Obj<ReplacementHandle>,
    ) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let outcome = this
            .own_replacement(&handle)
            .and_then(|replacement| this.core.borrow_mut().abort(replacement));
        Ok(answer(ruby, outcome, "aborted"))
    }
}
