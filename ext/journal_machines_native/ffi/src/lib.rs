//! Thin binding to the bounded segment metadata journal.
mod convert;
mod init;

use convert::{Number, roll_error, threshold, update};

use std::{cell::RefCell, sync::Arc};

use journal_machines::{
    Abort, Confirm, Durable, Observe, Pin, PinError, PinKind, PinUpdate, Policy, ProposeError,
    ReclaimError, Release, Replacement, Roll, RollReason, Segment, SegmentId, SegmentState,
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
            _ => "checkpoint",
        })
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
        let (max_bytes, max_records, max_age) = (
            threshold(ruby, max_bytes)?,
            threshold(ruby, max_records)?,
            threshold(ruby, max_age)?,
        );
        let policy = u16::try_from(min_dead_per_mille.0)
            .ok()
            .and_then(|per_mille| Policy::new(max_bytes, max_records, max_age, per_mille))
            .ok_or_else(|| {
                Error::new(
                    ruby.exception_range_error(),
                    "min_dead_per_mille must be within 0..=1000",
                )
            })?;
        Ok(Self {
            owner: Arc::new(()),
            core: RefCell::new(journal_machines::Journal::new(policy, base_offset.0, now.0)),
        })
    }

    fn size(&self) -> usize {
        self.core.borrow().len()
    }

    fn roll_decision(
        ruby: &Ruby,
        this: &Self,
        bytes: Number,
        records: Number,
        now: Number,
    ) -> Symbol {
        let decision = this.core.borrow().roll_decision(bytes.0, records.0, now.0);
        ruby.to_symbol(match decision {
            Ok(Roll::Due(RollReason::Size)) => "size",
            Ok(Roll::Due(RollReason::Records)) => "records",
            Ok(Roll::Due(_)) => "age",
            Ok(_) => "keep",
            Err(error) => roll_error(error),
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
        let rolled = this.core.borrow_mut().roll(bytes.0, records.0, now.0);
        Ok(match rolled {
            Ok(rolled) => {
                (rolled.sealed.raw(), rolled.opened.raw(), rolled.next_offset).into_value_with(ruby)
            }
            Err(error) => ruby.to_symbol(roll_error(error)).as_value(),
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
        Ok(ruby.to_symbol(match outcome {
            Observe::Recorded => "recorded",
            Observe::Active => "active",
            Observe::Exceeds => "exceeds",
            _ => "unknown",
        }))
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
        row.push(segment.opened_at)?;
        row.push(ruby.to_symbol(match segment.state {
            SegmentState::Active => "active",
            SegmentState::Sealed => "sealed",
            SegmentState::Proposed => "proposed",
            SegmentState::Durable => "durable",
            _ => "reclaimable",
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
            _ => core.reclaimable(after),
        };
        let rows = ruby.ary_new_capa(take);
        for segment in segments.take(take) {
            rows.push(this.row(ruby, segment)?)?;
        }
        Ok(rows)
    }

    fn pinned(ruby: &Ruby, this: Obj<Self>, pin: Result<Pin, PinError>) -> Value {
        match pin {
            Ok(pin) => PinHandle {
                owner: Arc::clone(&this.owner),
                pin,
            }
            .into_value_with(ruby),
            Err(PinError::Full) => ruby.to_symbol("full").as_value(),
            Err(PinError::Unassigned) => ruby.to_symbol("unassigned").as_value(),
            Err(_) => ruby.to_symbol("exhausted").as_value(),
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

    fn own_pin(&self, handle: &PinHandle) -> Option<Pin> {
        Arc::ptr_eq(&self.owner, &handle.owner).then_some(handle.pin)
    }

    fn advance(
        ruby: &Ruby,
        this: Obj<Self>,
        handle: Obj<PinHandle>,
        offset: Number,
    ) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let outcome = this.own_pin(&handle).map_or(PinUpdate::Stale, |pin| {
            this.core.borrow_mut().advance(pin, offset.0)
        });
        Ok(ruby.to_symbol(update(outcome)))
    }

    fn checkpoint(
        ruby: &Ruby,
        this: Obj<Self>,
        handle: Obj<PinHandle>,
        assignment: Number,
        offset: Number,
    ) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let outcome = this.own_pin(&handle).map_or(PinUpdate::Stale, |pin| {
            this.core
                .borrow_mut()
                .checkpoint(pin, assignment.0, offset.0)
        });
        Ok(ruby.to_symbol(update(outcome)))
    }

    fn release(ruby: &Ruby, this: Obj<Self>, handle: Obj<PinHandle>) -> Result<Symbol, Error> {
        this.check_frozen()?;
        let outcome = this
            .own_pin(&handle)
            .map_or(Release::Stale, |pin| this.core.borrow_mut().release(pin));
        Ok(ruby.to_symbol(match outcome {
            Release::Released => "released",
            _ => "stale",
        }))
    }

    fn pin_offset(&self, handle: Obj<PinHandle>) -> Option<u64> {
        self.own_pin(&handle)
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
            Err(ProposeError::Unknown) => ruby.to_symbol("unknown").as_value(),
            Err(ProposeError::Active) => ruby.to_symbol("active").as_value(),
            Err(ProposeError::Busy) => ruby.to_symbol("busy").as_value(),
            Err(_) => ruby.to_symbol("exhausted").as_value(),
        })
    }

    fn own_replacement(&self, handle: &ReplacementHandle) -> Option<Replacement> {
        Arc::ptr_eq(&self.owner, &handle.owner).then_some(handle.replacement)
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
            .map_or(Confirm::Stale, |replacement| {
                this.core.borrow_mut().confirm_durable(replacement, durable)
            });
        Ok(ruby.to_symbol(match outcome {
            Confirm::Confirmed => "confirmed",
            Confirm::AlreadyDurable => "already_durable",
            Confirm::Invalid => "invalid",
            _ => "stale",
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
            .map_or(Err(ReclaimError::Stale), |replacement| {
                this.core.borrow_mut().reclaim(replacement)
            });
        Ok(match outcome {
            Ok(reclaimed) => (
                reclaimed.segment.raw(),
                reclaimed.revision,
                reclaimed.bytes,
                reclaimed.replaced_by,
            )
                .into_value_with(ruby),
            Err(ReclaimError::NotDurable) => ruby.to_symbol("not_durable").as_value(),
            Err(ReclaimError::Pinned { .. }) => ruby.to_symbol("pinned").as_value(),
            Err(_) => ruby.to_symbol("stale").as_value(),
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
            .map_or(Abort::Stale, |replacement| {
                this.core.borrow_mut().abort(replacement)
            });
        Ok(ruby.to_symbol(match outcome {
            Abort::Aborted => "aborted",
            _ => "stale",
        }))
    }
}
