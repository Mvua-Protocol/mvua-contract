# Contributing to Mvua Contracts

Thank you for helping build open insurance infrastructure. This document is the contract between you and the maintainers: follow it and reviews are fast.

## Code of conduct

By participating you agree to the [Code of Conduct](./CODE_OF_CONDUCT.md).

## Development setup

1. Install [rustup](https://rustup.rs). The pinned toolchain in `rust-toolchain.toml` installs automatically on first cargo command.
2. Install the [Stellar CLI](https://developers.stellar.org/docs/tools/developer-tools/stellar-cli) if you will deploy or invoke contracts.
3. `cargo build --workspace && cargo test --workspace` must pass before you start changing things, so you know your baseline is green.

## How we work

- **Trunk based development.** `main` is always green and deployable. Branch from `main`, open a PR back to `main`.
- **Branch naming:** `feat/<topic>`, `fix/<topic>`, `chore/<topic>`, `docs/<topic>`, `test/<topic>`.
- **Commits are conventional:** `type(scope): imperative subject`. Types: `feat`, `fix`, `chore`, `docs`, `test`, `refactor`, `perf`, `ci`, `build`. Example: `feat(risk-pool): add tranche deposit and receipt tokens`. Subject at most 72 characters.
- **One logical change per PR.** A bug fix PR does not also reformat the codebase.
- **Every PR needs tests.** A fix without the test that would have caught the bug is not done.
- **CI must be green.** No exceptions, including for maintainers.

## Contract standards (the short version)

- Every contract has one `#[contracterror]` enum; variants are documented and codes are never reused.
- No `unwrap`, `expect`, or panicking indexing in production paths. Tests may use them.
- Every mutating function documents its required authorizations.
- Storage keys are documented in `docs/ARCHITECTURE.md`; adding or changing keys updates that document in the same PR.
- Index and payout math is pure and deterministic.
- Public functions, types, and error variants have rustdoc comments.

## PR checklist

- [ ] Conventional commit title and history
- [ ] Tests cover new behavior; `cargo test --workspace` passes
- [ ] `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` pass
- [ ] Docs updated, including `CHANGELOG.md` under `[Unreleased]`
- [ ] Error codes documented if errors changed
- [ ] No new unpinned dependencies (exact versions only, lockfile committed)
- [ ] No secrets, no personal data in the diff

## Reporting issues

- Bugs: use the bug report template. Include the network, commit or tag, and steps.
- Vulnerabilities: **never** in public issues. See [SECURITY.md](./SECURITY.md).
- Ideas: use the feature request template. Explain the problem first, then the solution.

## Review process

1. A maintainer triages and labels new PRs within a few days.
2. Review focuses on correctness, tests, determinism, and conventions.
3. Address comments with new commits; squash merge on approval with a conventional title.
4. First time contributors: start with issues labeled `good first issue`; a maintainer will help you get the environment running.

## Licensing

By contributing you agree your contributions are licensed under the [MIT License](./LICENSE).
