//! The replay window evicts what its bounds say, classifies cursors, and
//! restores exactly the windows appends reach.

use std::collections::{BTreeSet, VecDeque};

use matryona_machines::testing::{XorShift64, refuses};

use super::*;

fn budget(bytes: u64) -> Option<NonZeroU64> {
    NonZeroU64::new(bytes)
}

fn appended(window: &mut ReplayWindow<3>, bytes: u64) -> (u64, u64) {
    let answer = window.append(bytes).expect("offsets left");
    assert_eq!(Appended::new(answer.offset(), answer.floor()), Some(answer));
    (answer.offset(), answer.floor())
}

#[test]
fn appends_evict_past_records_and_bytes() {
    let mut window = ReplayWindow::<3>::new(budget(10), 5);
    assert_eq!(
        (window.floor(), window.next_offset(), window.len()),
        (5, 5, 0)
    );
    assert!(window.is_empty());
    assert_eq!(window.coverage(5), Coverage::Covered);
    assert_eq!(window.coverage(4), Coverage::Behind { missed: 1 });
    assert_eq!(window.coverage(6), Coverage::Ahead);

    assert_eq!(appended(&mut window, 2), (5, 5));
    assert_eq!(appended(&mut window, 3), (6, 5));
    assert_eq!(appended(&mut window, 4), (7, 5));
    assert_eq!((window.len(), window.bytes()), (3, 9));
    // A fourth record pushes the oldest out by count.
    assert_eq!(appended(&mut window, 1), (8, 6));
    assert_eq!(window.sizes().collect::<Vec<_>>(), [3, 4, 1]);
    // Six bytes more passes the budget: the two oldest go.
    assert_eq!(appended(&mut window, 6), (9, 8));
    assert_eq!(
        (window.size(8), window.size(9), window.size(7)),
        (Some(1), Some(6), None)
    );
    assert_eq!(window.size(10), None);
    // A record over the budget leaves at once, with everything before it.
    assert_eq!(appended(&mut window, 11), (10, 11));
    assert!(window.is_empty());
    assert_eq!(window.coverage(10), Coverage::Behind { missed: 1 });
    assert_eq!(window.coverage(11), Coverage::Covered);
    assert_eq!(window.coverage(12), Coverage::Ahead);
    assert_eq!(appended(&mut window, 10), (11, 11));
    assert_eq!(window.coverage(3), Coverage::Behind { missed: 8 });
    assert_eq!(window.max_bytes(), budget(10));
    assert_eq!(ReplayWindow::<3>::CAPACITY, 3);

    let mut unbounded = ReplayWindow::<2>::new(None, 0);
    for size in [u64::MAX; 3] {
        assert!(unbounded.append(size).is_ok());
    }
    assert_eq!(unbounded.bytes(), 2 * u128::from(u64::MAX));
    assert_eq!(unbounded.floor(), 1);
}

#[test]
fn the_offset_range_runs_out_with_nothing_changed() {
    let mut window = ReplayWindow::<2>::new(None, u64::MAX - 1);
    assert_eq!(window.append(1).map(|a| a.offset()), Ok(u64::MAX - 1));
    refuses(&mut window, Refusal::Overflow, |w| w.append(1));
    assert_eq!(window.coverage(u64::MAX), Coverage::Covered);
    assert_eq!(Appended::new(u64::MAX, 0), None);
    assert_eq!(Appended::new(4, 6), None);
    assert!(Appended::new(4, 5).is_some());
}

#[test]
fn restored_refuses_windows_no_append_leaves() {
    let ok = |max: Option<NonZeroU64>, base, next, sizes: &[u64]| {
        ReplayWindow::<3>::restored(max, base, next, sizes).is_some()
    };
    assert!(ok(budget(10), 0, 4, &[2, 3, 4]));
    assert!(!ok(budget(10), 0, 4, &[2, 3, 4, 1]));
    assert!(!ok(budget(10), 5, 4, &[]));
    assert!(!ok(budget(10), 3, 4, &[2, 3]));
    assert!(!ok(budget(10), 0, 4, &[5, 6]));
    // Without a budget only a full window evicts.
    assert!(ok(None, 0, 2, &[1, 1]));
    assert!(!ok(None, 0, 4, &[1, 1]));
    assert!(ok(None, 0, 4, &[1, 1, 1]));
    // A budget of u64::MAX evicts a lone record never, so an empty window
    // that evicted is unreachable; one holding bytes is not.
    let all = budget(u64::MAX);
    assert!(!ok(all, 0, 4, &[]));
    assert!(!ok(all, 0, 4, &[0, 0]));
    assert!(ok(all, 0, 4, &[0, 1]));
    assert!(ok(all, 0, 4, &[0, 0, 0]));
}

