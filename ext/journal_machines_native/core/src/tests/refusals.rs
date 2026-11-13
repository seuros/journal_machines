//! Every refusal and every idempotent repeat, once, each leaving the journal
//! bit-for-bit unchanged.
use std::string::ToString;

use super::{Small, at, journal_with, policy};
use crate::*;

/// Run `call`, expect `want`, and prove the journal did not change.
fn refused<T: core::fmt::Debug + PartialEq, const S: usize, const P: usize>(
    journal: &mut Journal<S, P>,
    want: Refusal,
    call: impl FnOnce(&mut Journal<S, P>) -> Result<T, Refusal>,
) {
    let before = journal.clone();
    assert_eq!(call(journal), Err(want));
    assert_eq!(*journal, before, "{want:?} must change nothing");
}

/// Run `call`, expect the honoured repeat `want`, and prove nothing changed.
fn repeated<T: core::fmt::Debug + PartialEq>(
    journal: &mut Small,
    want: T,
    call: impl FnOnce(&mut Small) -> Result<T, Refusal>,
) {
    let before = journal.clone();
    assert_eq!(call(journal), Ok(want));
    assert_eq!(*journal, before, "a repeat must change nothing");
}

#[test]
fn roll_and_observe_refusals_change_nothing() {
    let mut none = Journal::<0, 0>::new(policy(1, 1, 1, 0), 0, at(0));
    refused(&mut none, Refusal::NoActive, |j| j.roll(1, 1, at(1)));
    refused(&mut none, Refusal::NoActive, |j| {
        j.roll_decision(1, 1, at(1))
    });
    refused(&mut none, Refusal::PinsFull, |j| j.pin_reader(0));

    let mut journal = Small::new(policy(0, 0, 0, 500), u64::MAX - 5, at(7));
    refused(&mut journal, Refusal::Empty, |j| j.roll(1, 0, at(7)));
    refused(&mut journal, Refusal::TimeRegression, |j| {
        j.roll(1, 1, at(6))
    });
    refused(&mut journal, Refusal::TimeRegression, |j| {
        j.roll_decision(1, 1, at(6))
    });
    refused(&mut journal, Refusal::Overflow, |j| j.roll(1, 6, at(7)));
    refused(&mut journal, Refusal::Overflow, |j| {
        j.roll_decision(1, 6, at(7))
    });

    let (mut full, sealed) = journal_with(3);
    refused(&mut full, Refusal::SegmentsFull, |j| j.roll(1, 1, at(0)));
    let active = full.active().expect("active").id;
    refused(&mut full, Refusal::Unknown, |j| {
        j.observe_live(SegmentId::from_raw(9), 0)
    });
    refused(&mut full, Refusal::Active, |j| j.observe_live(active, 0));
    refused(&mut full, Refusal::Exceeds, |j| {
        j.observe_live(sealed[0], 11)
    });
    refused(&mut full, Refusal::Unknown, |j| {
        j.propose(SegmentId::from_raw(9))
    });
    refused(&mut full, Refusal::Active, |j| j.propose(active));
}

#[test]
fn pin_refusals_change_nothing() {
    let (mut journal, _) = journal_with(1);
    refused(&mut journal, Refusal::Unassigned, |j| {
        j.pin_checkpoint(0, 5)
    });
    let gone = journal.pin_reader(1).expect("pin");
    assert_eq!(journal.release(gone), Ok(()));
    let reader = journal.pin_reader(5).expect("reader");
    let checkpoint = journal.pin_checkpoint(3, 5).expect("checkpoint");
    refused(&mut journal, Refusal::PinsFull, |j| j.pin_reader(0));
    refused(&mut journal, Refusal::PinsFull, |j| j.pin_checkpoint(1, 0));
    refused(&mut journal, Refusal::Stale, |j| j.advance(gone, 9));
    refused(&mut journal, Refusal::Stale, |j| j.checkpoint(gone, 3, 9));
    refused(&mut journal, Refusal::Stale, |j| j.release(gone));
    let mut other = Small::new(policy(0, 0, 0, 500), 0, at(0));
    refused(&mut other, Refusal::Stale, |j| j.release(checkpoint));
    refused(&mut journal, Refusal::WrongKind, |j| {
        j.advance(checkpoint, 9)
    });
    refused(&mut journal, Refusal::WrongKind, |j| {
        j.checkpoint(reader, 3, 9)
    });
    refused(&mut journal, Refusal::Regressed, |j| j.advance(reader, 4));
    repeated(&mut journal, Advanced::Unchanged, |j| j.advance(reader, 5));
    refused(&mut journal, Refusal::Unassigned, |j| {
        j.checkpoint(checkpoint, 0, 9)
    });
    refused(&mut journal, Refusal::StaleAssignment, |j| {
        j.checkpoint(checkpoint, 2, 9)
    });
    refused(&mut journal, Refusal::Regressed, |j| {
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
    refused(&mut journal, Refusal::Busy, |j| j.propose(sealed[0]));
    refused(&mut journal, Refusal::Stale, |j| {
        j.confirm_durable(old, Durable::Superseded)
    });
    refused(&mut journal, Refusal::Stale, |j| j.reclaim(old));
    refused(&mut journal, Refusal::Stale, |j| j.abort(old));
    refused(&mut journal, Refusal::Exceeds, |j| {
        j.confirm_durable(current, rewritten(11))
    });
    refused(&mut journal, Refusal::NotDurable, |j| j.reclaim(current));
    assert_eq!(
        journal.confirm_durable(current, rewritten(4)),
        Ok(Confirmed::Recorded)
    );
    repeated(&mut journal, Confirmed::AlreadyDurable, |j| {
        j.confirm_durable(current, rewritten(4))
    });
    refused(&mut journal, Refusal::Mismatch, |j| {
        j.confirm_durable(current, Durable::Superseded)
    });
    refused(&mut journal, Refusal::Mismatch, |j| {
        j.confirm_durable(current, rewritten(11))
    });
    refused(&mut journal, Refusal::Busy, |j| j.propose(sealed[0]));
    let pin = journal.pin_reader(9).expect("pin");
    refused(&mut journal, Refusal::Pinned { offset: 9 }, |j| {
        j.reclaim(current)
    });
    assert_eq!(journal.release(pin), Ok(()));
    assert!(journal.reclaim(current).is_ok());
    refused(&mut journal, Refusal::Stale, |j| j.reclaim(current));
}

#[test]
fn spent_identity_spaces_are_exhausted_not_overflow() {
    let (mut worn, sealed) = journal_with(1);
    worn.issued = u64::MAX;
    worn.attempts = u64::MAX;
    worn.pin_generations = u64::MAX;
    refused(&mut worn, Refusal::Exhausted, |j| j.roll(1, 1, at(0)));
    refused(&mut worn, Refusal::Exhausted, |j| j.propose(sealed[0]));
    refused(&mut worn, Refusal::Exhausted, |j| j.pin_reader(0));
    refused(&mut worn, Refusal::Exhausted, |j| j.pin_checkpoint(1, 0));
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
