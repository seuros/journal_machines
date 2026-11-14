//! Pins, reclamation gating and integer limits.
use offset_machines::{Ack, Commit, Tracker};

use super::{Small, at, ids, journal_with, policy};
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
    assert_eq!(journal.pin_reader(0), Err(Refusal::PinsFull));
    assert_eq!((journal.pins(), journal.low_pin()), (2, Some(3)));
    let replacement = journal.propose(sealed[0]).expect("propose");
    assert_eq!(
        journal.confirm_durable(replacement, Durable::Superseded),
        Ok(Confirmed::Recorded)
    );
    let view = journal.segment(sealed[0]).expect("segment");
    assert_eq!((view.state, view.pinned), (SegmentState::Durable, true));
    assert_eq!(journal.reclaimable(None).count(), 0);
    let before = journal.clone();
    assert_eq!(
        journal.reclaim(replacement),
        Err(Refusal::Pinned { offset: 3 })
    );
    assert_eq!(journal, before);
    assert_eq!(journal.advance(reader, 9), Ok(Advanced::Moved));
    assert_eq!(
        journal.reclaim(replacement),
        Err(Refusal::Pinned { offset: 9 }),
        "offset 9 is still inside 0..10"
    );
    assert_eq!(journal.advance(reader, 10), Ok(Advanced::Moved));
    assert_eq!(ids(journal.reclaimable(None)), [1]);

    let later = journal.propose(sealed[1]).expect("propose");
    assert_eq!(
        journal.confirm_durable(later, Durable::Superseded),
        Ok(Confirmed::Recorded)
    );
    assert_eq!(journal.reclaim(later), Err(Refusal::Pinned { offset: 10 }));
    assert_eq!(journal.release(reader), Ok(()));
    assert_eq!(
        journal.reclaim(later),
        Err(Refusal::Pinned { offset: 12 }),
        "the checkpoint alone still holds 10..20"
    );
    assert_eq!(
        journal.checkpoint(checkpoint, 1, 20),
        Ok(Checkpointed::Advanced)
    );
    assert!(journal.reclaim(later).is_ok());
    assert!(journal.reclaim(replacement).is_ok());
    assert_eq!(ids(journal.segments(None)), [3]);
    assert_eq!(journal.release(checkpoint), Ok(()));
    assert_eq!((journal.pins(), journal.low_pin()), (0, None));
}

#[test]
fn tracker_committed_base_drives_the_checkpoint_pin() {
    let (mut journal, sealed) = journal_with(1);
    let mut tracker = Tracker::<1>::new();
    assert_eq!(
        journal.pin_checkpoint_from(&tracker),
        Err(Refusal::Unassigned)
    );
    assert_eq!(tracker.assign(1, 0), Ok(()));
    let pin = journal.pin_checkpoint_from(&tracker).expect("pin");
    let replacement = journal.propose(sealed[0]).expect("propose");
    assert_eq!(
        journal.confirm_durable(replacement, Durable::Superseded),
        Ok(Confirmed::Recorded)
    );
    for offset in 0..10 {
        assert_eq!(tracker.ack(1, offset), Ok(Ack::Recorded));
    }
    let before = journal.clone();
    assert_eq!(
        journal.checkpoint_from(pin, &tracker),
        Ok(Checkpointed::Unchanged)
    );
    assert_eq!(journal, before);
    assert_eq!(
        journal.reclaim(replacement),
        Err(Refusal::Pinned { offset: 0 }),
        "acknowledged is not committed"
    );
    assert_eq!(tracker.mark_committed(1, 10), Ok(Commit::Advanced));
    assert_eq!(
        journal.checkpoint_from(pin, &tracker),
        Ok(Checkpointed::Advanced)
    );
    assert_eq!(journal.pin_offset(pin), Some(10));
    assert!(journal.reclaim(replacement).is_ok());
    let mut zombie = Tracker::<1>::new();
    assert_eq!(tracker.reassign(4), Ok(2));
    assert_eq!(
        journal.checkpoint_from(pin, &tracker),
        Ok(Checkpointed::Reassigned)
    );
    assert_eq!(zombie.assign(1, 60), Ok(()));
    assert_eq!(
        journal.checkpoint_from(pin, &zombie),
        Err(Refusal::StaleAssignment)
    );
    assert_eq!(journal.pin_offset(pin), Some(4));
}

#[test]
fn u64_limits_refuse_instead_of_wrapping() {
    let mut journal = Small::new(policy(0, 0, 0, 500), u64::MAX - 5, at(u64::MAX));
    assert_eq!(
        journal.roll_decision(1, 6, at(u64::MAX)),
        Err(Refusal::Overflow)
    );
    assert_eq!(journal.roll(1, 6, at(u64::MAX)), Err(Refusal::Overflow));
    let rolled = journal.roll(u64::MAX, 5, at(u64::MAX)).expect("roll");
    assert_eq!(rolled.next_offset, u64::MAX);
    assert_eq!(
        journal.segment(rolled.sealed).map(|s| s.end_offset()),
        Some(u64::MAX)
    );
    assert_eq!(journal.roll(1, 1, at(u64::MAX)), Err(Refusal::Overflow));
    let top = journal.pin_reader(u64::MAX).expect("pin");
    assert_eq!(journal.advance(top, u64::MAX), Ok(Advanced::Unchanged));
    assert_eq!(
        journal.segment(rolled.sealed).map(|s| s.pinned),
        Some(false)
    );

    let huge = Small::new(policy(0, 0, 0, 1000), 0, at(0));
    assert!(huge.policy().compactable(u64::MAX, 0));
    assert!(!huge.policy().compactable(u64::MAX, 1));
}
