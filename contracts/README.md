# Contracts

Implementation target for the Soroban contract crates. Each crate lives in its own directory here and is registered in the workspace `Cargo.toml`.

| Crate | Role | Status |
|---|---|---|
| `common` | Shared error codes, value types, and storage TTL helpers | scaffolded |
| `risk-pool` | Pool creation, tranche deposits, premium accounting, season settlement | pool creation, deposits, and withdrawals implemented; premium and season logic pending |
| `policy` | Policy certificate minting and lifecycle | scaffolded |
| `oracle-adapter` | Publisher registry, ed25519 signed observations, median aggregation | scaffolded |
| `trigger-engine` | Index definitions, deterministic evaluation, severity curves | scaffolded |
| `payout-vault` | Resumable batch payouts to claimable balances, pause | scaffolded |
| `test-utils` | Shared test fixtures, scenario builders, mock publishers | scaffolded |

"scaffolded" means the crate compiles with its storage layout (`DataKey`) and error model (`#[contracterror]`) in place and a constructor, but the business logic is implemented in later sprints. See the repository README and `docs/ARCHITECTURE.md` for the roadmap and the authoritative storage and error tables.
