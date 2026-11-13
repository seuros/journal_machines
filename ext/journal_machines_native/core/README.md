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

Every call that can refuse returns `Result<T, Refusal>`. `T` says only what
was honoured; `Refusal` is the one enum for "this call was not honoured", and
a refused call leaves the journal exactly as it was. An idempotent repeat is
honoured, not refused: when what the caller asked for already holds, the call
answers `Ok` with a variant that says so, and still changes nothing. Only a
repeat the journal can prove is honoured: it must see, in its own state, that
the same call with the same arguments was already applied.
No public enum is `#[non_exhaustive]`: a new variant is a breaking change every
caller matches on.

Time is `core::time::Duration` on a monotonic clock whose epoch the caller
picks: `now`, a segment's `opened_at` and the policy's `max_age`. Offsets,
record and byte counts, ids, revisions and generations are `u64`.

- `Policy::new(max_bytes, max_records, max_age, min_dead_per_mille)` takes
  `Option<NonZeroU64>` sizes and an `Option<Duration>` age; `None` disables a
  threshold. It answers `None` for a per-mille above 1000 or a zero age.
- `Journal::new(policy, base_offset, now)` opens the first segment at `now`.
- `roll_decision(bytes, records, now) -> Result<Roll, Refusal>` reads the
  caller's observation of the active segment and answers `Keep` or
  `Due(Size | Records | Age)` against the `Policy`. An empty segment is never
  due. Refuses `NoActive`, `TimeRegression`, `Overflow`.
- `roll(bytes, records, now) -> Result<Rolled, Refusal>` records that the
  caller sealed the active segment and opened the next. Refuses `NoActive`,
  `Empty`, `TimeRegression`, `Overflow`, `SegmentsFull`, `Exhausted`.
- `observe_live(id, live) -> Result<(), Refusal>` records how many stored
  records are still live. Refuses `Unknown`, `Active`, `Exceeds`.
  `compaction_candidates(after)` iterates sealed segments whose dead share
  meets `min_dead_per_mille`, oldest first; resume by passing the last id.
- `pin_reader(offset)` and `pin_checkpoint(assignment, offset)`
  `-> Result<Pin, Refusal>` retain every offset at or after the pin. Refuse
  `PinsFull`, `Exhausted`, and for a checkpoint `Unassigned`.
- `advance(pin, offset) -> Result<Advanced, Refusal>` moves a reader pin
  forward and answers `Moved`, or `Unchanged` when the pin already sits
  there. Refuses `Stale`, `WrongKind`, `Regressed`.
- `checkpoint(pin, assignment, offset) -> Result<Checkpointed, Refusal>`
  answers `Advanced`, `Reassigned` when a newer assignment generation takes
  over, or `Unchanged` when the same generation reports the offset the pin
  already sits at. Refuses `Stale`, `WrongKind`, `Unassigned`,
  `StaleAssignment`, `Regressed`: an older assignment cannot move the pin.
- `release(pin) -> Result<(), Refusal>` is checked against the acquisition
  generation. Refuses `Stale` for a pin that is not currently held, a second
  release included: the journal keeps no record of the pins it issued, so it
  cannot prove that one it does not hold was ever its own. Whoever holds the
  slot now keeps it.
- `propose(id) -> Result<Replacement, Refusal>` opens a replacement attempt.
  Refuses `Unknown`, `Active`, `Busy`, `Exhausted`.
- `confirm_durable(handle, Rewritten { .. } | Superseded)`
  `-> Result<Confirmed, Refusal>` records the caller's word that the
  replacement is durable and answers `Recorded`, or `AlreadyDurable` when the
  attempt was confirmed before with that same `Durable`. Refuses `Stale`,
  `Exceeds`, and `Mismatch` when the attempt is already durable with a
  different `Durable` (the other variant, or other `bytes` or `live_records`):
  the first confirmation stands.
- `abort(handle) -> Result<(), Refusal>` gives up and keeps the original.
  Refuses `Stale`.
