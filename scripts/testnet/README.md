# Testnet deploy and season scaffold

This directory holds the scaffold for running one full Mvua season on live
Stellar testnet: the deferred half of Phase 1 exit criterion 1. The in host
integration test (`contracts/integration-tests`) already proves the five
contracts compose end to end under CI by advancing ledger time; running it for
real on testnet is what validates the two safety timelocks and the premium curve
against real wall clock time and real fees. See DR-0028 for why these are split.

These scripts are authored to be run on the owner shell. The Claude desktop
sandbox has no Rust toolchain and no Stellar CLI, so nothing here has been
executed; every complex typed argument is left as a clearly marked placeholder to
fill in against `--help`.

## Why this cannot be one run

A live season waits out two provisional timelocks that a shared testnet cannot
fast forward:

- the seven day publisher registry timelock (`REGISTRY_TIMELOCK_SECS`, DR-0024)
- the twenty four hour trigger challenge window (`CHALLENGE_WINDOW_SECS`, DR-0025)

So the season is organized as named stages you run in order, waiting out each
timelock between them. Budget over a week of wall clock time end to end.

## Prerequisites

- Stellar CLI installed and on `PATH` (`stellar --version`).
- A funded testnet identity to deploy and admin the contracts. It becomes the
  shared guardian/admin:

  ```bash
  stellar keys generate deployer --network testnet --fund
  ```

- A settlement token contract id on testnet (a test USDC SAC or one you control),
  exported as `MVUA_TOKEN`.
- Two ed25519 publisher keypairs for the oracle. The public keys go into the
  registry; the private keys sign observations off chain (see the signing note
  below).
- An off chain signer that can produce a 64 byte ed25519 signature over the XDR
  of `ObservationPayload { region, metric, timestamp, value }`. The integration
  test builds this with `ed25519-dalek`; any language with ed25519 and Stellar
  XDR encoding works.

## Running

1. Deploy and wire all five contracts (Address only arguments):

   ```bash
   MVUA_TOKEN=<token-contract-id> SOURCE=deployer ./deploy.sh
   ```

   This builds the wasm, deploys risk-pool, policy, oracle-adapter,
   trigger-engine, and payout-vault, performs the three wiring calls
   (`set_policy_contract`, `set_payout_vault`, `set_oracle`), and writes the
   contract ids to `addresses.env`.

2. Run the season stages in order, filling each placeholder and waiting out the
   timelocks where noted:

   ```bash
   ./season.sh propose-publishers   # starts the 7 day registry timelock
   # ... wait 7 days ...
   ./season.sh execute-publishers
   ./season.sh create-pool          # + open_season + activate_season
   ./season.sh fund-reserves        # junior deposit
   ./season.sh create-index
   ./season.sh buy-policy
   ./season.sh report               # sign + submit observations, record days
   ./season.sh evaluate             # expect Triggered on a drought window
   # ... wait 24 hours for the challenge window ...
   ./season.sh finalize
   ./season.sh pay                  # pay_batch + close_batch
   ./season.sh claim
   ```

   Run `./season.sh` with no argument to list the stages.

## Discovering complex argument shapes

Contract functions that take structs (`create_pool` with `PoolConfig`,
`create_index` with `IndexParams`, `policy.mint` with `PolicyTerms`, `pay_batch`
with `Vec<PayoutEntry>`) want JSON. Get the exact shape the CLI expects with
`--help` on the function, for example:

```bash
stellar contract invoke --id "$RISK_POOL_ID" --network testnet \
  --source "$MVUA_SOURCE" -- create_pool --help
```

The golden rainfall index parameters (baseline 800 over a four day window,
trigger 7500 bps, exhaustion 4000 bps) mirror the integration test and
INDEX-SPEC section 8; a drought season at 130 mm/day reproduces golden vector G3
(severity 2857 bps).

## After the run

Record the outcome in the Phase 1 exit review (`context-repo` STRATEGY) under
criterion 1, and reconcile `risk-pool.season(pool_id, season_id).payouts_paid`
and `risk-pool.solvency(pool_id).reserves` the way the integration test does.
