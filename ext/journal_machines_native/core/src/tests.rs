#![allow(clippy::expect_used, clippy::unwrap_used)]
use core::num::NonZeroU64;
use std::vec::Vec;

use super::*;

mod model;
mod pins;
mod random;

type Small = Journal<4, 2>;

fn policy(bytes: u64, records: u64, age: u64, per_mille: u16) -> Policy {
    Policy::new(
        NonZeroU64::new(bytes),
        NonZeroU64::new(records),
        NonZeroU64::new(age),
        per_mille,
    )
    .expect("policy")
}

/// A journal with `sealed` ten-record, hundred-byte segments behind the active one.
fn journal_with(sealed: u64) -> (Small, Vec<SegmentId>) {
    let mut journal = Small::new(policy(0, 0, 0, 500), 0, 0);
    let ids = (0..sealed)
        .map(|_| journal.roll(100, 10, 0).expect("roll").sealed)
        .collect();
    (journal, ids)
}

fn ids(segments: Segments<'_, 4, 2>) -> Vec<u64> {
    segments.map(|segment| segment.id.raw()).collect()
}

#[test]
fn roll_decision_follows_size_then_records_then_age() {
    let journal = Small::new(policy(100, 10, 50, 0), 0, 1000);
    assert_eq!(journal.roll_decision(99, 9, 1049), Ok(Roll::Keep));
    assert_eq!(
        journal.roll_decision(100, 9, 1000),
        Ok(Roll::Due(RollReason::Size))
    );
    assert_eq!(
        journal.roll_decision(1, 10, 1000),
        Ok(Roll::Due(RollReason::Records))
    );
    assert_eq!(
        journal.roll_decision(1, 1, 1050),
        Ok(Roll::Due(RollReason::Age))
    );
    assert_eq!(
        journal.roll_decision(100, 10, 1050),
        Ok(Roll::Due(RollReason::Size))
    );
    assert_eq!(
        journal.roll_decision(1, 10, 1050),
        Ok(Roll::Due(RollReason::Records))
    );
    assert_eq!(journal.roll_decision(0, 0, u64::MAX), Ok(Roll::Keep));
    assert_eq!(
        journal.roll_decision(1, 1, 999),
        Err(RollError::TimeRegression)
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
    let journal = Small::new(never, 0, 0);
    assert_eq!(
        journal.roll_decision(u64::MAX, u64::MAX, u64::MAX),
        Ok(Roll::Keep)
    );
    assert_eq!(journal.policy(), never);
    assert!(Policy::new(None, None, None, 1001).is_none());
}

#[test]
fn roll_seals_the_active_segment_and_opens_the_next() {
    let mut journal = Small::new(policy(0, 0, 0, 500), 100, 7);
    assert_eq!(journal.roll(64, 0, 7), Err(RollError::Empty));
    assert_eq!(journal.roll(64, 5, 6), Err(RollError::TimeRegression));
    let rolled = journal.roll(64, 5, 9).expect("roll");
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
    assert_eq!((sealed.opened_at, sealed.revision), (7, 0));
    let active = journal.active().expect("active");
    assert_eq!(
        (active.id, active.base_offset, active.opened_at),
        (rolled.opened, 105, 9)
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
fn full_journal_refuses_to_roll_without_changing() {
    let (mut journal, sealed) = journal_with(3);
    assert_eq!((journal.len(), Small::SEGMENT_CAPACITY), (4, 4));
    let before = journal.clone();
    assert_eq!(journal.roll(1, 1, 0), Err(RollError::Full));
    assert_eq!(journal, before);
    assert_eq!(
        journal.roll_decision(1, 1, 0),
        Ok(Roll::Keep),
        "deciding reads only"
    );
    let replacement = journal.propose(sealed[0]).expect("propose");
    assert_eq!(
        journal.confirm_durable(replacement, Durable::Superseded),
        Confirm::Confirmed
    );
    assert!(journal.reclaim(replacement).is_ok());
    assert_eq!(
        journal.roll(1, 1, 0).map(|rolled| rolled.opened.raw()),
        Ok(5)
    );
}

#[test]
fn zero_capacity_journal_refuses_everything() {
    let mut journal = Journal::<0, 0>::new(policy(1, 1, 1, 0), 0, 0);
    assert!(journal.is_empty());
    assert_eq!(journal.active(), None);
    assert_eq!(journal.roll_decision(1, 1, 1), Err(RollError::NoActive));
    assert_eq!(journal.roll(1, 1, 1), Err(RollError::NoActive));
    assert_eq!(journal.pin_reader(0), Err(PinError::Full));
    assert_eq!(
        journal.propose(SegmentId::from_raw(1)),
        Err(ProposeError::Unknown)
    );
    assert_eq!(journal.segments(None).count(), 0);
}

#[test]
fn candidates_need_enough_dead_records_and_resume_by_identity() {
    let (mut journal, sealed) = journal_with(3);
    assert_eq!(ids(journal.compaction_candidates(None)), []);
    assert_eq!(journal.observe_live(sealed[0], 5), Observe::Recorded);
    assert_eq!(journal.observe_live(sealed[1], 6), Observe::Recorded);
    assert_eq!(journal.observe_live(sealed[2], 0), Observe::Recorded);
    assert_eq!(journal.observe_live(sealed[2], 11), Observe::Exceeds);
    assert_eq!(
        journal.observe_live(SegmentId::from_raw(4), 0),
        Observe::Active
    );
    assert_eq!(
        journal.observe_live(SegmentId::from_raw(9), 0),
        Observe::Unknown
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
    assert_eq!(journal.abort(replacement), Abort::Aborted);
    assert_eq!(ids(journal.compaction_candidates(None)), [1, 3]);
    let mut strict = Small::new(policy(0, 0, 0, 0), 0, 0);
    let all_live = strict.roll(1, 3, 0).expect("roll").sealed;
    assert_eq!(
        ids(strict.compaction_candidates(None)),
        [],
        "nothing dead, nothing to do"
    );
    assert_eq!(strict.observe_live(all_live, 2), Observe::Recorded);
    assert_eq!(ids(strict.compaction_candidates(None)), [1]);
}

#[test]
fn replacement_moves_from_proposed_to_durable_to_reclaimed() {
    let (mut journal, sealed) = journal_with(2);
    let state = |journal: &Small, id| journal.segment(id).map(|segment| segment.state);
    assert_eq!(
        journal.propose(SegmentId::from_raw(3)),
        Err(ProposeError::Active)
    );
    assert_eq!(
        journal.propose(SegmentId::from_raw(8)),
        Err(ProposeError::Unknown)
    );
    let first = journal.propose(sealed[0]).expect("propose");
    assert_eq!((first.segment(), first.attempt()), (sealed[0], 1));
    assert_eq!(journal.propose(sealed[0]), Err(ProposeError::Busy));
    assert_eq!(state(&journal, sealed[0]), Some(SegmentState::Proposed));
    assert_eq!(
        journal.segment(sealed[0]).and_then(|s| s.replacement),
        Some(first)
    );
    assert_eq!(journal.reclaim(first), Err(ReclaimError::NotDurable));
    let too_many = Durable::Rewritten {
        bytes: 1,
        live_records: 11,
    };
    assert_eq!(journal.confirm_durable(first, too_many), Confirm::Invalid);
    let rewritten = Durable::Rewritten {
        bytes: 30,
        live_records: 4,
    };
    assert_eq!(
        journal.confirm_durable(first, rewritten),
        Confirm::Confirmed
    );
    assert_eq!(
        journal.confirm_durable(first, Durable::Superseded),
        Confirm::AlreadyDurable
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
            replaced_by: Some(1)
        }
    );
    let after = journal.segment(sealed[0]).expect("kept");
    assert_eq!((after.revision, after.bytes, after.records), (1, 30, 10));
    assert_eq!((after.stored_records, after.live_records), (4, 4));
    assert_eq!(
        (after.state, after.replacement),
        (SegmentState::Sealed, None)
    );
    assert_eq!(journal.reclaim(first), Err(ReclaimError::Stale));
    assert_eq!(journal.abort(first), Abort::Stale);
    assert_eq!(journal.confirm_durable(first, rewritten), Confirm::Stale);

    let second = journal.propose(sealed[1]).expect("propose");
    assert_eq!(
        journal.confirm_durable(second, Durable::Superseded),
        Confirm::Confirmed
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
        Confirm::Confirmed
    );
    assert_eq!(journal.abort(old), Abort::Aborted);
    assert_eq!(journal.abort(old), Abort::Stale);
    let new = journal.propose(sealed[0]).expect("propose");
    assert_ne!(old, new);
    assert_eq!(journal.reclaim(old), Err(ReclaimError::Stale));
    assert_eq!(
        journal.confirm_durable(old, Durable::Superseded),
        Confirm::Stale
    );
    let rewritten = Durable::Rewritten {
        bytes: 9,
        live_records: 6,
    };
    assert_eq!(journal.confirm_durable(new, rewritten), Confirm::Confirmed);
    assert_eq!(journal.observe_live(sealed[0], 3), Observe::Recorded);
    assert!(journal.reclaim(new).is_ok());
    let segment = journal.segment(sealed[0]).expect("kept");
    assert_eq!(
        (
            segment.stored_records,
            segment.live_records,
            segment.revision
        ),
        (6, 3, 2)
    );
}
