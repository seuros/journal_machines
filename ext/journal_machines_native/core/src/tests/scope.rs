//! Every identity the journal issues carries its scope.
use std::vec::Vec;

use matryona_machines::testing::refuses;
use periscope_machines::{SEQUENCE_MAX, scope_of, scoped};

use super::{Small, at, open, policy, worn};
use crate::*;

/// Everything one journal hands out, built by the same calls in any scope.
struct Issued {
    journal: Small,
    sealed: SegmentId,
    reader: Pin,
    checkpoint: Pin,
    released: Pin,
    proposed: Replacement,
    durable: Replacement,
}

fn issued(scope: u16) -> Issued {
    let mut journal = open(scope);
    let sealed: Vec<SegmentId> = (0..2)
        .map(|_| journal.roll(100, 10, at(0)).expect("roll").sealed)
        .collect();
    let released = journal.pin_reader(30).expect("pin");
    assert_eq!(journal.release(released), Ok(()));
    let reader = journal.pin_reader(30).expect("pin");
    let checkpoint = journal.pin_checkpoint(3, 30).expect("pin");
    let proposed = journal.propose(sealed[0]).expect("propose");
    let durable = journal.propose(sealed[1]).expect("propose");
    assert_eq!(
        journal.confirm_durable(durable, Durable::Superseded),
        Ok(Confirmed::Recorded)
    );
    Issued {
        journal,
        sealed: sealed[0],
        reader,
        checkpoint,
        released,
        proposed,
        durable,
    }
}

#[test]
fn a_handle_from_another_journal_names_nothing() {
    // The review's sequence: two journals, the same calls, every handle of
    // one landing on a live entry of the other.
    let (mut near, mut far) = (issued(1), issued(2));
    assert_ne!(near.reader, far.reader);
    assert_eq!(near.reader.slot, far.reader.slot);
    refuses_every_handle_of(&mut near.journal, &far);
    refuses_every_handle_of(&mut far.journal, &near);
    each_still_honours_its_own(near);
}

fn refuses_every_handle_of(journal: &mut Small, other: &Issued) {
    let before = journal.clone();
    // Pins: live ones move and release nothing, repeats are not honoured.
    assert_eq!(journal.advance(other.reader, 30), Err(Refusal::Stale));
    assert_eq!(journal.advance(other.reader, 31), Err(Refusal::Stale));
    let checkpoint = other.checkpoint;
    assert_eq!(journal.checkpoint(checkpoint, 3, 30), Err(Refusal::Stale));
    assert_eq!(journal.checkpoint(checkpoint, 4, 0), Err(Refusal::Stale));
    let tracker = offset_machines::Tracker::<1>::new();
    assert_eq!(
        journal.checkpoint_from(checkpoint, &tracker),
        Err(Refusal::Stale),
        "refused before the tracker is read"
    );
    assert_eq!(journal.release(other.reader), Err(Refusal::Stale));
    assert_eq!(journal.release(other.released), Err(Refusal::Stale));
    assert_eq!(journal.pin_offset(other.reader), None);
    // Replacements: nothing confirmed, repeated, reclaimed or aborted.
    let rewritten = Durable::Rewritten {
        bytes: 1,
        live_records: 1,
    };
    assert_eq!(
        journal.confirm_durable(other.proposed, rewritten),
        Err(Refusal::Stale)
    );
    assert_eq!(
        journal.confirm_durable(other.durable, Durable::Superseded),
        Err(Refusal::Stale)
    );
    assert_eq!(journal.reclaim(other.durable), Err(Refusal::Stale));
    assert_eq!(journal.reclaim(other.proposed), Err(Refusal::Stale));
    assert_eq!(journal.abort(other.proposed), Err(Refusal::Stale));
    assert_eq!(journal.abort(other.durable), Err(Refusal::Stale));
    // Segment ids: no such segment, and no position to resume from.
    assert_eq!(journal.segment(other.sealed), None);
    assert_eq!(journal.observe_live(other.sealed, 1), Err(Refusal::Unknown));
    assert_eq!(journal.propose(other.sealed), Err(Refusal::Unknown));
    assert_eq!(journal.segments(Some(other.sealed)).count(), 0);
    assert_eq!(journal.compaction_candidates(Some(other.sealed)).count(), 0);
    assert_eq!(journal.reclaimable(Some(other.sealed)).count(), 0);
    assert_eq!(*journal, before);
}

