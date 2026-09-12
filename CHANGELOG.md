# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Repository governance: README, contributing guide, security policy, code of conduct, MIT license.
- CI: format, clippy, build, test, commit lint, and secret scanning workflows.
- Pinned Rust toolchain and workspace dependency pins for the Soroban contract stack.

### Fixed

- CI installs the pinned toolchain via the action's version input instead of a nonexistent tag.
- Secret scanning runs the pinned gitleaks binary directly, which works on organization repositories without a license key.

[Unreleased]: https://github.com/Mvua-Protocol/mvua-contract/compare/...HEAD
