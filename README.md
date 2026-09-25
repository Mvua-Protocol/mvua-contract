# Mvua Protocol Contracts

[![CI](https://github.com/Mvua-Protocol/mvua-contract/actions/workflows/ci.yml/badge.svg)](./.github/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)

**Parametric climate insurance for the people who feed us, built on [Stellar](https://stellar.org) with [Soroban](https://soroban.stellar.org).**

Mvua lets smallholder farmers and climate exposed communities buy micro insurance policies in USDC, and pays out automatically when on chain weather data crosses a predefined index trigger. No claims adjusters, no paperwork: a failed season is a payout in minutes, not months.

> Status: **pre alpha**, in active development. Contracts are not deployed to mainnet. Everything currently targets Stellar testnet.

## Why parametric insurance

Traditional insurance needs loss assessors, forms, and disputes, which makes small policies uneconomical. Parametric insurance pays on an objective, measurable index instead: cumulative rainfall, consecutive dry days, vegetation health. The index is the contract. Farmers get paid when the data says the season failed, and everyone can verify every payout on chain.

## Architecture

```
                      ┌─────────────────────────┐
                      │   Weather data sources   │
                      │  Open-Meteo / NOAA /     │
                      │  satellite (NDVI)        │
                      └───────────┬─────────────┘
                                  │ signed observations (ed25519)
                                  ▼
┌──────────┐  premium   ┌──────────────────┐  median  ┌──────────────────┐
│  Farmer  │──────────►│     Risk Pool     │◄─────────│  Oracle Adapter   │
│ (USDC)   │           │  junior/senior    └──────────┤ publisher registry│
└────┬─────┘           │  tranches, fees     │        └────────┬─────────┘
     │ policy NFT      └─────────┬──────────┘                  │ index series
     ▼                           │ reserves                    ▼
┌──────────┐            ┌───────▼─────────┐          ┌──────────────────┐
│  Policy  │            │   Payout Vault   │◄─────────│  Trigger Engine   │
│ (NFT)    │            │ claimable balance│  payout  │ index definitions │
└──────────┘            │     batches      │  list    │ severity curves   │
                        └──────────────────┘          └──────────────────┘
```

- **Risk pool**: holds premiums and tranche capital in USDC. Junior tranche takes first loss; senior tranche is protected. Solvency is an enforced invariant, not a promise.
- **Policy**: each policy is a non fungible certificate with coverage amount, region, window, and severity curve.
- **Oracle adapter**: independent publishers sign weather observations; the contract verifies ed25519 signatures and aggregates a median. Stale data blocks triggers rather than firing them.
- **Trigger engine**: deterministic index formulas (rainfall shortfall, consecutive dry days, NDVI drop) evaluated against the oracle series.
- **Payout vault**: resumable batch payouts into Stellar claimable balances, so farmers never need XLM for fees.

## Crates

| Crate | Role | Status |
|---|---|---|
| `risk-pool` | Pool creation, tranches, premiums, settlement | planned |
| `policy` | Policy certificates and lifecycle | planned |
| `oracle-adapter` | Publisher registry, signatures, median | planned |
| `trigger-engine` | Index definitions and evaluation | planned |
| `payout-vault` | Batch payouts and pause | planned |
| `test-utils` | Shared test fixtures and scenarios | planned |

Implementation starts in Phase 1 of the roadmap; this repository currently ships governance, CI, the pinned toolchain, and the design documents below.

## Design documents

- [Architecture](docs/ARCHITECTURE.md): module map, storage layout, error model, authorization, and the upgrade pattern.
- [Threat model](docs/THREAT-MODEL.md): assets, trust boundaries, adversaries, and mitigations.
- [Index definitions](docs/INDEX-SPEC.md): the trigger formulas, severity curve, and golden test vectors.
- [Upgrade and governance](docs/UPGRADE-GOVERNANCE.md): guardian multisig, timelock, and migration procedures.

## Getting started

Prerequisites:

- Rust (version pinned in [`rust-toolchain.toml`](./rust-toolchain.toml); rustup installs it automatically)
- [Stellar CLI](https://developers.stellar.org/docs/tools/developer-tools/stellar-cli)

```bash
git clone https://github.com/Mvua-Protocol/mvua-contract
cd mvua-contract

cargo fmt --all --check     # formatting
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace
```

## Testing

| Command | What it runs |
|---|---|
| `cargo test --workspace` | Unit and integration tests for all crates |
| `cargo clippy --workspace --all-targets -- -D warnings` | Lint gate (warnings deny) |
| `cargo fmt --all --check` | Format gate |
| `cargo test --workspace -- --include-ignored testnet` | Testnet integration suite (Phase 1 onward) |

## Deployment (testnet)

Deployment scripts land in Phase 1 (P1.11). The pattern:

```bash
stellar contract deploy \
  --wasm ./target/wasm32v1-none/release/risk_pool.wasm \
  --network testnet \
  --source <deployer-account>
```

Deployed testnet contract IDs will be listed here:

| Contract | ID |
|---|---|
| `risk-pool` | - |
| `policy` | - |
| `oracle-adapter` | - |
| `trigger-engine` | - |
| `payout-vault` | - |

## Roadmap

| Milestone | Status |
|---|---|
| Governance, CI, pinned toolchain | in progress |
| Core contracts, full lifecycle on testnet | planned |
| Publisher service and index backtests | planned |
| Audit preparation and beta season | planned |
| Mainnet v1.0.0 | planned |

## Security

We take security seriously and this is financial infrastructure. Please read [`SECURITY.md`](./SECURITY.md) before disclosing anything. In short: report privately, do not open public issues for vulnerabilities. An external audit is planned before any mainnet deployment.

## Contributing

Contributions are welcome. Read [`CONTRIBUTING.md`](./CONTRIBUTING.md) for the workflow, conventions, and what makes a PR mergeable. Good first issues are labeled `good first issue`.

## License

[MIT](./LICENSE) (c) the Mvua Protocol contributors.
