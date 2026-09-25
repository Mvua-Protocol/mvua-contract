# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `risk-pool` pool creation (`create_pool`): validates configuration, allocates a monotonic pool id, and initializes both tranches, the solvency record, and the treasury to zero (guardian only).
- `risk-pool` tranche deposits (`deposit`): moves the pool token in and mints pro rata receipts, one to one into an empty tranche and pro rata to the existing supply thereafter, with per tranche caps.
- `risk-pool` withdrawals (`withdraw`): burns receipts and releases the underlying, rejected when it would break the solvency invariant `reserves >= committed`, and blocked for the senior tranche while any payout is committed.
- `risk-pool` read helpers (`pool_config`, `tranche`, `receipt`, `solvency`) and unit and property tests covering the deposit, withdrawal, and solvency guard paths.
- `risk-pool` season lifecycle (`open_season`, `activate_season`, `close_season`): a strictly forward state machine `Open` to `Active` to `Closed` to `Settled` with monotonic per pool season ids, guarded transitions, and lifecycle events.
- `risk-pool` premium intake (`collect_premium`): authenticates the payer, accrues capped protocol and publisher fees to the treasury, moves the pool token in, and credits the net premium to the open season and to solvency reserves.
- `risk-pool` season settlement (`settle_season`): guardian only, permitted only on a closed season, applies the settlement waterfall by crediting surplus entirely to the junior tranche and absorbing losses junior first then senior, and returns the realized net.
- `risk-pool` fee configuration (`FeeConfig`) on the pool with validated protocol and publisher basis points and caps, plus `season` and `treasury` read helpers, new error codes `InvalidSeasonState` (110) and `SeasonNotFound` (111), and unit tests covering the lifecycle, fee accrual and caps, and the surplus and loss waterfall paths.
- Contract workspace scaffold (Sprint 1.1): the five contract crates `risk-pool`, `policy`, `oracle-adapter`, `trigger-engine`, and `payout-vault`, each with its `DataKey` storage layout, `#[contracterror]` error enum in its documented code range, and a guardian constructor.
- `common` shared library crate: the shared error code range (900 to 999), basis point math, and storage TTL bump and extend helpers, linked into every contract.
- `test-utils` shared library crate: environment and ledger fixtures, time and sequence advance, and assertion helpers for contract tests.
- Architecture document (`docs/ARCHITECTURE.md`): module map, key flows, storage layout per contract, error model with per contract code ranges, authorization model, and upgrade pattern.
- Threat model (`docs/THREAT-MODEL.md`): assets, trust boundaries, adversary list, and twenty threats with mitigations mapped to requirement IDs.
- Index definition specification (`docs/INDEX-SPEC.md`): rainfall shortfall and consecutive dry days formulas with integer math, severity curve, staleness rules, and golden test vectors.
- Upgrade and governance specification (`docs/UPGRADE-GOVERNANCE.md`): guardian multisig responsibilities, timelock flow, and per contract migration procedures.
- Brand assets in `assets/` (raindrop logo, horizontal lockup, and a system map README banner) and a rewritten README with the project system diagram, the repository's place in the whole project, per crate status, and a summary of the implemented risk-pool lifecycle.

## [0.1.0] - 2026-09-12

### Added

- Repository governance: README, contributing guide, security policy, code of conduct, MIT license.
- CI: format, clippy, build, test, dependency audit, commit lint, and secret scanning workflows.
- Pinned Rust toolchain and workspace dependency pins for the Soroban contract stack.

### Fixed

- CI installs the pinned toolchain via the action's version input instead of a nonexistent tag.
- Secret scanning runs the pinned gitleaks binary directly, which works on organization repositories without a license key.

[Unreleased]: https://github.com/Mvua-Protocol/mvua-contract/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Mvua-Protocol/mvua-contract/releases/tag/v0.1.0
