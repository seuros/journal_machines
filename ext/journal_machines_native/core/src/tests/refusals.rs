//! Every refusal and every idempotent repeat, once, each leaving the journal
//! bit-for-bit unchanged.
use std::string::ToString;

use matryona_machines::testing::{refuses, unchanged};

use super::{Small, at, fresh, journal_with, policy};
use crate::*;

/// Run `call`, expect the honoured repeat `want`, and prove nothing changed.
fn repeated<T: core::fmt::Debug + PartialEq>(
    journal: &mut Small,
    want: T,
    call: impl FnOnce(&mut Small) -> Result<T, Refusal>,
) {
    assert_eq!(unchanged(journal, call), Ok(want));
}

#[test]
fn roll_and_observe_refusals_change_nothing() {
    let mut none = Journal::<0, 0>::new(policy(1, 1, 1, 0), 0, at(0));
    refuses(&mut none, Refusal::NoActive, |j| j.roll(1, 1, at(1)));
    refuses(&mut none, Refusal::NoActive, |j| {
        j.roll_decision(1, 1, at(1))
    });
    refuses(&mut none, Refusal::PinsFull, |j| j.pin_reader(0));
    refuses(&mut none, Refusal::Unknown, |j| {
        j.propose(SegmentId::from_raw(1))
    });
    assert!(none.is_empty());
    assert_eq!((none.active(), none.segments(None).count()), (None, 0));

    let mut journal = Small::new(policy(0, 0, 0, 500), u64::MAX - 5, at(7));
    refuses(&mut journal, Refusal::Empty, |j| j.roll(1, 0, at(7)));
    refuses(&mut journal, Refusal::TimeRegression, |j| {
        j.roll(1, 1, at(6))
    });
    refuses(&mut journal, Refusal::TimeRegression, |j| {
        j.roll_decision(1, 1, at(6))
    });
    refuses(&mut journal, Refusal::Overflow, |j| j.roll(1, 6, at(7)));
    refuses(&mut journal, Refusal::Overflow, |j| {
        j.roll_decision(1, 6, at(7))
    });

    let (mut full, sealed) = journal_with(3);
    refuses(&mut full, Refusal::SegmentsFull, |j| j.roll(1, 1, at(0)));
    let active = full.active().expect("active").id;
    refuses(&mut full, Refusal::Unknown, |j| {
        j.observe_live(SegmentId::from_raw(9), 0)
    });
    refuses(&mut full, Refusal::Active, |j| j.observe_live(active, 0));
    refuses(&mut full, Refusal::Exceeds, |j| {
        j.observe_live(sealed[0], 11)
    });
    refuses(&mut full, Refusal::Unknown, |j| {
        j.propose(SegmentId::from_raw(9))
    });
    refuses(&mut full, Refusal::Active, |j| j.propose(active));
}

#[test]
fn pin_refusals_change_nothing() {
    let (mut journal, _) = journal_with(1);
    refuses(&mut journal, Refusal::Unassigned, |j| {
        j.pin_checkpoint(0, 5)
    });
    let gone = journal.pin_reader(1).expect("pin");
    assert_eq!(journal.release(gone), Ok(()));
    let reader = journal.pin_reader(5).expect("reader");
    let checkpoint = journal.pin_checkpoint(3, 5).expect("checkpoint");
    refuses(&mut journal, Refusal::PinsFull, |j| j.pin_reader(0));
    refuses(&mut journal, Refusal::PinsFull, |j| j.pin_checkpoint(1, 0));
    refuses(&mut journal, Refusal::Stale, |j| j.advance(gone, 9));
    refuses(&mut journal, Refusal::Stale, |j| j.checkpoint(gone, 3, 9));
    // Nothing remembers a released pin: releasing it again is not provable.
    refuses(&mut journal, Refusal::Stale, |j| j.release(gone));
    // A journal that has issued no pin never issued this one.
    let mut other = fresh();
    refuses(&mut other, Refusal::Stale, |j| j.release(checkpoint));
    refuses(&mut journal, Refusal::WrongKind, |j| {
        j.advance(checkpoint, 9)
    });
    refuses(&mut journal, Refusal::WrongKind, |j| {
        j.checkpoint(reader, 3, 9)
    });
    refuses(&mut journal, Refusal::Regressed, |j| j.advance(reader, 4));
    repeated(&mut journal, Advanced::Unchanged, |j| j.advance(reader, 5));
    refuses(&mut journal, Refusal::Unassigned, |j| {
        j.checkpoint(checkpoint, 0, 9)
    });
    refuses(&mut journal, Refusal::StaleAssignment, |j| {
        j.checkpoint(checkpoint, 2, 9)
    });
    refuses(&mut journal, Refusal::Regressed, |j| {
        j.checkpoint(checkpoint, 3, 4)
    });
    repeated(&mut journal, Checkpointed::Unchanged, |j| {
        j.checkpoint(checkpoint, 3, 5)
    });
    assert_eq!(journal.advance(reader, 6), Ok(Advanced::Moved));
    assert_eq!(
        journal.checkpoint(checkpoint, 3, 6),
        Ok(Checkpointed::Advanced)
    );
    assert_eq!(
        journal.checkpoint(checkpoint, 4, 0),
        Ok(Checkpointed::Reassigned)
    );
    assert_eq!(
        journal.pin_offset(checkpoint),
        Some(0),
        "the new owner reports its own base"
    );
    let top = u64::MAX;
    assert_eq!(
        journal.checkpoint(checkpoint, top, top),
        Ok(Checkpointed::Reassigned)
    );
    repeated(&mut journal, Checkpointed::Unchanged, |j| {
        j.checkpoint(checkpoint, top, top)
    });
}