/// Each journal still honours its own, repeats included.
fn each_still_honours_its_own(mut near: Issued) {
    let journal = &mut near.journal;
    assert_eq!(journal.advance(near.reader, 30), Ok(Advanced::Unchanged));
    assert_eq!(
        journal.checkpoint(near.checkpoint, 3, 30),
        Ok(Checkpointed::Unchanged)
    );
    assert_eq!(journal.release(near.reader), Ok(()));
    assert_eq!(
        journal.confirm_durable(near.durable, Durable::Superseded),
        Ok(Confirmed::AlreadyDurable)
    );
    assert_eq!(journal.segments(Some(near.sealed)).count(), 2);
}

#[test]
fn a_handle_with_one_foreign_half_is_stale() {
    let (mut near, far) = (issued(1), issued(2));
    let before = near.journal.clone();
    for mixed in [
        Replacement {
            segment: near.durable.segment,
            attempt: far.durable.attempt,
        },
        Replacement {
            segment: far.durable.segment,
            attempt: near.durable.attempt,
        },
    ] {
        assert_eq!(
            near.journal.confirm_durable(mixed, Durable::Superseded),
            Err(Refusal::Stale)
        );
        assert_eq!(near.journal.reclaim(mixed), Err(Refusal::Stale));
        assert_eq!(near.journal.abort(mixed), Err(Refusal::Stale));
    }
    assert_eq!(near.journal, before);
}

#[test]
fn two_journals_left_in_one_scope_still_alias() {
    // Nothing tells them apart: keeping scopes distinct is the caller's job.
    let (mut one, other) = (issued(5), issued(5));
    assert_eq!(one.reader, other.reader);
    assert_eq!(one.journal.advance(other.reader, 31), Ok(Advanced::Moved));
    assert_eq!(one.journal.release(other.checkpoint), Ok(()));
    assert_eq!(one.journal.abort(other.proposed), Ok(()));
    assert_eq!(one.journal.observe_live(other.sealed, 1), Ok(()));
}

#[test]
fn scope_zero_issues_one_plain_counter() {
    let Issued {
        journal,
        sealed,
        reader,
        checkpoint,
        released,
        proposed,
        durable,
    } = issued(0);
    assert_eq!(journal.scope(), 0);
    assert_eq!(journal.active().map(|s| s.id.raw()), Some(3));
    assert_eq!(sealed.raw(), 1);
    let generations = [released, reader, checkpoint].map(Pin::generation);
    assert_eq!(generations, [4, 5, 6], "after the three segment ids");
    assert_eq!([proposed, durable].map(Replacement::attempt), [7, 8]);
    assert_eq!(journal.issued(), 8);
    assert_eq!(
        Small::new(policy(0, 0, 0, 500), 0, at(0)),
        Small::with_scope(policy(0, 0, 0, 500), 0, at(0), 0)
    );

    let high = issued(9);
    assert_eq!(high.journal.scope(), 9);
    assert_eq!((high.sealed.raw(), high.sealed.scope()), (scoped(9, 1), 9));
    assert_eq!(
        (high.reader.generation(), high.reader.scope()),
        (scoped(9, 5), 9)
    );
    assert_eq!(
        (high.durable.attempt(), high.durable.scope()),
        (scoped(9, 8), 9)
    );
    let view = high.journal.segment(high.sealed).expect("segment");
    assert_eq!(view.replacement, Some(high.proposed));
}

