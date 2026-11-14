#![allow(clippy::expect_used, clippy::unwrap_used)]
use core::{num::NonZeroU64, time::Duration};
use std::vec::Vec;

use super::*;

mod checked;
mod model;
mod pins;
mod random;
mod refusals;
mod repeats;
mod resume;
mod scope;

type Small = Journal<4, 2>;

/// A journal of scope zero with no thresholds, at offset and instant zero.
fn fresh() -> Small {
    open(0)
}

/// As [`fresh`], issuing in `scope`.
fn open(scope: u16) -> Small {
    Small::with_scope(policy(0, 0, 0, 500), 0, at(0), scope)
}

/// As [`open`], resumed after `issued` identities.
fn resumed(scope: u16, issued: u64) -> Option<Small> {
    Small::resumed(policy(0, 0, 0, 500), 0, at(0), scope, issued)
}

/// A journal of `scope` whose sequence has `left` identities to issue after
/// its active segment took one, seeded through the public constructor.
fn worn(scope: u16, left: u64) -> Small {
    resumed(scope, periscope_machines::SEQUENCE_MAX - left - 1).expect("resumed")
}

/// A monotonic instant or span of `seconds`.
const fn at(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

/// Zero disables a threshold; `age` is in seconds.
fn policy(bytes: u64, records: u64, age: u64, per_mille: u16) -> Policy {
    Policy::new(
        NonZeroU64::new(bytes),
        NonZeroU64::new(records),
        Some(at(age)).filter(|age| !age.is_zero()),
        per_mille,
    )
    .expect("policy")
}

/// A journal with `sealed` ten-record, hundred-byte segments behind the active one.
fn journal_with(sealed: u64) -> (Small, Vec<SegmentId>) {
    let mut journal = fresh();
    let ids = (0..sealed)
        .map(|_| journal.roll(100, 10, at(0)).expect("roll").sealed)
        .collect();
    (journal, ids)
}

fn ids(segments: Segments<'_, 4, 2>) -> Vec<u64> {
    segments.map(|segment| segment.id.raw()).collect()
}

#[test]
fn roll_decision_follows_size_then_records_then_age() {
    let journal = Small::new(policy(100, 10, 50, 0), 0, at(1000));
    assert_eq!(journal.roll_decision(99, 9, at(1049)), Ok(Roll::Keep));
    assert_eq!(
        journal.roll_decision(100, 9, at(1000)),
        Ok(Roll::Due(RollReason::Size))
    );
    assert_eq!(
        journal.roll_decision(1, 10, at(1000)),
        Ok(Roll::Due(RollReason::Records))
    );
    assert_eq!(
        journal.roll_decision(1, 1, at(1050)),
        Ok(Roll::Due(RollReason::Age))
    );
    assert_eq!(
        journal.roll_decision(100, 10, at(1050)),
        Ok(Roll::Due(RollReason::Size))
    );
    assert_eq!(
        journal.roll_decision(1, 10, at(1050)),
        Ok(Roll::Due(RollReason::Records))
    );
    assert_eq!(journal.roll_decision(0, 0, at(u64::MAX)), Ok(Roll::Keep));
    assert_eq!(
        journal.roll_decision(1, 1, at(999)),
        Err(Refusal::TimeRegression)
    );
}

#[test]
fn disabled_thresholds_never_trigger_and_policy_is_validated() {
    let never = policy(0, 0, 0, 1000);
    assert_eq!(
        (never.max_bytes(), never.max_records(), never.max_age()),
        (None, None, None)
    );
    assert_eq!(never.min_dead_per_mille(), 1000);
    let journal = Small::new(never, 0, at(0));
    assert_eq!(
        journal.roll_decision(u64::MAX, u64::MAX, at(u64::MAX)),
        Ok(Roll::Keep)
    );
    assert_eq!(journal.policy(), never);
    assert!(Policy::new(None, None, None, 1001).is_none());
    assert!(
        Policy::new(None, None, Some(Duration::ZERO), 0).is_none(),
        "a zero age is not a threshold"
    );
    let aged = Policy::new(None, None, Some(Duration::from_nanos(1)), 0).expect("policy");
    assert_eq!(aged.max_age(), Some(Duration::from_nanos(1)));
    let journal = Small::new(aged, 0, Duration::from_nanos(5));
    assert_eq!(
        journal.roll_decision(1, 1, Duration::from_nanos(5)),
        Ok(Roll::Keep)
    );
    assert_eq!(
        journal.roll_decision(1, 1, Duration::from_nanos(6)),
        Ok(Roll::Due(RollReason::Age)),
        "ages are compared at full Duration resolution"
    );
    assert_eq!(
        journal.roll_decision(1, 1, Duration::MAX).map(|_| ()),
        Ok(())
    );
}

#[test]
fn roll_seals_the_active_segment_and_opens_the_next() {
    let mut journal = Small::new(policy(0, 0, 0, 500), 100, at(7));
    assert_eq!(journal.roll(64, 0, at(7)), Err(Refusal::Empty));
    assert_eq!(journal.roll(64, 5, at(6)), Err(Refusal::TimeRegression));
    let rolled = journal.roll(64, 5, at(9)).expect("roll");
    assert_eq!(
        (rolled.sealed.raw(), rolled.opened.raw(), rolled.next_offset),
        (1, 2, 105)
    );
    let sealed = journal.segment(rolled.sealed).expect("sealed");
    assert_eq!(sealed.state, SegmentState::Sealed);
    assert_eq!(
        (sealed.base_offset, sealed.end_offset(), sealed.bytes),
        (100, 105, 64)
    );
    assert_eq!((sealed.stored_records, sealed.live_records), (5, 5));
    assert_eq!((sealed.opened_at, sealed.revision), (at(7), 0));
    let active = journal.active().expect("active");
    assert_eq!(
        (active.id, active.base_offset, active.opened_at),
        (rolled.opened, 105, at(9))
    );
    assert_eq!(active.state, SegmentState::Active);
    assert_eq!(journal.covering(99), None);
    assert_eq!(journal.covering(104).map(|s| s.id), Some(rolled.sealed));
    assert_eq!(journal.covering(105).map(|s| s.id), Some(rolled.opened));
    assert_eq!(
        journal.covering(u64::MAX).map(|s| s.id),
        Some(rolled.opened)
    );
    assert_eq!(journal.len(), 2);
    assert!(!journal.is_empty());
}

#[test]
fn a_full_journal_rolls_again_once_a_segment_leaves() {
    let (mut journal, sealed) = journal_with(3);
    assert_eq!((journal.len(), Small::SEGMENT_CAPACITY), (4, 4));
    assert_eq!(journal.roll(1, 1, at(0)), Err(Refusal::SegmentsFull));
    assert_eq!(
        journal.roll_decision(1, 1, at(0)),
        Ok(Roll::Keep),
        "deciding reads only"
    );
    let replacement = journal.propose(sealed[0]).expect("propose");
    assert_eq!(
        journal.confirm_durable(replacement, Durable::Superseded),
        Ok(Confirmed::Recorded)
    );
    assert!(journal.reclaim(replacement).is_ok());
    assert_eq!(
        journal.roll(1, 1, at(0)).map(|rolled| rolled.opened.raw()),
        Ok(6),
        "ids ascend but are not consecutive: the attempt took 5"
    );
}

#[test]
fn candidates_need_enough_dead_records_and_resume_by_identity() {
    let (mut journal, sealed) = journal_with(3);
    assert_eq!(ids(journal.compaction_candidates(None)), []);
    assert_eq!(journal.observe_live(sealed[0], 5), Ok(()));
    assert_eq!(journal.observe_live(sealed[1], 6), Ok(()));
    assert_eq!(journal.observe_live(sealed[2], 0), Ok(()));
    assert_eq!(journal.observe_live(sealed[2], 11), Err(Refusal::Exceeds));
    assert_eq!(
        journal.observe_live(SegmentId::from_raw(4), 0),
        Err(Refusal::Active)
    );
    assert_eq!(
        journal.observe_live(SegmentId::from_raw(9), 0),
        Err(Refusal::Unknown)
    );
    assert_eq!(ids(journal.compaction_candidates(None)), [1, 3]);
    assert_eq!(ids(journal.compaction_candidates(Some(sealed[0]))), [3]);
    assert_eq!(ids(journal.compaction_candidates(Some(sealed[2]))), []);
    assert_eq!(ids(journal.segments(Some(sealed[1]))), [3, 4]);
    assert_eq!(
        ids(journal.segments(Some(SegmentId::from_raw(u64::MAX)))),
        []
    );
    let replacement = journal.propose(sealed[0]).expect("propose");
    assert_eq!(
        ids(journal.compaction_candidates(None)),
        [3],
        "in flight is not a candidate"
    );
    assert_eq!(journal.abort(replacement), Ok(()));
    assert_eq!(ids(journal.compaction_candidates(None)), [1, 3]);
    let mut strict = Small::new(policy(0, 0, 0, 0), 0, at(0));
    let all_live = strict.roll(1, 3, at(0)).expect("roll").sealed;
    assert_eq!(
        ids(strict.compaction_candidates(None)),
        [],
        "nothing dead, nothing to do"
    );
    assert_eq!(strict.observe_live(all_live, 2), Ok(()));
    assert_eq!(ids(strict.compaction_candidates(None)), [1]);
}

#[test]
fn replacement_moves_from_proposed_to_durable_to_reclaimed() {
    let (mut journal, sealed) = journal_with(2);
    let state = |journal: &Small, id| journal.segment(id).map(|segment| segment.state);
    assert_eq!(
        journal.propose(SegmentId::from_raw(3)),
        Err(Refusal::Active)
    );
    assert_eq!(
        journal.propose(SegmentId::from_raw(8)),
        Err(Refusal::Unknown)
    );
    let first = journal.propose(sealed[0]).expect("propose");
    assert_eq!((first.segment(), first.attempt()), (sealed[0], 4));
    assert_eq!(journal.propose(sealed[0]), Err(Refusal::Busy));
    assert_eq!(state(&journal, sealed[0]), Some(SegmentState::Proposed));
    assert_eq!(
        journal.segment(sealed[0]).and_then(|s| s.replacement),
        Some(first)
    );
    assert_eq!(journal.reclaim(first), Err(Refusal::NotDurable));
    let too_many = Durable::Rewritten {
        bytes: 1,
        live_records: 11,
    };
    assert_eq!(
        journal.confirm_durable(first, too_many),
        Err(Refusal::Exceeds)
    );
    let rewritten = Durable::Rewritten {
        bytes: 30,
        live_records: 4,
    };
    assert_eq!(
        journal.confirm_durable(first, rewritten),
        Ok(Confirmed::Recorded)
    );
    assert_eq!(state(&journal, sealed[0]), Some(SegmentState::Reclaimable));
    assert_eq!(ids(journal.reclaimable(None)), [1]);
    let reclaimed = journal.reclaim(first).expect("reclaim");
    assert_eq!(
        reclaimed,
        Reclaimed {
            segment: sealed[0],
            revision: 0,
            bytes: 100,
            replaced_by: Some(4)
        }
    );
    let after = journal.segment(sealed[0]).expect("kept");
    assert_eq!((after.revision, after.bytes, after.records), (4, 30, 10));
    assert_eq!((after.stored_records, after.live_records), (4, 4));
    assert_eq!(
        (after.state, after.replacement),
        (SegmentState::Sealed, None)
    );
    assert_eq!(journal.reclaim(first), Err(Refusal::Stale));
    assert_eq!(journal.abort(first), Err(Refusal::Stale));
    assert_eq!(
        journal.confirm_durable(first, rewritten),
        Err(Refusal::Stale)
    );

    let second = journal.propose(sealed[1]).expect("propose");
    assert_eq!(
        journal.confirm_durable(second, Durable::Superseded),
        Ok(Confirmed::Recorded)
    );
    let gone = journal.reclaim(second).expect("reclaim");
    assert_eq!((gone.replaced_by, gone.bytes), (None, 100));
    assert_eq!(journal.segment(sealed[1]), None);
    assert_eq!(ids(journal.segments(None)), [1, 3]);
    assert_eq!(journal.covering(15), None, "a superseded range is a hole");
}

#[test]
fn aborted_attempts_leave_stale_handles_and_live_counts_carry_over() {
    let (mut journal, sealed) = journal_with(1);
    let old = journal.propose(sealed[0]).expect("propose");
    assert_eq!(
        journal.confirm_durable(old, Durable::Superseded),
        Ok(Confirmed::Recorded)
    );
    assert_eq!(journal.abort(old), Ok(()));
    assert_eq!(journal.abort(old), Err(Refusal::Stale));
    let new = journal.propose(sealed[0]).expect("propose");
    assert_ne!(old, new);
    assert_eq!(journal.reclaim(old), Err(Refusal::Stale));
    assert_eq!(
        journal.confirm_durable(old, Durable::Superseded),
        Err(Refusal::Stale)
    );
    let rewritten = Durable::Rewritten {
        bytes: 9,
        live_records: 6,
    };
    assert_eq!(
        journal.confirm_durable(new, rewritten),
        Ok(Confirmed::Recorded)
    );
    assert_eq!(journal.observe_live(sealed[0], 3), Ok(()));
    assert!(journal.reclaim(new).is_ok());
    let segment = journal.segment(sealed[0]).expect("kept");
    assert_eq!(
        (
            segment.stored_records,
            segment.live_records,
            segment.revision
        ),
        (6, 3, 4)
    );
}
