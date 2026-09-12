# Contracts

Implementation target for the Soroban contract crates. Each crate lives in its own directory here and is registered in the workspace `Cargo.toml`.

| Crate | Role | Status |
|---|---|---|
| `risk-pool` | Pool creation, tranche deposits, premium accounting, season settlement | planned |
| `policy` | Policy certificate minting and lifecycle | planned |
| `oracle-adapter` | Publisher registry, ed25519 signed observations, median aggregation | planned |
| `trigger-engine` | Index definitions, deterministic evaluation, severity curves | planned |
| `payout-vault` | Resumable batch payouts to claimable balances, pause | planned |
| `test-utils` | Shared test fixtures, scenario builders, mock publishers | planned |

Status is "planned" until the first scaffold lands; this repository currently ships governance, CI, and the pinned toolchain. See the repository README for the roadmap.
