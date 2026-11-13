//! Thin binding to the bounded segment metadata journal.
mod convert;
mod init;

use convert::{Number, Scope, answer, nanos, policy, refusal};

use std::{cell::RefCell, num::NonZeroU64};

use journal_machines::{
    Advanced, Checkpointed, Confirmed, Durable, Pin, PinKind, Refusal, Replacement, Roll,
    RollReason, Segment, SegmentId, SegmentState,
};
use magnus::{Error, IntoValue, RArray, Ruby, Symbol, Value, prelude::*, typed_data::Obj};

/// The four `Policy` fields as Ruby passes them, in order.
type Thresholds = (Option<Number>, Option<Number>, Option<Number>, Number);

const SEGMENTS: usize = 64;
const PINS: usize = 32;

#[magnus::wrap(class = "JournalMachines::Pin", free_immediately, size)]
struct PinHandle {
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

    const fn scope(&self) -> u16 {
        self.pin.scope()
    }
}

#[magnus::wrap(class = "JournalMachines::Replacement", free_immediately, size)]
struct ReplacementHandle {
    replacement: Replacement,
}

impl ReplacementHandle {
    const fn segment(&self) -> u64 {
        self.replacement.segment().raw()
    }

    const fn attempt(&self) -> u64 {
        self.replacement.attempt()
    }

    const fn scope(&self) -> u16 {
        self.replacement.scope()
    }
}

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
        (max_bytes, max_records, max_age, min_dead_per_mille): Thresholds,
    ) -> Result<Self, Error> {
        let policy = policy(ruby, max_bytes, max_records, max_age, min_dead_per_mille)?;
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

    fn row(ruby: &Ruby, segment: Segment) -> Result<RArray, Error> {
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
        row.push(
            segment
                .replacement
                .map(|replacement| ReplacementHandle { replacement }),
        )?;
        Ok(row)
    }

    fn optional(ruby: &Ruby, segment: Option<Segment>) -> Result<Option<RArray>, Error> {
        segment.map(|segment| Self::row(ruby, segment)).transpose()
    }

    fn active(ruby: &Ruby, this: &Self) -> Result<Option<RArray>, Error> {
        let segment = this.core.borrow().active();
        Self::optional(ruby, segment)
    }

    fn segment(ruby: &Ruby, this: &Self, id: Number) -> Result<Option<RArray>, Error> {
        let segment = this.core.borrow().segment(SegmentId::from_raw(id.0));
        Self::optional(ruby, segment)
    }

    fn covering(ruby: &Ruby, this: &Self, offset: Number) -> Result<Option<RArray>, Error> {
        let segment = this.core.borrow().covering(offset.0);
        Self::optional(ruby, segment)
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
            rows.push(Self::row(ruby, segment)?)?;
        }
        Ok(rows)
    }

    fn pinned(ruby: &Ruby, pin: Result<Pin, Refusal>) -> Value {
        match pin {
            Ok(pin) => PinHandle { pin }.into_value_with(ruby),
            Err(error) => ruby.to_symbol(refusal(error)).as_value(),
        }
    }

    fn pin_reader(ruby: &Ruby, this: Obj<Self>, offset: Number) -> Result<Value, Error> {
        this.check_frozen()?;
        let pin = this.core.borrow_mut().pin_reader(offset.0);
        Ok(Self::pinned(ruby, pin))
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
        Ok(Self::pinned(ruby, pin))
    }

    fn advance(
        ruby: &Ruby,
        this: Obj<Self>,
        handle: Obj<PinHandle>,
        offset: Number,
    ) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let outcome = this.core.borrow_mut().advance(handle.pin, offset.0);
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
        let outcome = this
            .core
            .borrow_mut()
            .checkpoint(handle.pin, assignment.0, offset.0);
        Ok(ruby.to_symbol(match outcome {
            Ok(Checkpointed::Reassigned) => "reassigned",
            Ok(Checkpointed::Unchanged) => "unchanged",
            Ok(Checkpointed::Advanced) => "advanced",
            Err(error) => refusal(error),
        }))
    }

    fn release(ruby: &Ruby, this: Obj<Self>, handle: Obj<PinHandle>) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let outcome = this.core.borrow_mut().release(handle.pin);
        Ok(answer(ruby, outcome, "released"))
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
        Ok(match proposed {
            Ok(replacement) => ReplacementHandle { replacement }.into_value_with(ruby),
            Err(error) => ruby.to_symbol(refusal(error)).as_value(),
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
        let outcome = this.core.borrow_mut().reclaim(handle.replacement);
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
        let outcome = this.core.borrow_mut().abort(handle.replacement);
        Ok(answer(ruby, outcome, "aborted"))
    }
}
