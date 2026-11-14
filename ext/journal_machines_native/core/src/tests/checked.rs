//! Validating constructors, and what counts as a resume position.
use std::vec::Vec;

use periscope_machines::scoped;

use super::{ids, journal_with};
use crate::*;

fn id(raw: u64) -> SegmentId {
    SegmentId::from_raw(raw)
}

#[test]
fn a_rolled_is_one_a_journal_records() {
    assert!(Rolled::new(id(1), id(2), 10).is_some());
    assert!(Rolled::new(id(scoped(3, 4)), id(scoped(3, 9)), 1).is_some());
    for (sealed, opened, next) in [
        (0, 2, 10),                       // sequence zero is never issued
        (scoped(3, 0), scoped(3, 2), 10), // nor in another scope
        (2, 2, 10),                       // opened is drawn after sealed
        (3, 2, 10),
        (1, scoped(1, 2), 10), // one scope
        (1, 2, 0),             // at least one sealed record
    ] {
        assert_eq!(
            Rolled::new(id(sealed), id(opened), next),
            None,
            "{sealed} {opened} {next}"
        );
    }
}

#[test]
fn a_reclaimed_is_one_a_journal_answers() {
    assert!(Reclaimed::new(id(1), 0, 100, None).is_some());
    assert!(Reclaimed::new(id(1), 0, 100, Some(5)).is_some());
    assert!(Reclaimed::new(id(1), 5, 30, Some(9)).is_some());
    assert!(Reclaimed::new(id(1), 5, 30, None).is_some());
    for (segment, revision, replaced_by) in [
        (0, 0, None),
        (3, 2, None),            // a revision is drawn after its segment
        (1, scoped(1, 5), None), // in its scope
        (1, 0, Some(1)),         // so is the attempt that replaced it
        (1, 5, Some(5)),         // and it is later than the revision
        (1, 5, Some(4)),
        (1, 0, Some(scoped(2, 5))),
    ] {
        let got = Reclaimed::new(id(segment), revision, 0, replaced_by);
        assert_eq!(got, None, "{segment} {revision} {replaced_by:?}");
    }
}

#[test]
fn a_segment_is_one_a_journal_answers() {
    let (mut journal, sealed) = journal_with(2);
    let replacement = journal.propose(sealed[0]).expect("propose");
    let views: Vec<Segment> = journal.segments(None).collect();
    for view in &views {
        assert_eq!(view.checked(), Some(*view));
    }
    let (active, proposed, plain) = (views[2], views[0], views[1]);
    let bad = [
        Segment {
            records: 1,
            ..active
        },
        Segment { bytes: 1, ..active },
        Segment {
            pinned: true,
            ..active
        },
        Segment {
            revision: 9,
            ..active
        },
        Segment {
            replacement: Some(replacement),
            ..active
        },
        Segment { id: id(0), ..plain },
        Segment {
            records: 0,
            stored_records: 0,
            live_records: 0,
            ..plain
        },
        Segment {
            stored_records: 11,
            ..plain
        },
        Segment {
            live_records: 11,
            ..plain
        },
        Segment {
            base_offset: u64::MAX,
            ..plain
        },
        Segment {
            revision: 1,
            ..plain
        },
        Segment {
            replacement: Some(replacement),
            ..plain
        },
        Segment {
            state: SegmentState::Proposed,
            ..plain
        },
        Segment {
            replacement: None,
            ..proposed
        },
        Segment {
            id: sealed[1],
            ..proposed
        },
        Segment {
            revision: replacement.attempt,
            ..proposed
        },
        Segment {
            state: SegmentState::Durable,
            ..proposed
        },
        Segment {
            state: SegmentState::Reclaimable,
            pinned: true,
            ..proposed
        },
    ];
    for view in bad {
        assert_eq!(view.checked(), None, "{view:?}");
    }
    assert!(
        Segment {
            state: SegmentState::Reclaimable,
            ..proposed
        }
        .checked()
        .is_some()
    );
    assert!(
        Segment {
            state: SegmentState::Durable,
            pinned: true,
            ..proposed
        }
        .checked()
        .is_some()
    );
}

#[test]
fn a_cursor_is_an_identity_this_journal_issued() {
    let (mut journal, sealed) = journal_with(3);
    let all = ids(journal.segments(None));
    assert_eq!(all, [1, 2, 3, 4]);
    // Sequence zero and one not reached were never issued: no position.
    for never in [0, journal.issued() + 1, scoped(1, 2)] {
        assert_eq!(journal.segments(Some(id(never))).count(), 0, "{never}");
        assert_eq!(journal.compaction_candidates(Some(id(never))).count(), 0);
        assert_eq!(journal.reclaimable(Some(id(never))).count(), 0);
    }
    // An issued identity is a position even once its segment has left.
    let replacement = journal.propose(sealed[0]).expect("propose");
    let durable = Durable::Superseded;
    assert_eq!(
        journal.confirm_durable(replacement, durable),
        Ok(Confirmed::Recorded)
    );
    assert!(journal.reclaim(replacement).is_ok());
    assert_eq!(journal.segment(sealed[0]), None);
    assert_eq!(ids(journal.segments(Some(sealed[0]))), [2, 3, 4]);
    // So is an attempt: it is a number of the same sequence.
    assert_eq!(
        ids(journal.segments(Some(id(replacement.attempt())))),
        Vec::<u64>::new()
    );
}
