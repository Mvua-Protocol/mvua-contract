# Contracts

Implementation target for the Soroban contract crates. Each crate lives in its own directory here and is registered in the workspace `Cargo.toml`.

| Crate | Role | Status |
|---|---|---|
| `common` | Shared error codes, value types, and storage TTL helpers | scaffolded |
| `risk-pool` | Pool creation, tranche deposits, premium accounting, season settlement | pool creation, deposits, withdrawals, season lifecycle (Open to Active to Closed to Settled), premium intake with capped fees, and settlement waterfall implemented |
| `policy` | Policy certificate minting and lifecycle | scaffolded |
| `oracle-adapter` | Publisher registry, ed25519 signed observations, median aggregation | scaffolded |
| `trigger-engine` | Index definitions, deterministic evaluation, severity curves | scaffolded |
| `payout-vault` | Resumable batch payouts to claimable balances, pause | scaffolded |
| `test-utils` | Shared test fixtures, scenario builders, mock publishers | scaffolded |
| `spec-check` | Host only CI tool: fails when a built contract spec references an undefined type (DR-0032); run via `scripts/check-specs.sh` | implemented |
| `backtest` | Host only tool: replays `data/history` seasons through the real trigger-engine and reports triggers, severities, and payouts (P2.4.2), then a pool solvency simulation over synthetic loss years (P2.4.3); its flags are documented at the top of `src/main.rs` | implemented |

"scaffolded" means the crate compiles with its storage layout (`DataKey`) and error model (`#[contracterror]`) in place and a constructor, but the business logic is implemented in later sprints. See the repository README and `docs/ARCHITECTURE.md` for the roadmap and the authoritative storage and error tables.
