//! Randomized comparison of the journal against the naive model.
use core::num::NonZeroU64;
use std::vec::Vec;

use super::at;
use super::model::{Model, PINS, Real};
use crate::*;

struct Rng(u64);

impl Rng {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) % bound
    }

    fn index(&mut self, len: usize) -> usize {
        usize::try_from(self.below(len as u64)).expect("index")
    }
}

fn compare(real: &Real, model: &Model, rng: &mut Rng) {
    let want = model.views();
    assert_eq!(real.segments(None).collect::<Vec<_>>(), want);
    assert_eq!(real.len(), want.len());
    assert_eq!(real.active(), want.last().copied());
    assert_eq!(real.pins(), model.pins.len());
    assert_eq!(
        real.low_pin(),
        model.pins.iter().map(|pin| pin.offset).min()
    );
    let after = SegmentId::from_raw(rng.below(model.ids + 2));
    let resumed = |views: Vec<Segment>| -> Vec<Segment> {
        views.into_iter().filter(|view| view.id > after).collect()
    };
    let candidates: Vec<Segment> = want
        .iter()
        .copied()
        .filter(|view| model.candidate(view))
        .collect();
    let reclaimable: Vec<Segment> = want
        .iter()
        .copied()
        .filter(|view| view.state == SegmentState::Reclaimable)
        .collect();
    assert_eq!(
        real.compaction_candidates(None).collect::<Vec<_>>(),
        candidates
    );
    assert_eq!(real.reclaimable(None).collect::<Vec<_>>(), reclaimable);
    assert_eq!(
        real.segments(Some(after)).collect::<Vec<_>>(),
        resumed(want.clone())
    );
    assert_eq!(
        real.compaction_candidates(Some(after)).collect::<Vec<_>>(),
        resumed(candidates)
    );
    assert_eq!(
        real.reclaimable(Some(after)).collect::<Vec<_>>(),
        resumed(reclaimable)
    );
    for view in &want {
        assert_eq!(real.segment(view.id), Some(*view));
    }
    for pin in &model.pins {
        let holds = want
            .iter()
            .find(|view| view.state != SegmentState::Active && pin.offset < view.end_offset());
        assert!(holds.is_none_or(|view| view.pinned));
    }
}

/// A pin another journal issued, held there or already released there. The
/// other journal's generations keep pace with the one under test, so its
/// pins land on slots and generations this one has issued, holds or will
/// issue.
fn stranger(foreign: &mut Real, known: &mut Vec<Pin>, issued: u64, rng: &mut Rng) -> Pin {
    if foreign.pin_generations < issued + 2 {
        if foreign.pins() == PINS {
            let held = known
                .iter()
                .copied()
                .find(|pin| foreign.pin_offset(*pin).is_some())
                .expect("a held pin");
            assert_eq!(foreign.release(held), Ok(()));
        }
        let pin = if rng.below(2) == 0 {
            foreign.pin_reader(rng.below(9))
        } else {
            foreign.pin_checkpoint(1 + rng.below(3), rng.below(9))
        };
        known.push(pin.expect("room"));
        if known.len() > 12 {
            known.remove(0);
        }
    }
    known[rng.index(known.len())]
}

/// What a repeat of an already-confirmed attempt carries: now and then the
/// recorded payload again, otherwise a fresh one that most often differs.
fn payload(model: &mut Model, handle: Replacement, rng: &mut Rng) -> Durable {
    let recorded = model.current(handle).and_then(|segment| segment.durable);
    match (rng.below(4), recorded) {
        (0, _) => Durable::Superseded,
        (1, Some(recorded)) => recorded,
        (..) => Durable::Rewritten {
            bytes: rng.below(40),
            live_records: rng.below(7),
        },
    }
}

