//! A repeat is honoured only when the journal can prove it: regressions for
//! repeats that were honoured on a handle or a payload the journal never saw,
//! and for the second release it can no longer prove.
use core::time::Duration;

use super::{Small, at, fresh, open};
use crate::*;

#[test]
fn releasing_a_pin_another_scope_issued_is_stale() {
    let (mut a, mut b) = (open(1), open(2));
    let p = a.pin_checkpoint(1, 0).expect("checkpoint");
    let q = b.pin_reader(0).expect("reader");
    assert_eq!(b.release(q), Ok(()));
    let before = b.clone();
    assert_eq!(
        b.release(p),
        Err(Refusal::Stale),
        "the same slot and sequence number under another scope is not a repeat"
    );
    assert_eq!(b, before);
    assert_eq!(a.pin_offset(p), Some(0), "the issuer still holds it");
}

#[test]
fn releasing_a_released_pin_is_stale_and_spares_the_slot() {
    let mut journal = fresh();
    let first = journal.pin_reader(3).expect("pin");
    let kept = journal.pin_checkpoint(2, 4).expect("pin");
    assert_eq!(journal.release(first), Ok(()));
    let before = journal.clone();
    assert_eq!(journal.release(first), Err(Refusal::Stale));
    assert_eq!(journal, before);
    let second = journal.pin_reader(7).expect("pin");
    assert_eq!((first.slot, first.kind()), (second.slot, second.kind()));
    let before = journal.clone();
    assert_eq!(journal.release(first), Err(Refusal::Stale));
    assert_eq!(journal, before, "the slot's new holder keeps it");
    assert_eq!(journal.pin_offset(second), Some(7));
    // Reader or checkpoint alike.
    assert_eq!(journal.release(kept), Ok(()));
    let before = journal.clone();
    assert_eq!(journal.release(kept), Err(Refusal::Stale));
    assert_eq!(journal, before);
    // Released is not live again for anything else.
    assert_eq!(journal.advance(first, 9), Err(Refusal::Stale));
    assert_eq!(journal.checkpoint(kept, 2, 9), Err(Refusal::Stale));
    assert_eq!(journal.pin_offset(kept), None);
}

#[test]
fn an_identity_issued_as_another_kind_is_no_released_pin() {
    // Segment ids, attempts and pin generations share one sequence, so the
    // issuer owns all of them: owning a generation does not make it a pin.
    let mut journal = fresh();
    let sealed = journal.roll(1, 1, at(0)).expect("roll").sealed;
    let attempt = journal.propose(sealed).expect("propose");
    let held = journal.pin_reader(0).expect("pin");
    let before = journal.clone();
    for generation in [sealed.raw(), attempt.attempt()] {
        for slot in 0..Small::PIN_CAPACITY {
            let forged = Pin {
                slot,
                generation,
                ..held
            };
            assert_eq!(journal.release(forged), Err(Refusal::Stale));
            assert_eq!(journal.advance(forged, 9), Err(Refusal::Stale));
            assert_eq!(journal.pin_offset(forged), None);
        }
    }
    assert_eq!(journal, before);
}

#[test]
fn a_pin_this_journal_never_issued_or_holds_elsewhere_is_stale() {
    // A journal of the same scope that issued further: the generation was
    // never issued here.
    let (mut journal, mut ahead) = (fresh(), fresh());
    let mine = journal.pin_reader(1).expect("pin");
    let _ = ahead.pin_reader(1).expect("pin");
    let beyond = ahead.pin_reader(1).expect("pin");
    let before = journal.clone();
    assert_eq!(journal.release(beyond), Err(Refusal::Stale));
    assert_eq!(journal, before);

    // A handle whose generation is held here, in another slot than it names,
    // is not that pin.
    let other = journal.pin_reader(2).expect("pin");
    assert_eq!(journal.release(mine), Ok(()));
    let misplaced = Pin {
        slot: mine.slot,
        ..other
    };
    let before = journal.clone();
    assert_eq!(journal.release(misplaced), Err(Refusal::Stale));
    assert_eq!(journal, before);
    assert_eq!(journal.pin_offset(other), Some(2));
}

#[test]
fn confirming_a_durable_attempt_with_another_payload_is_a_mismatch() {
    let mut journal = fresh();
    let sealed = journal.roll(100, 10, Duration::ZERO).expect("roll").sealed;
    let handle = journal.propose(sealed).expect("propose");
    let rewritten = Durable::Rewritten {
        bytes: 50,
        live_records: 5,
    };
    assert_eq!(
        journal.confirm_durable(handle, rewritten),
        Ok(Confirmed::Recorded)
    );
    let before = journal.clone();
    assert_eq!(
        journal.confirm_durable(handle, Durable::Superseded),
        Err(Refusal::Mismatch),
        "the supersession asked for was never recorded"
    );
    for (bytes, live_records) in [(51, 5), (50, 4), (50, 11)] {
        let other = Durable::Rewritten {
            bytes,
            live_records,
        };
        assert_eq!(
            journal.confirm_durable(handle, other),
            Err(Refusal::Mismatch)
        );
    }
    assert_eq!(
        journal.confirm_durable(handle, rewritten),
        Ok(Confirmed::AlreadyDurable),
        "the identical payload is the provable repeat"
    );
    assert_eq!(journal, before);
    let reclaimed = journal.reclaim(handle).expect("reclaim");
    assert_eq!(reclaimed.replaced_by, Some(handle.attempt()));
    assert_eq!(journal.segment(sealed).expect("kept").bytes, 50);
}

#[test]
fn confirming_a_superseded_attempt_as_rewritten_is_a_mismatch() {
    let mut journal = fresh();
    let sealed = journal.roll(100, 10, Duration::ZERO).expect("roll").sealed;
    let handle = journal.propose(sealed).expect("propose");
    assert_eq!(
        journal.confirm_durable(handle, Durable::Superseded),
        Ok(Confirmed::Recorded)
    );
    let before = journal.clone();
    let rewritten = Durable::Rewritten {
        bytes: 50,
        live_records: 5,
    };
    assert_eq!(
        journal.confirm_durable(handle, rewritten),
        Err(Refusal::Mismatch)
    );
    assert_eq!(
        journal.confirm_durable(handle, Durable::Superseded),
        Ok(Confirmed::AlreadyDurable)
    );
    assert_eq!(journal, before);
    assert_eq!(journal.reclaim(handle).expect("reclaim").replaced_by, None);
    assert_eq!(journal.segment(sealed), None);
}