#[test]
fn replacement_refusals_change_nothing() {
    let (mut journal, sealed) = journal_with(2);
    let old = journal.propose(sealed[0]).expect("propose");
    assert_eq!(journal.abort(old), Ok(()));
    let current = journal.propose(sealed[0]).expect("propose");
    let rewritten = |live_records| Durable::Rewritten {
        bytes: 1,
        live_records,
    };
    refuses(&mut journal, Refusal::Busy, |j| j.propose(sealed[0]));
    refuses(&mut journal, Refusal::Stale, |j| {
        j.confirm_durable(old, Durable::Superseded)
    });
    refuses(&mut journal, Refusal::Stale, |j| j.reclaim(old));
    refuses(&mut journal, Refusal::Stale, |j| j.abort(old));
    refuses(&mut journal, Refusal::Exceeds, |j| {
        j.confirm_durable(current, rewritten(11))
    });
    refuses(&mut journal, Refusal::NotDurable, |j| j.reclaim(current));
    assert_eq!(
        journal.confirm_durable(current, rewritten(4)),
        Ok(Confirmed::Recorded)
    );
    repeated(&mut journal, Confirmed::AlreadyDurable, |j| {
        j.confirm_durable(current, rewritten(4))
    });
    refuses(&mut journal, Refusal::Mismatch, |j| {
        j.confirm_durable(current, Durable::Superseded)
    });
    refuses(&mut journal, Refusal::Mismatch, |j| {
        j.confirm_durable(current, rewritten(11))
    });
    refuses(&mut journal, Refusal::Busy, |j| j.propose(sealed[0]));
    let pin = journal.pin_reader(9).expect("pin");
    refuses(&mut journal, Refusal::Pinned { offset: 9 }, |j| {
        j.reclaim(current)
    });
    assert_eq!(journal.release(pin), Ok(()));
    assert!(journal.reclaim(current).is_ok());
    refuses(&mut journal, Refusal::Stale, |j| j.reclaim(current));
}

#[test]
fn refusal_is_an_error_with_a_message_per_variant() {
    let all = [
        Refusal::NoActive,
        Refusal::Empty,
        Refusal::TimeRegression,
        Refusal::Overflow,
        Refusal::SegmentsFull,
        Refusal::PinsFull,
        Refusal::Exhausted,
        Refusal::Unknown,
        Refusal::Active,
        Refusal::Busy,
        Refusal::Exceeds,
        Refusal::Mismatch,
        Refusal::Stale,
        Refusal::WrongKind,
        Refusal::Unassigned,
        Refusal::StaleAssignment,
        Refusal::Regressed,
        Refusal::NotDurable,
        Refusal::Pinned { offset: 3 },
    ];
    let source: &dyn core::error::Error = &all[0];
    assert!(source.source().is_none());
    for (index, refusal) in all.iter().enumerate() {
        let message = refusal.to_string();
        assert_ne!(message, "");
        for other in &all[index + 1..] {
            assert_ne!(*refusal, *other);
            assert_ne!(message, other.to_string());
        }
    }
}
