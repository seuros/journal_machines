//! Restarts: a journal resumes its sequence from a saved count.
use std::vec::Vec;

use periscope_machines::{SEQUENCE_MAX, scoped};

use super::{Small, at, policy};
use crate::*;

fn resumed(scope: u16, issued: u64) -> Option<Small> {
    Small::resumed(policy(0, 0, 0, 500), 0, at(0), scope, issued)
}

/// One segment id, one pin generation and one attempt, in raw form.
fn use_up(journal: &mut Small) -> (Pin, Replacement, Vec<u64>) {
    let rolled = journal.roll(1, 1, at(0)).expect("roll");
    let pin = journal.pin_reader(0).expect("pin");
    let attempt = journal.propose(rolled.sealed).expect("propose");
    let raws = Vec::from([rolled.opened.raw(), pin.generation(), attempt.attempt()]);
    (pin, attempt, raws)
}

#[test]
fn resuming_from_nothing_issued_is_a_fresh_journal() {
    for scope in [0, 9, u16::MAX] {
        let fresh = Small::with_scope(policy(0, 0, 0, 500), 0, at(0), scope);
        assert_eq!(resumed(scope, 0), Some(fresh.clone()));
        assert_eq!((fresh.scope(), fresh.issued()), (scope, 1));
        assert_eq!(fresh.active().map(|s| s.id.raw()), Some(scoped(scope, 1)));
    }
}

#[test]
fn a_resumed_journal_continues_the_sequence_and_reissues_nothing() {
    for scope in [0, 9, u16::MAX] {
        let mut first = Small::with_scope(policy(0, 0, 0, 500), 0, at(0), scope);
        let (pin, attempt, mut seen) = use_up(&mut first);
        seen.push(scoped(scope, 1));
        assert_eq!(first.issued(), 4);

        let mut second = resumed(scope, first.issued()).expect("resumed");
        assert_eq!((second.scope(), second.issued()), (scope, 5));
        let active = second.active().expect("active").id;
        assert_eq!(active.raw(), scoped(scope, 5));
        let (_, _, later) = use_up(&mut second);
        assert_eq!(later, [6, 7, 8].map(|sequence| scoped(scope, sequence)));
        assert!(
            seen.iter()
                .all(|raw| !later.contains(raw) && *raw != active.raw())
        );

        // What outlived the restart names nothing in the new incarnation.
        let before = second.clone();
        assert_eq!(second.release(pin), Err(Refusal::Stale));
        assert_eq!(second.advance(pin, 9), Err(Refusal::Stale));
        assert_eq!(second.abort(attempt), Err(Refusal::Stale));
        assert_eq!(second.reclaim(attempt), Err(Refusal::Stale));
        for raw in seen {
            assert_eq!(second.segment(SegmentId::from_raw(raw)), None);
            assert_eq!(
                second.propose(SegmentId::from_raw(raw)),
                Err(Refusal::Unknown)
            );
        }
        assert_eq!(second, before);
    }
}

#[test]
fn a_journal_started_again_in_its_scope_reissues() {
    // The hazard `resumed` closes: without the saved count, an old handle
    // names a new entry.
    let mut first = Small::with_scope(policy(0, 0, 0, 500), 0, at(0), 9);
    let old = first.pin_reader(5).expect("pin");
    let mut again = Small::with_scope(policy(0, 0, 0, 500), 0, at(0), 9);
    let new = again.pin_checkpoint(1, 7).expect("pin");
    assert_eq!(old.generation(), new.generation());
    assert_eq!(
        again.release(old),
        Ok(()),
        "the old handle released a new pin"
    );
}

#[test]
fn resuming_refuses_a_count_no_scope_holds() {
    assert!(resumed(0, SEQUENCE_MAX + 1).is_none());
    assert!(resumed(9, u64::MAX).is_none());
    assert!(
        resumed(9, scoped(9, 4)).is_none(),
        "a raw id is not a count"
    );
    assert!(
        resumed(9, SEQUENCE_MAX).is_none(),
        "no identity is left for the active segment"
    );
    let last = resumed(9, SEQUENCE_MAX - 1).expect("resumed");
    assert_eq!(
        last.active().map(|s| s.id.raw()),
        Some(scoped(9, SEQUENCE_MAX))
    );
    assert_eq!(last.issued(), SEQUENCE_MAX);
    // Zero segment capacity needs no identity.
    let none = Journal::<0, 1>::resumed(policy(0, 0, 0, 500), 0, at(0), 9, SEQUENCE_MAX);
    assert_eq!(
        none.map(|journal| (journal.len(), journal.issued())),
        Some((0, SEQUENCE_MAX))
    );
    assert!(
        Journal::<0, 1>::resumed(policy(0, 0, 0, 500), 0, at(0), 9, SEQUENCE_MAX + 1).is_none()
    );
}
