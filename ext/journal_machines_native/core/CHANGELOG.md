# Changelog

## [0.9.0](https://github.com/seuros/journal_machines/compare/journal-machines-v0.8.1...journal-machines-v0.9.0) (2026-11-16)


### Features

* bounded replay window over a caller-stored log ([30c71d3](https://github.com/seuros/journal_machines/commit/30c71d3669ee40d755783422178b2de05144657f))

## [0.8.1](https://github.com/seuros/journal_machines/compare/journal-machines-v0.8.0...journal-machines-v0.8.1) (2026-11-16)


### Bug Fixes

* fix wrapped classes at load so building a handle runs no Ruby ([f87e049](https://github.com/seuros/journal_machines/commit/f87e049d521da4c49f9b160a5247c2a94307aaf5))

## [0.8.0](https://github.com/seuros/journal_machines/compare/journal-machines-v0.7.0...journal-machines-v0.8.0) (2026-11-16)


### ⚠ BREAKING CHANGES

* replace Segment pinned bool with a Retention enum

### Features

* replace Segment pinned bool with a Retention enum ([98d6904](https://github.com/seuros/journal_machines/commit/98d690430cea94988b5ceb7389d7238999896dcd))

## [0.7.0](https://github.com/seuros/journal_machines/compare/journal-machines-v0.6.0...journal-machines-v0.7.0) (2026-11-14)


### ⚠ BREAKING CHANGES

* adopt matryona-machines 0.2: exact-class Pin/Replacement handles, positive thresholds, compiler-checked symbol tables

### Features

* adopt matryona-machines 0.2: exact-class Pin/Replacement handles, positive thresholds, compiler-checked symbol tables ([53ef3b1](https://github.com/seuros/journal_machines/commit/53ef3b10ded3b1b994e2da14f04ecd5dcf5c7a7c))

## [0.6.0](https://github.com/seuros/journal_machines/compare/journal-machines-v0.5.0...journal-machines-v0.6.0) (2026-11-14)


### ⚠ BREAKING CHANGES

* validate values through the core, value handles, native answers, read Policy by exact class, clone-then-keep roll/reclaim

### Features

* validate values through the core, value handles, native answers, read Policy by exact class, clone-then-keep roll/reclaim ([5b00d40](https://github.com/seuros/journal_machines/commit/5b00d406d92bdfb6f1ec331b60c61243ec013994))

## [0.5.0](https://github.com/seuros/journal_machines/compare/journal-machines-v0.4.0...journal-machines-v0.5.0) (2026-11-13)


### ⚠ BREAKING CHANGES

* handles are issued through periscope-machines. Every instance can be built with a scope; a handle from another scope is refused before any lookup; all handle kinds of an instance share one sequence; resumed restores the sequence after a restart. Handles of a journal no longer share raw values; release of a pin that is not held is Stale; the Ruby owner tag is gone.

### Features

* draw segment ids, pin generations and attempts from one scoped periscope sequence; add Journal::resumed ([6a9b959](https://github.com/seuros/journal_machines/commit/6a9b9597aceffa452dee5d74903f99b184a15d2e))

## [0.4.0](https://github.com/seuros/journal_machines/compare/journal-machines-v0.3.0...journal-machines-v0.4.0) (2026-11-13)


### ⚠ BREAKING CHANGES

* see CONVENTIONS: one exhaustive Refusal per crate, Result from every mutator, honoured idempotent repeats only when provable, Duration time, Ruby mirrors the core. No compatibility aliases. Requires offset-machines >= 0.3. A repeated confirm with another payload is Mismatch; releasing a pin that is not held is Stale.

### Features

* one Refusal enum with Result everywhere, provable repeats only, Duration time, Ruby Policy and _from pin calls ([f820899](https://github.com/seuros/journal_machines/commit/f8208990722b8407f206f8714c5708720f72dc74))

## [0.3.0](https://github.com/seuros/journal_machines/compare/journal-machines-v0.2.0...journal-machines-v0.3.0) (2026-11-13)


### Features

* add pin_checkpoint_from and checkpoint_from to the Ruby journal ([a42f1da](https://github.com/seuros/journal_machines/commit/a42f1dace271d39d47264133d63bcd3d1101e441))

## [0.2.0](https://github.com/seuros/journal_machines/compare/journal-machines-v0.1.0...journal-machines-v0.2.0) (2026-11-12)


### Features

* initial extraction ([1e4f7a9](https://github.com/seuros/journal_machines/commit/1e4f7a9c3344bd27293fc3f51ee66e22d266e183))
