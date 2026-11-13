//! A repeat is honoured only when the journal can prove it: regressions for
//! repeats that were honoured on a handle or a payload the journal never saw.
use core::time::Duration;

use super::{Small, at, policy};
use crate::*;

fn fresh() -> Small {
    Small::new(policy(0, 0, 0, 500), 0, at(0))
}

#[test]
fn releasing_a_pin_another_journal_issued_is_stale() {
    let (mut a, mut b) = (fresh(), fresh());
    let p = a.pin_checkpoint(1, 0).expect("checkpoint");
    let q = b.pin_reader(0).expect("reader");
    assert_eq!(b.release(q), Ok(()));
    let before = b.clone();
    assert_eq!(
        b.release(p),
        Err(Refusal::Stale),
        "an old-enough generation does not prove this journal issued the pin"
    );
    assert_eq!(b, before);
    assert_eq!(a.pin_offset(p), Some(0), "the issuer still holds it");
}

#[test]
fn releasing_a_released_pin_is_stale_and_spares_the_slot() {
    let mut journal = fresh();
    let first = journal.pin_reader(3).expect("pin");
    assert_eq!(journal.release(first), Ok(()));
    let before = journal.clone();
    assert_eq!(journal.release(first), Err(Refusal::Stale));
    assert_eq!(journal, before);
    let second = journal.pin_reader(7).expect("pin");
    let before = journal.clone();
    assert_eq!(journal.release(first), Err(Refusal::Stale));
    assert_eq!(journal, before, "the slot's new holder keeps it");
    assert_eq!(journal.pin_offset(second), Some(7));
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
