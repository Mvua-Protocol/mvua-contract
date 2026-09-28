#!/usr/bin/env bash
#
# season.sh - drive one rainfall season through the deployed contracts on testnet.
#
# This is the complex typed stage of a live testnet season. Because it waits out
# the real seven day publisher registry timelock (DR-0024) and the twenty four
# hour trigger challenge window (DR-0025), it CANNOT run as one continuous script:
# a single season spans over a week of wall clock time. It is therefore organized
# as named stages you run in order, waiting out each timelock between them. This
# is exactly the wall clock cost the in host integration test avoids by advancing
# ledger time (DR-0028); running it for real is what validates the timelocks and
# the premium curve against a live network.
#
# Prerequisite: run deploy.sh first. It writes addresses.env, which this sources.
#
# Usage:
#   ./season.sh <stage>
# Stages, in order:
#   propose-publishers   deploy.sh already ran; propose the two oracle publishers
#   (wait 7 days for the registry timelock)
#   execute-publishers   activate both publishers
#   create-pool          create a pool and open+activate a season
#   create-index         create the rainfall index
#   buy-policy           quote and mint one farmer policy
#   report               submit signed observations and record days from oracle
#   evaluate             evaluate the window (expects Triggered on a drought)
#   (wait 24 hours for the challenge window)
#   finalize             finalize the window, locking severity
#   pay                  pay the batch and close it
#   claim                the farmer claims the payout
#
# Every stage that needs a complex typed argument (PoolConfig, IndexParams,
# PolicyTerms, ObservationPayload) has a PLACEHOLDER you must fill. Discover the
# exact JSON shape the CLI wants with, e.g.:
#   stellar contract invoke --id "$RISK_POOL_ID" --network testnet \
#     --source "$MVUA_SOURCE" -- create_pool --help

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ADDR="${SCRIPT_DIR}/addresses.env"
if [ ! -f "${ADDR}" ]; then
  echo "error: ${ADDR} not found. Run ./deploy.sh first." >&2
  exit 1
fi
# shellcheck disable=SC1090
source "${ADDR}"

STAGE="${1:-}"
if [ -z "${STAGE}" ]; then
  grep -E '^#   [a-z(]' "${BASH_SOURCE[0]}" | sed 's/^# //'
  exit 1
fi

invoke() {
  local cid="$1"; shift
  local fn="$1"; shift
  stellar contract invoke \
    --id "${cid}" \
    --network "${MVUA_NETWORK}" \
    --source "${MVUA_SOURCE}" \
    -- "${fn}" "$@"
}

# Fill these in as you go. Publisher keys are ed25519 public keys (32 byte hex).
# The two distinct publishers satisfy the oracle median's MIN_PUBLISHERS=2 quorum.
PUBLISHER_A="${PUBLISHER_A:-<PUBLISHER_A_PUBKEY_HEX_32>}"
PUBLISHER_B="${PUBLISHER_B:-<PUBLISHER_B_PUBKEY_HEX_32>}"
# Identifiers captured from earlier stages (create-pool prints pool/season ids,
# create-index prints the index id). Export them or paste them here.
POOL_ID="${POOL_ID:-<POOL_ID>}"
SEASON_ID="${SEASON_ID:-<SEASON_ID>}"
INDEX_ID="${INDEX_ID:-<INDEX_ID>}"
WINDOW_ID="${WINDOW_ID:-0}"
POLICY_ID_MINTED="${POLICY_ID_MINTED:-<POLICY_ID_MINTED>}"
FARMER="${FARMER:-<FARMER_ADDRESS>}"
COVERAGE="${COVERAGE:-10000000}"       # one coverage unit at 7 decimals
MAX_PREMIUM="${MAX_PREMIUM:-1000000}"  # farmer's slippage cap on the quote

