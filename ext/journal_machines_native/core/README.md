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
- `Journal::new(policy, base_offset, now)` opens the first segment at `now`,
  in scope zero. `Journal::with_scope(policy, base_offset, now, scope)` does
  the same under `scope`; `scope()` reads it back. See Scopes below.
- `Journal::resumed(policy, base_offset, now, scope, issued) -> Option<Journal>`
  does the same after a restart, continuing the identity sequence after the
  `issued()` count saved from the journal before it. See Restarts below.
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
  generation. Refuses `Stale` for a pin that is not held: released already,
  or never issued here. Whoever holds the slot now keeps it.
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

### Scopes

Segment ids, `Pin` and `Replacement` handles are only meaningful to the
journal that issued them. Each is built on an integer (a segment id; a slot
and a pin generation; a segment id and an attempt number) drawn from one
sequence the journal counts from one, so two journals issue the same ones,
and a handle given to the wrong journal names a live pin or attempt there. Give each journal its own scope with
`Journal::with_scope`: the scope rides in the top 16 bits of every segment
id, pin generation and attempt number, in the
[`periscope-machines`](https://crates.io/crates/periscope-machines) layout,
and `SegmentId::scope()`, `Pin::scope()` and `Replacement::scope()` read it.
`Journal::new` is scope zero and issues the plain numbers 1, 2, 3.

One sequence serves all three kinds: the active segment takes 1, and every
roll, pin and proposal takes the next number. No two handles of a journal
share a raw value, so a pin generation or an attempt (which is also a
rewritten segment's `revision`) passed to `SegmentId::from_raw` names no
segment. Segment ids still ascend in offset order; they are not consecutive.

An identity this journal never issued, of another scope or beyond what it
has issued, names nothing here, and is refused before anything is looked up,
with nothing changed: a pin or a replacement is
`Stale` (`None` from `pin_offset`), a segment id is `Unknown` (`None` from
`segment`, an empty iterator as a resume position). A `Replacement` is
refused if either its attempt or its segment was never issued here. The
journal reads no clock and no random source, so it cannot pick a scope; two
journals left in the same scope still alias. A scope holds 2^48 - 1
identities, all kinds together. A rewritten segment's `revision` is the
attempt that rewrote it, so it carries the scope too.

### Restarts

A journal started again with `new` or `with_scope` counts from one again, so
a handle that outlived the restart could name a new entry. Either resume the
sequence, `Journal::resumed(policy, base_offset, now, scope, issued)` with
the `issued()` saved from the journal before it, or give each incarnation a
scope of its own. `resumed` restores the sequence and nothing else: segments
and pins are replayed by the caller, and the one active segment takes the
next number. It is `None` when `issued` is more than a scope holds or leaves
no identity for the active segment. Resuming from a count lower than what was
really issued reissues: save `issued()` before handing an identity out.

### Repeats

`Advanced::Unchanged`, `Checkpointed::Unchanged` and
`Confirmed::AlreadyDurable` are answered from a live pin or a live attempt,
which only a handle this journal issued reaches. No repeat is honoured on a
handle that is no longer live. A second `release` is `Stale`: pin
generations share the sequence with segment ids and attempts, so "issued
here and held in no slot" is as true of a segment id as of a released pin,
and the journal keeps no record of which numbers were pins. A second
`reclaim` or `abort` is `Stale` too: an attempt that is no longer current was
aborted, reclaimed as a rewrite or reclaimed as superseded, and the journal
does not keep which.

`Stale` always means a handle that is not current, pin or replacement, or one
this journal never issued.
`Mismatch` always means a repeat that carries something other than what was
recorded.
`Regressed` stays a refusal: a pin asked to move back is asking for retention
the journal can no longer promise.
`Exhausted` always means the journal's identity sequence is spent: 2^48 - 1
segment ids, pin generations and attempts together per scope. `Overflow` is
only the offset range.
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
outside. Persist what you need and replay it into a resumed journal.

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
assert_eq!(journal.advance(reader, 11), Err(Refusal::Stale));

// One sequence for every kind: no two handles share a raw value.
assert_eq!((rolled.sealed.raw(), rolled.opened.raw()), (1, 2));
assert_eq!((checkpoint.generation(), reader.generation()), (3, 4));
assert_eq!((replacement.attempt(), journal.issued()), (5, 5));

// A journal of another scope issues the same sequence numbers under its own
// scope, and neither takes the other's handles.
let mut other = Journal::<8, 4>::with_scope(policy, 0, Duration::ZERO, 7);
let theirs = other.pin_checkpoint(1, 0).unwrap();
assert_eq!((checkpoint.scope(), theirs.scope()), (0, 7));
assert_eq!(journal.release(theirs), Err(Refusal::Stale));
assert_eq!(other.checkpoint(checkpoint, 1, 10), Err(Refusal::Stale));
assert_eq!(journal.checkpoint(checkpoint, 1, 10), Ok(Checkpointed::Advanced));
assert_eq!(journal.checkpoint(checkpoint, 1, 10), Ok(Checkpointed::Unchanged));
assert_eq!(journal.reclaim(replacement).unwrap().bytes, 2048);

// After a restart, continue the sequence from the saved count.
let resumed = Journal::<8, 4>::resumed(policy, 20, now, 0, journal.issued()).unwrap();
assert_eq!(resumed.active().map(|segment| segment.id.raw()), Some(6));
```
