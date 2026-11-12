//! Pins, reclamation gating and integer limits.
use offset_machines::{Ack, Assign, Commit, Tracker};

use super::{Small, ids, journal_with, policy};
use crate::*;

#[test]
fn reclaim_waits_for_every_pin_of_either_kind() {
    let (mut journal, sealed) = journal_with(2);
    let reader = journal.pin_reader(3).expect("reader");
    let checkpoint = journal.pin_checkpoint(1, 12).expect("checkpoint");
    assert_eq!(
        (reader.kind(), checkpoint.kind()),
        (PinKind::Reader, PinKind::Checkpoint)
    );
    assert_eq!(journal.pin_reader(0), Err(PinError::Full));
    assert_eq!((journal.pins(), journal.low_pin()), (2, Some(3)));
    let replacement = journal.propose(sealed[0]).expect("propose");
    assert_eq!(
        journal.confirm_durable(replacement, Durable::Superseded),
        Confirm::Confirmed
    );
    let view = journal.segment(sealed[0]).expect("segment");
    assert_eq!((view.state, view.pinned), (SegmentState::Durable, true));
    assert_eq!(journal.reclaimable(None).count(), 0);
    let before = journal.clone();
    assert_eq!(
        journal.reclaim(replacement),
        Err(ReclaimError::Pinned { offset: 3 })
    );
    assert_eq!(journal, before);
    assert_eq!(journal.advance(reader, 9), PinUpdate::Advanced);
    assert_eq!(
        journal.reclaim(replacement),
        Err(ReclaimError::Pinned { offset: 9 }),
        "offset 9 is still inside 0..10"
    );
    assert_eq!(journal.advance(reader, 10), PinUpdate::Advanced);
    assert_eq!(ids(journal.reclaimable(None)), [1]);

    let later = journal.propose(sealed[1]).expect("propose");
    assert_eq!(
        journal.confirm_durable(later, Durable::Superseded),
        Confirm::Confirmed
    );
    assert_eq!(
        journal.reclaim(later),
        Err(ReclaimError::Pinned { offset: 10 })
    );
    assert_eq!(journal.release(reader), Release::Released);
    assert_eq!(
        journal.reclaim(later),
        Err(ReclaimError::Pinned { offset: 12 }),
        "the checkpoint alone still holds 10..20"
    );
    assert_eq!(journal.checkpoint(checkpoint, 1, 20), PinUpdate::Advanced);
    assert!(journal.reclaim(later).is_ok());
    assert!(journal.reclaim(replacement).is_ok());
    assert_eq!(ids(journal.segments(None)), [3]);
}

#[test]
fn release_is_checked_against_the_acquisition_generation() {
    let (mut journal, _) = journal_with(0);
    let first = journal.pin_reader(5).expect("pin");
    assert_eq!(journal.pin_offset(first), Some(5));
    assert_eq!(journal.release(first), Release::Released);
    assert_eq!(journal.release(first), Release::Stale);
    let second = journal.pin_reader(1).expect("pin");
    assert_ne!(first.generation(), second.generation());
    let before = journal.clone();
    assert_eq!(
        journal.release(first),
        Release::Stale,
        "same slot, older generation"
    );
    assert_eq!(journal.advance(first, 50), PinUpdate::Stale);
    assert_eq!(journal.checkpoint(first, 1, 50), PinUpdate::Stale);
    assert_eq!(journal.pin_offset(first), None);
    assert_eq!(journal, before);
    assert_eq!(journal.low_pin(), Some(1));
    assert_eq!(journal.advance(second, 0), PinUpdate::Regressed);
    assert_eq!(journal.advance(second, 1), PinUpdate::Unchanged);
    assert_eq!(journal.checkpoint(second, 1, 9), PinUpdate::WrongKind);
    assert_eq!(journal.release(second), Release::Released);
    assert_eq!((journal.pins(), journal.low_pin()), (0, None));
}