case "${STAGE}" in

  propose-publishers)
    # oracle-adapter.propose_publisher(publisher, add): starts the 7 day timelock.
    invoke "${ORACLE_ID}" propose_publisher --publisher "${PUBLISHER_A}" --add true
    invoke "${ORACLE_ID}" propose_publisher --publisher "${PUBLISHER_B}" --add true
    echo "Proposed both publishers. Wait out REGISTRY_TIMELOCK (7 days), then run: execute-publishers"
    ;;

  execute-publishers)
    # oracle-adapter.execute_publisher(publisher): fails with RegistryTimelock if
    # the 7 day eta has not passed yet.
    invoke "${ORACLE_ID}" execute_publisher --publisher "${PUBLISHER_A}"
    invoke "${ORACLE_ID}" execute_publisher --publisher "${PUBLISHER_B}"
    echo "Both publishers active."
    ;;

  create-pool)
    # risk-pool.create_pool(config: PoolConfig) -> pool_id
    # PLACEHOLDER: pass the PoolConfig JSON. Discover its shape with --help.
    # Fields (see contracts/risk-pool): token, regions (Vec<Symbol>),
    # season_length, premium{base_bps,coverage_slope_bps},
    # tranche{junior_cap,senior_cap}, fee{protocol_bps,publisher_bps,
    # protocol_cap,publisher_cap}, transferable. Token must be $MVUA_TOKEN.
    echo "PLACEHOLDER: invoke create_pool with a PoolConfig, then:" >&2
    echo "  invoke risk-pool open_season --pool_id <id>" >&2
    echo "  invoke risk-pool activate_season --pool_id <id> --season_id <sid>" >&2
    echo "Record the printed pool_id and season_id (export POOL_ID / SEASON_ID)." >&2
    # Example skeleton (fill CONFIG_JSON):
    # POOL_ID=$(invoke "${RISK_POOL_ID}" create_pool --config "${CONFIG_JSON}")
    # SEASON_ID=$(invoke "${RISK_POOL_ID}" open_season --pool_id "${POOL_ID}")
    # invoke "${RISK_POOL_ID}" activate_season --pool_id "${POOL_ID}" --season_id "${SEASON_ID}"
    exit 2
    ;;

  fund-reserves)
    # A junior depositor must fund reserves before any payout can be drawn.
    # risk-pool.deposit(pool_id, tier, from, amount). Tier is Junior or Senior.
    # The depositor needs a token balance and authorizes the transfer.
    echo "PLACEHOLDER: mint/transfer token to a depositor, then:" >&2
    echo "  invoke risk-pool deposit --pool_id ${POOL_ID} --tier Junior --from <addr> --amount <reserves>" >&2
    exit 2
    ;;

  create-index)
    # trigger-engine.create_index(params: IndexParams) -> index_id
    # PLACEHOLDER: pass the IndexParams JSON (see contracts/trigger-engine and
    # INDEX-SPEC). For the golden rainfall index: kind RainfallShortfall, region,
    # metric, window_len 4, baseline 800, trigger_ratio_bps 7500,
    # exhaustion_ratio_bps 4000, the dry-day fields 0, data_source_ref (BytesN<32>).
    echo "PLACEHOLDER: invoke create_index with an IndexParams; export INDEX_ID." >&2
    # INDEX_ID=$(invoke "${TRIGGER_ID}" create_index --params "${INDEX_PARAMS_JSON}")
    exit 2
    ;;

  buy-policy)
    # policy.quote(pool_id, coverage) then policy.mint(buyer, pool_id, season_id,
    # terms: PolicyTerms, max_premium, metadata: BytesN<32>).
    # PLACEHOLDER: build PolicyTerms{region, coverage, index_ref=INDEX_ID,
    # window_start, window_end, severity_curve}. metadata is an opaque 32 byte
    # pointer (all zero is fine for a smoke run).
    PREMIUM="$(invoke "${POLICY_ID}" quote --pool_id "${POOL_ID}" --coverage "${COVERAGE}")"
    echo "Quoted premium: ${PREMIUM}. Fund the farmer with at least this, then mint." >&2
    echo "PLACEHOLDER: invoke policy mint with PolicyTerms; export POLICY_ID_MINTED." >&2
    exit 2
    ;;

  report)
    # oracle-adapter.submit(region, metric, timestamp, value, publisher, signature).
    # SIGNING NOTE: the contract authenticates by ed25519_verify over the XDR of
    # ObservationPayload{region, metric, timestamp, value} (NOT require_auth), so
    # each observation must be signed OFF CHAIN with the publisher's private key
    # over exactly that XDR. The 64 byte signature is passed as --signature. A
    # small signing helper (any language with ed25519 + Stellar XDR) is needed to
    # produce it; the in host test builds it with ed25519-dalek. Submit one
    # observation per publisher (>=2 distinct) within the 48h staleness bound, then
    # record each day from the oracle:
    #   trigger-engine.record_day_from_oracle(index_id, window_id, day)
    echo "PLACEHOLDER: sign and submit >=2 observations, then record days 1..window_len:" >&2
    echo "  invoke trigger-engine record_day_from_oracle --index_id ${INDEX_ID} --window_id ${WINDOW_ID} --day <n>" >&2
    exit 2
    ;;

  evaluate)
    # trigger-engine.evaluate(index_id, window_id). Permissionless. On a drought
    # window this returns Triggered(at); on a healthy one, Healthy.
    invoke "${TRIGGER_ID}" evaluate --index_id "${INDEX_ID}" --window_id "${WINDOW_ID}"
    echo "If Triggered, wait out CHALLENGE_WINDOW (24h), then run: finalize"
    ;;

  finalize)
    # trigger-engine.finalize(index_id, window_id). Guardian only; fails with
    # TimelockPending before 24h have passed since the trigger. Locks severity.
    invoke "${TRIGGER_ID}" finalize --index_id "${INDEX_ID}" --window_id "${WINDOW_ID}"
    echo "Finalized. Run: pay"
    ;;

  pay)
    # payout-vault.pay_batch(index_id, window_id, pool_id, season_id, entries).
    # entries is Vec<PayoutEntry{policy_id, owner, coverage}>, guardian asserted.
    # payout_for TRAPS unless the window is Finalized, so this is the on chain gate.
    echo "PLACEHOLDER: build the entries Vec<PayoutEntry>, then:" >&2
    echo "  invoke payout-vault pay_batch --index_id ${INDEX_ID} --window_id ${WINDOW_ID} --pool_id ${POOL_ID} --season_id ${SEASON_ID} --entries '<JSON>'" >&2
    echo "  invoke payout-vault close_batch --index_id ${INDEX_ID} --window_id ${WINDOW_ID}" >&2
    exit 2
    ;;

  claim)
    # payout-vault.claim(owner). Permissionless; pushes the accrued balance and
    # zeroes the ledger. Never blocked by pause.
    invoke "${VAULT_ID}" claim --owner "${FARMER}"
    echo "Claimed. Reconcile: risk-pool.season(pool_id, season_id).payouts_paid and solvency(pool_id).reserves."
    ;;

  *)
    echo "unknown stage: ${STAGE}" >&2
    grep -E '^#   [a-z(]' "${BASH_SOURCE[0]}" | sed 's/^# //' >&2
    exit 1
    ;;
esac