#[allow(clippy::too_many_lines)]
fn run(seed: u64, steps: usize) {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let limits = [rng.below(3) * 20, rng.below(3) * 2, rng.below(3) * 4];
    let per_mille = [0, 250, 500, 1000][rng.index(4)];
    let policy = Policy::new(
        NonZeroU64::new(limits[0]),
        NonZeroU64::new(limits[1]),
        Some(at(limits[2])).filter(|age| !age.is_zero()),
        per_mille,
    )
    .expect("policy");
    let base = rng.below(50);
    let mut real = Real::new(policy, base, at(10));
    let mut model = Model {
        limits,
        per_mille: u64::from(per_mille),
        segments: Vec::from([Model::open(1, base, 10)]),
        pins: Vec::new(),
        ids: 1,
        attempts: 0,
        generations: 0,
    };
    let mut clock = 10_u64;
    let mut pins: Vec<Pin> = Vec::new();
    let mut handles: Vec<Replacement> = Vec::new();
    let mut foreign = Real::new(policy, base, at(10));
    let mut strangers: Vec<Pin> = Vec::new();
    for _ in 0..steps {
        let before = real.clone();
        let id = rng.below(model.ids + 2);
        let end = model.segments.last().map_or(0, |segment| segment.base);
        let offset = rng.below(end + 8);
        let now = if rng.below(8) == 0 {
            clock.saturating_sub(rng.below(6))
        } else {
            clock += rng.below(3);
            clock
        };
        let pin = if rng.below(4) == 0 {
            Some(stranger(
                &mut foreign,
                &mut strangers,
                model.generations,
                &mut rng,
            ))
        } else {
            pins.get(rng.index(pins.len() + 1)).copied()
        };
        let handle = handles.get(rng.index(handles.len() + 1)).copied();
        let unchanged = match rng.below(13) {
            0 => {
                let (bytes, records) = (rng.below(60), rng.below(6));
                assert_eq!(
                    real.roll_decision(bytes, records, at(now)),
                    model.roll_decision(bytes, records, now)
                );
                true
            }
            1 | 2 => {
                let (bytes, records) = (rng.below(60), rng.below(6));
                let got = real.roll(bytes, records, at(now));
                assert_eq!(got, model.roll(bytes, records, now));
                got.is_err()
            }
            3 => {
                let live = rng.below(7);
                let got = real.observe_live(SegmentId::from_raw(id), live);
                assert_eq!(got, model.observe_live(id, live));
                got.is_err()
            }
            4 | 5 => {
                let reader = rng.below(2) == 0;
                let assignment = rng.below(4);
                let got = if reader {
                    real.pin_reader(offset)
                } else {
                    real.pin_checkpoint(assignment, offset)
                };
                assert_eq!(
                    got.map(Pin::generation),
                    model.pin(reader, assignment, offset)
                );
                pins.extend(got.ok());
                got.is_err()
            }
            6 => pin.is_none_or(|pin| {
                let got = real.advance(pin, offset);
                let want = model
                    .move_pin(pin, true, 0, offset)
                    .map(|moved| match moved {
                        Checkpointed::Unchanged => Advanced::Unchanged,
                        Checkpointed::Advanced | Checkpointed::Reassigned => Advanced::Moved,
                    });
                assert_eq!(got, want);
                assert_eq!(
                    real.pin_offset(pin),
                    model.held(pin).map(|held| held.offset)
                );
                got != Ok(Advanced::Moved)
            }),
            7 => pin.is_none_or(|pin| {
                let assignment = rng.below(5);
                let got = real.checkpoint(pin, assignment, offset);
                assert_eq!(got, model.move_pin(pin, false, assignment, offset));
                got.is_err() || got == Ok(Checkpointed::Unchanged)
            }),
            8 => pin.is_none_or(|pin| {
                let got = real.release(pin);
                assert_eq!(got, model.release(pin));
                got.is_err()
            }),
            9 => {
                let got = real.propose(SegmentId::from_raw(id));
                assert_eq!(got.map(Replacement::attempt), model.propose(id));
                handles.extend(got.ok());
                got.is_err()
            }
            10 => handle.is_none_or(|handle| {
                let durable = payload(&mut model, handle, &mut rng);
                let got = real.confirm_durable(handle, durable);
                assert_eq!(got, model.confirm(handle, durable));
                got != Ok(Confirmed::Recorded)
            }),
            11 => handle.is_none_or(|handle| {
                let got = real.reclaim(handle);
                assert_eq!(got, model.reclaim(handle));
                got.is_err()
            }),
            _ => handle.is_none_or(|handle| {
                if rng.below(3) != 0 {
                    return true;
                }
                let got = real.abort(handle);
                assert_eq!(got, model.abort(handle));
                got.is_err()
            }),
        };
        if unchanged {
            assert_eq!(
                real, before,
                "a refusal or an idempotent repeat must not change the journal"
            );
        }
        compare(&real, &model, &mut rng);
        if pins.len() > 12 {
            pins.remove(0);
        }
        if handles.len() > 12 {
            handles.remove(0);
        }
    }
}

#[test]
fn random_operations_match_the_naive_model() {
    for seed in 1..=60 {
        run(seed, 2000);
    }
}