/// Every call that issues is `Exhausted` and changes nothing.
fn is_spent(journal: &mut Small, sealed: SegmentId) {
    assert_eq!(journal.issued(), SEQUENCE_MAX);
    refuses(journal, Refusal::Exhausted, |j| j.roll(1, 1, at(0)));
    refuses(journal, Refusal::Exhausted, |j| j.propose(sealed));
    refuses(journal, Refusal::Exhausted, |j| j.pin_reader(0));
    refuses(journal, Refusal::Exhausted, |j| j.pin_checkpoint(1, 0));
}

#[test]
fn the_last_identity_goes_to_any_kind_then_the_scope_is_spent() {
    // As a segment id.
    let mut journal = worn(u16::MAX, 1);
    let last = journal.roll(1, 1, at(0)).expect("roll");
    assert_eq!(last.opened.raw(), u64::MAX);
    is_spent(&mut journal, last.sealed);
    // As an attempt.
    let mut journal = worn(u16::MAX, 2);
    let sealed = journal.roll(1, 1, at(0)).expect("roll").sealed;
    let attempt = journal.propose(sealed).expect("propose");
    assert_eq!(attempt.attempt(), u64::MAX);
    assert_eq!(journal.abort(attempt), Ok(()));
    is_spent(&mut journal, sealed);
    // As a pin generation, of either kind.
    for checkpoint in [false, true] {
        let mut journal = worn(u16::MAX, 2);
        let sealed = journal.roll(1, 1, at(0)).expect("roll").sealed;
        let pin = if checkpoint {
            journal.pin_checkpoint(1, 0)
        } else {
            journal.pin_reader(0)
        }
        .expect("pin");
        assert_eq!(pin.generation(), u64::MAX);
        is_spent(&mut journal, sealed);
        // What was issued last still works.
        assert_eq!(journal.release(pin), Ok(()));
        assert_eq!(journal.release(pin), Err(Refusal::Stale));
    }
}

#[test]
fn a_refusal_for_another_reason_consumes_no_identity() {
    let mut journal = worn(0, 1);
    let active = journal.active().expect("active").id;
    let before = journal.clone();
    assert_eq!(journal.roll(1, 0, at(0)), Err(Refusal::Empty));
    assert_eq!(journal.propose(active), Err(Refusal::Active));
    assert_eq!(journal.pin_checkpoint(0, 0), Err(Refusal::Unassigned));
    let mut full = Journal::<4, 0>::resumed(policy(0, 0, 0, 500), 0, at(0), 0, before.issued() - 1)
        .expect("resumed");
    assert_eq!(full.pin_reader(0), Err(Refusal::PinsFull));
    assert_eq!(full.issued(), before.issued());
    assert_eq!(journal, before);
    assert_eq!(
        journal.roll(1, 1, at(0)).map(|r| r.opened.raw()),
        Ok(SEQUENCE_MAX)
    );
}

#[test]
fn no_two_handles_of_a_journal_share_a_raw_value() {
    // The review's finding: in scope 3 the first segment id, pin generation
    // and attempt were all 0x3000000000001, so one rebuilt as another kind
    // named something real.
    let Issued {
        mut journal,
        sealed,
        reader,
        checkpoint,
        released,
        proposed,
        durable,
    } = issued(3);
    let active = journal.active().expect("active").id;
    let mut raws = Vec::from([sealed.raw(), durable.segment().raw(), active.raw()]);
    raws.extend([released, reader, checkpoint].map(Pin::generation));
    raws.extend([proposed, durable].map(Replacement::attempt));
    let mut distinct = raws.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), raws.len(), "raw values: {raws:x?}");
    assert!(raws.iter().all(|raw| scope_of(*raw) == 3));
    // An attempt (also a revision) or a pin generation rebuilt as a segment
    // id names no segment.
    let before = journal.clone();
    for raw in [proposed.attempt(), durable.attempt(), reader.generation()] {
        let id = SegmentId::from_raw(raw);
        assert_eq!(journal.segment(id), None);
        assert_eq!(journal.propose(id), Err(Refusal::Unknown));
        assert_eq!(journal.observe_live(id, 0), Err(Refusal::Unknown));
    }
    assert_eq!(journal, before);
}
