# Changelog

## [0.2.0](https://github.com/seuros/journal_machines/compare/journal_machines/v0.1.2...journal_machines/v0.2.0) (2026-11-13)


### ⚠ BREAKING CHANGES

* see CONVENTIONS: one exhaustive Refusal per crate, Result from every mutator, honoured idempotent repeats only when provable, Duration time, Ruby mirrors the core. No compatibility aliases. Requires offset-machines >= 0.3. A repeated confirm with another payload is Mismatch; releasing a pin that is not held is Stale.

### Features

* one Refusal enum with Result everywhere, provable repeats only, Duration time, Ruby Policy and _from pin calls ([f820899](https://github.com/seuros/journal_machines/commit/f8208990722b8407f206f8714c5708720f72dc74))

## [0.1.2](https://github.com/seuros/journal_machines/compare/journal_machines/v0.1.1...journal_machines/v0.1.2) (2026-11-13)


### Features

* add pin_checkpoint_from and checkpoint_from to the Ruby journal ([a42f1da](https://github.com/seuros/journal_machines/commit/a42f1dace271d39d47264133d63bcd3d1101e441))

## [0.1.1](https://github.com/seuros/journal_machines/compare/journal_machines-v0.1.0...journal_machines/v0.1.1) (2026-11-12)


### Features

* initial extraction ([1e4f7a9](https://github.com/seuros/journal_machines/commit/1e4f7a9c3344bd27293fc3f51ee66e22d266e183))
