# journal-machines

> **AMS Rosenblum**, NCC-4228, *Rosenblum*-class archive cutter.

Engine-room flight manual: know which log segments exist, and which may go.

Initial, unstable. Allocation-free `no_std` segment metadata policy for a
segmented log: when the active segment should roll, which sealed segments are
worth compacting, and when an original may be deleted.

This is not a log writer, a byte compactor or a Raft log. The journal never
touches a file. The caller writes, seals, compacts, fsyncs and deletes; the
journal holds the bookkeeping and refuses the unsafe order. `Journal<SEGMENTS,
PINS>` has compile-time capacities and refuses rather than grows.

- `roll_decision(bytes, records, now)` reads the caller's observation of the
  active segment and answers `Keep` or `Due(Size | Records | Age)` against the
  `Policy`. An empty segment is never due. `roll(bytes, records, now)` records
  that the caller sealed it and opened the next; it refuses `Empty`,
  `TimeRegression`, `Full` and `Overflow`.
- `observe_live(id, live)` records how many stored records are still live.
  `compaction_candidates(after)` iterates sealed segments whose dead share
  meets `min_dead_per_mille`, oldest first; resume by passing the last id.
- `propose(id)` opens a replacement attempt. `confirm_durable(handle,
  Rewritten { .. } | Superseded)` records the caller's word that the
  replacement is durable. `abort(handle)` gives up and keeps the original.
- `pin_reader(offset)` and `pin_checkpoint(assignment, offset)` retain every
  offset at or after the pin. `advance` and `checkpoint` move a pin forward;
  `release(pin)` is checked against the acquisition generation, so a stale
  handle releases nothing. A checkpoint pin also carries the consumer's
  assignment generation: an older one cannot move it.
- `reclaim(handle)` succeeds only when the replacement is durable **and** no
  pin of either kind sits below the segment's end. Otherwise `NotDurable` or
  `Pinned { offset }`, and nothing changes. A rewritten segment keeps its slot
  under a new revision; a superseded one leaves the journal.

Checkpoint pins compose with `offset-machines`: `pin_checkpoint_from(&tracker)`
and `checkpoint_from(pin, &tracker)` pin at the tracker's confirmed-committed
base under its assignment generation, never at merely acknowledged offsets. An
unassigned tracker is refused as `Unassigned`, an older assignment as
`StaleAssignment`. The Ruby gem offers the same two calls over any object
with `generation` and `base` readers, such as `OffsetMachines::Tracker`.

Durability is the caller's claim; the journal cannot check it. Filesystem
operations, crash recovery and rebuilding this metadata after a restart stay
outside. Persist what you need and replay it into a fresh journal.

```rust
use journal_machines::{Durable, Journal, Policy, ReclaimError, Roll, RollReason};
use core::num::NonZeroU64;

let policy = Policy::new(NonZeroU64::new(1024), None, None, 500).unwrap();
let mut journal = Journal::<8, 4>::new(policy, 0, 0);
assert_eq!(journal.roll_decision(2048, 10, 5), Ok(Roll::Due(RollReason::Size)));
let rolled = journal.roll(2048, 10, 5).unwrap();

let checkpoint = journal.pin_checkpoint(1, 0).unwrap();
let _ = journal.observe_live(rolled.sealed, 2);
assert_eq!(journal.compaction_candidates(None).next().map(|s| s.id), Some(rolled.sealed));

let replacement = journal.propose(rolled.sealed).unwrap();
assert_eq!(journal.reclaim(replacement), Err(ReclaimError::NotDurable));
let _ = journal.confirm_durable(replacement, Durable::Rewritten { bytes: 400, live_records: 2 });
assert_eq!(journal.reclaim(replacement), Err(ReclaimError::Pinned { offset: 0 }));
let _ = journal.checkpoint(checkpoint, 1, 10);
assert_eq!(journal.reclaim(replacement).unwrap().bytes, 2048);
```