#[test]
fn checkpoint_pins_reject_older_assignment_generations() {
    let (mut journal, _) = journal_with(0);
    assert_eq!(journal.pin_checkpoint(0, 5), Err(PinError::Unassigned));
    let pin = journal.pin_checkpoint(3, 20).expect("pin");
    assert_eq!(journal.advance(pin, 30), PinUpdate::WrongKind);
    assert_eq!(journal.checkpoint(pin, 0, 30), PinUpdate::Unassigned);
    assert_eq!(journal.checkpoint(pin, 2, 99), PinUpdate::StaleAssignment);
    assert_eq!(journal.checkpoint(pin, 3, 19), PinUpdate::Regressed);
    assert_eq!(journal.checkpoint(pin, 3, 20), PinUpdate::Unchanged);
    assert_eq!(journal.pin_offset(pin), Some(20));
    assert_eq!(journal.checkpoint(pin, 3, 25), PinUpdate::Advanced);
    assert_eq!(journal.checkpoint(pin, 4, 8), PinUpdate::Reassigned);
    assert_eq!(
        journal.pin_offset(pin),
        Some(8),
        "the new owner reports its own base"
    );
    assert_eq!(journal.checkpoint(pin, 3, 99), PinUpdate::StaleAssignment);
    assert_eq!(
        journal.checkpoint(pin, u64::MAX, u64::MAX),
        PinUpdate::Reassigned
    );
    assert_eq!(
        journal.checkpoint(pin, u64::MAX, u64::MAX),
        PinUpdate::Unchanged
    );
}

#[test]
fn tracker_committed_base_drives_the_checkpoint_pin() {
    let (mut journal, sealed) = journal_with(1);
    let mut tracker = Tracker::<1>::new();
    assert_eq!(
        journal.pin_checkpoint_from(&tracker),
        Err(PinError::Unassigned)
    );
    assert_eq!(tracker.assign(1, 0), Assign::Assigned);
    let pin = journal.pin_checkpoint_from(&tracker).expect("pin");
    let replacement = journal.propose(sealed[0]).expect("propose");
    assert_eq!(
        journal.confirm_durable(replacement, Durable::Superseded),
        Confirm::Confirmed
    );
    for offset in 0..10 {
        assert_eq!(tracker.ack(1, offset), Ack::Ok);
    }
    assert_eq!(journal.checkpoint_from(pin, &tracker), PinUpdate::Unchanged);
    assert_eq!(
        journal.reclaim(replacement),
        Err(ReclaimError::Pinned { offset: 0 }),
        "acknowledged is not committed"
    );
    assert_eq!(tracker.mark_committed(1, 10), Commit::Advanced);
    assert_eq!(journal.checkpoint_from(pin, &tracker), PinUpdate::Advanced);
    assert_eq!(journal.pin_offset(pin), Some(10));
    assert!(journal.reclaim(replacement).is_ok());
    let mut zombie = Tracker::<1>::new();
    assert_eq!(tracker.reassign(4), Some(2));
    assert_eq!(
        journal.checkpoint_from(pin, &tracker),
        PinUpdate::Reassigned
    );
    assert_eq!(zombie.assign(1, 60), Assign::Assigned);
    assert_eq!(
        journal.checkpoint_from(pin, &zombie),
        PinUpdate::StaleAssignment
    );
    assert_eq!(journal.pin_offset(pin), Some(4));
}

#[test]
fn u64_limits_refuse_instead_of_wrapping() {
    let mut journal = Small::new(policy(0, 0, 0, 500), u64::MAX - 5, u64::MAX);
    assert_eq!(
        journal.roll_decision(1, 6, u64::MAX),
        Err(RollError::Overflow)
    );
    assert_eq!(journal.roll(1, 6, u64::MAX), Err(RollError::Overflow));
    let rolled = journal.roll(u64::MAX, 5, u64::MAX).expect("roll");
    assert_eq!(rolled.next_offset, u64::MAX);
    assert_eq!(
        journal.segment(rolled.sealed).map(|s| s.end_offset()),
        Some(u64::MAX)
    );
    assert_eq!(journal.roll(1, 1, u64::MAX), Err(RollError::Overflow));
    let top = journal.pin_reader(u64::MAX).expect("pin");
    assert_eq!(journal.advance(top, u64::MAX), PinUpdate::Unchanged);
    assert_eq!(
        journal.segment(rolled.sealed).map(|s| s.pinned),
        Some(false)
    );

    let mut worn = Small::new(policy(0, 0, 0, 500), 0, 0);
    let sealed = worn.roll(1, 1, 0).expect("roll").sealed;
    worn.issued = u64::MAX;
    worn.attempts = u64::MAX;
    worn.pin_generations = u64::MAX;
    let before = worn.clone();
    assert_eq!(worn.roll(1, 1, 0), Err(RollError::Overflow));
    assert_eq!(worn.propose(sealed), Err(ProposeError::Exhausted));
    assert_eq!(worn.pin_reader(0), Err(PinError::Exhausted));
    assert_eq!(worn.pin_checkpoint(1, 0), Err(PinError::Exhausted));
    assert_eq!(worn, before);

    let huge = Small::new(policy(0, 0, 0, 1000), 0, 0);
    assert!(huge.policy().compactable(u64::MAX, 0));
    assert!(!huge.policy().compactable(u64::MAX, 1));
}
