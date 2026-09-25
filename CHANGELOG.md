# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Architecture document (`docs/ARCHITECTURE.md`): module map, key flows, storage layout per contract, error model with per contract code ranges, authorization model, and upgrade pattern.
- Threat model (`docs/THREAT-MODEL.md`): assets, trust boundaries, adversary list, and twenty threats with mitigations mapped to requirement IDs.
- Index definition specification (`docs/INDEX-SPEC.md`): rainfall shortfall and consecutive dry days formulas with integer math, severity curve, staleness rules, and golden test vectors.
- Upgrade and governance specification (`docs/UPGRADE-GOVERNANCE.md`): guardian multisig responsibilities, timelock flow, and per contract migration procedures.

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