/// Independent reference: a deque of sizes, evicting oldest first.
struct Model {
    max: Option<u64>,
    records: usize,
    sizes: VecDeque<u64>,
    next: u64,
}

impl Model {
    fn append(&mut self, size: u64) {
        self.sizes.push_back(size);
        self.next += 1;
        while self.sizes.len() > self.records
            || self.max.is_some_and(|max| {
                self.sizes.iter().map(|&s| u128::from(s)).sum::<u128>() > u128::from(max)
            })
        {
            self.sizes.pop_front();
        }
    }

    fn floor(&self) -> u64 {
        self.next - self.sizes.len() as u64
    }
}

#[test]
fn matches_a_naive_model_and_restores_every_state() {
    let mut rng = XorShift64::new(0x0005_EEDE);
    for round in 0..200 {
        let max = match round % 4 {
            0 => None,
            1 => Some(u64::MAX),
            _ => Some(1 + rng.below(64)),
        };
        let base = rng.below(1000);
        let mut window = ReplayWindow::<5>::new(max.and_then(NonZeroU64::new), base);
        let mut model = Model {
            max,
            records: 5,
            sizes: VecDeque::new(),
            next: base,
        };
        for _ in 0..60 {
            let size = match rng.below(8) {
                0 => 0,
                1 => u64::MAX,
                _ => rng.below(40),
            };
            let answer = window.append(size).expect("offsets left");
            model.append(size);
            assert_eq!(answer.floor(), model.floor());
            assert_eq!(
                window.sizes().collect::<Vec<_>>(),
                Vec::from(model.sizes.clone())
            );
            let sizes: Vec<u64> = window.sizes().collect();
            let rebuilt =
                ReplayWindow::<5>::restored(window.max_bytes(), base, window.next_offset(), &sizes);
            assert_eq!(rebuilt.as_ref(), Some(&window), "round {round}");
            let cursor = base + rng.below(window.next_offset() - base + 3);
            let expected = if cursor > model.next {
                Coverage::Ahead
            } else if cursor < model.floor() {
                Coverage::Behind {
                    missed: model.floor() - cursor,
                }
            } else {
                Coverage::Covered
            };
            assert_eq!(window.coverage(cursor), expected);
        }
    }
}

/// Every window `restored` accepts over a small domain is one some sequence
/// of appends reaches, and the reverse.
#[test]
fn restored_accepts_exactly_the_reachable_windows() {
    const DOMAIN: [u64; 5] = [0, 1, 3, 6, u64::MAX];
    const DEPTH: u64 = 5;
    for max in [None, budget(4), budget(u64::MAX)] {
        let mut reached: BTreeSet<(u64, Vec<u64>)> = BTreeSet::new();
        let mut frontier = std::vec![ReplayWindow::<3>::new(max, 0)];
        reached.insert((0, Vec::new()));
        for _ in 0..DEPTH {
            let mut next = Vec::new();
            for window in &frontier {
                for size in DOMAIN {
                    let mut grown = window.clone();
                    assert!(grown.append(size).is_ok());
                    reached.insert((grown.next_offset(), grown.sizes().collect()));
                    next.push(grown);
                }
            }
            frontier = next;
        }
        let mut candidates = std::vec![Vec::new()];
        for _ in 0..3 {
            let longer: Vec<Vec<u64>> = candidates
                .iter()
                .filter(|sizes| sizes.len() == candidates.iter().map(Vec::len).max().unwrap_or(0))
                .flat_map(|sizes| DOMAIN.map(|size| [sizes.as_slice(), &[size]].concat()))
                .collect();
            candidates.extend(longer);
        }
        let mut accepted = 0;
        for next in 0..=DEPTH {
            for sizes in &candidates {
                let restored = ReplayWindow::<3>::restored(max, 0, next, sizes);
                let reachable = reached.contains(&(next, sizes.clone()));
                assert_eq!(restored.is_some(), reachable, "{max:?} {next} {sizes:?}");
                accepted += usize::from(reachable);
            }
        }
        assert!(
            accepted > 20,
            "only {accepted} reachable windows for {max:?}"
        );
    }
}