- `reclaim(handle) -> Result<Reclaimed, Refusal>` succeeds only when the
  replacement is durable **and** no pin of either kind sits below the
  segment's end. Refuses `Stale`, `NotDurable`, `Pinned { offset }`. A
  rewritten segment keeps its slot under a new revision; a superseded one
  leaves the journal.

## Contract

`Pin` and `Replacement` handles are only meaningful to the journal that issued
them. They are plain integers (a slot and a generation; a segment id and an
attempt number) with no journal identity, so a handle from another journal
that happens to name a live pin or attempt here is taken for it. Keep each
journal's handles with that journal. A handle that names nothing live is
`Stale`, whoever issued it.

`Stale` always means a handle that is not current, pin or replacement.
`Mismatch` always means a repeat that carries something other than what was
recorded.
`Regressed` stays a refusal: a pin asked to move back is asking for retention
the journal can no longer promise.
`Exhausted` always means a journal-issued identity space is spent: segment
ids, pin generations or attempts. `Overflow` is only the offset range.
`active`, `segment`, `covering`, the iterators, `pin_offset`, `low_pin` and
`pins` read only and never refuse.

Checkpoint pins compose with `offset-machines`: `pin_checkpoint_from(&tracker)`
and `checkpoint_from(pin, &tracker)` pin at the tracker's confirmed-committed
base under its assignment generation, never at merely acknowledged offsets. An
unassigned tracker is refused as `Unassigned`, an older assignment as
`StaleAssignment`; a tracker whose base has not moved is `Unchanged`. The Ruby gem offers the same two calls over any object
with `generation` and `base` readers, such as `OffsetMachines::Tracker`.

Durability is the caller's claim; the journal cannot check it. Filesystem
operations, crash recovery and rebuilding this metadata after a restart stay
outside. Persist what you need and replay it into a fresh journal.

```rust
use core::{num::NonZeroU64, time::Duration};
use journal_machines::{
    Advanced, Checkpointed, Confirmed, Durable, Journal, Policy, Refusal, Roll, RollReason,
};

let hour = Duration::from_secs(3600);
let policy = Policy::new(NonZeroU64::new(1024), None, Some(hour), 500).unwrap();
let mut journal = Journal::<8, 4>::new(policy, 0, Duration::ZERO);
let now = Duration::from_secs(5);
assert_eq!(journal.roll_decision(2048, 10, now), Ok(Roll::Due(RollReason::Size)));
assert_eq!(journal.roll_decision(1, 1, hour), Ok(Roll::Due(RollReason::Age)));
let rolled = journal.roll(2048, 10, now).unwrap();

let checkpoint = journal.pin_checkpoint(1, 0).unwrap();
let reader = journal.pin_reader(0).unwrap();
assert_eq!(journal.observe_live(rolled.sealed, 2), Ok(()));
assert_eq!(journal.compaction_candidates(None).next().map(|s| s.id), Some(rolled.sealed));

let replacement = journal.propose(rolled.sealed).unwrap();
assert_eq!(journal.reclaim(replacement), Err(Refusal::NotDurable));
let rewritten = Durable::Rewritten { bytes: 400, live_records: 2 };
assert_eq!(journal.confirm_durable(replacement, rewritten), Ok(Confirmed::Recorded));
assert_eq!(journal.confirm_durable(replacement, rewritten), Ok(Confirmed::AlreadyDurable));
assert_eq!(journal.confirm_durable(replacement, Durable::Superseded), Err(Refusal::Mismatch));
assert_eq!(journal.reclaim(replacement), Err(Refusal::Pinned { offset: 0 }));

assert_eq!(journal.advance(reader, 10), Ok(Advanced::Moved));
assert_eq!(journal.advance(reader, 10), Ok(Advanced::Unchanged));
assert_eq!(journal.release(reader), Ok(()));
assert_eq!(journal.release(reader), Err(Refusal::Stale));
assert_eq!(journal.checkpoint(checkpoint, 1, 10), Ok(Checkpointed::Advanced));
assert_eq!(journal.checkpoint(checkpoint, 1, 10), Ok(Checkpointed::Unchanged));
assert_eq!(journal.reclaim(replacement).unwrap().bytes, 2048);
```
