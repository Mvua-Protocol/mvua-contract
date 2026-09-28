#!/usr/bin/env bash
#
# deploy.sh - build, deploy, and wire the five Mvua contracts on Stellar testnet.
#
# This is the Address-only stage of a live testnet season (Phase 1 exit criterion
# 1, deferred from the in host integration test per DR-0028). It builds every
# contract to wasm, deploys all five, and performs the three wiring calls a
# deployment needs. It does NOT create a pool, index, or policy: those take
# complex typed arguments and live in season.sh.
#
# The sandbox that authored this cannot run it (no Rust toolchain, no Stellar
# CLI). Run it on the owner shell. See README.md for prerequisites.
#
# Usage:
#   ./deploy.sh                  # uses NETWORK=testnet and SOURCE=$MVUA_DEPLOYER
#   NETWORK=testnet SOURCE=alice ./deploy.sh
#
# On success it writes deployed contract ids to ./addresses.env, which season.sh
# sources. Re-running redeploys fresh contracts and overwrites that file.

set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
NETWORK="${NETWORK:-testnet}"
# SOURCE is the Stellar CLI identity (a funded testnet account) that deploys and
# admins every contract. It becomes the shared guardian/admin. Fund it with
# Friendbot first (see README).
SOURCE="${SOURCE:-${MVUA_DEPLOYER:-}}"

if [ -z "${SOURCE}" ]; then
  echo "error: set SOURCE (or MVUA_DEPLOYER) to a funded testnet identity" >&2
  echo "  e.g. stellar keys generate deployer --network testnet --fund" >&2
  echo "       SOURCE=deployer ./deploy.sh" >&2
  exit 1
fi

# The settlement token. On testnet this is typically a test USDC SAC or a SAC you
# control. Set MVUA_TOKEN to its contract id. If unset, deploy.sh stops before
# the payout-vault (which needs a token in its constructor) and tells you.
TOKEN="${MVUA_TOKEN:-}"

# Repo layout: this script sits in mvua-contract/scripts/testnet.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
WASM_DIR="${REPO_ROOT}/target/wasm32v1-none/release"
OUT="${SCRIPT_DIR}/addresses.env"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
say() { printf '\n=== %s ===\n' "$1"; }

deploy_one() {
  # deploy_one <wasm-basename> [-- <ctor args...>]
  local wasm="$1"; shift
  stellar contract deploy \
    --wasm "${WASM_DIR}/${wasm}" \
    --network "${NETWORK}" \
    --source "${SOURCE}" \
    "$@"
}

invoke() {
  # invoke <contract-id> <fn> [--arg val ...]
  local cid="$1"; shift
  local fn="$1"; shift
  stellar contract invoke \
    --id "${cid}" \
    --network "${NETWORK}" \
    --source "${SOURCE}" \
    -- "${fn}" "$@"
}

# ---------------------------------------------------------------------------
# 1. Build
# ---------------------------------------------------------------------------
say "Building contracts (wasm32v1-none, release)"
( cd "${REPO_ROOT}" && stellar contract build )

for w in risk_pool policy oracle_adapter trigger_engine payout_vault; do
  if [ ! -f "${WASM_DIR}/${w}.wasm" ]; then
    echo "error: expected ${WASM_DIR}/${w}.wasm after build" >&2
    exit 1
  fi
done

# ADMIN is the deployer's own address; it becomes the shared guardian/admin that
# every constructor and setter below authorizes against.
ADMIN="$(stellar keys address "${SOURCE}")"
say "Admin / guardian address: ${ADMIN}"

# ---------------------------------------------------------------------------
# 2. Deploy (constructor arg order is verified against each contract's
#    __constructor: see the signatures noted beside each call)
# ---------------------------------------------------------------------------
say "Deploying risk-pool"
# __constructor(env, admin)
RISK_POOL_ID="$(deploy_one risk_pool.wasm -- --admin "${ADMIN}")"
echo "risk-pool: ${RISK_POOL_ID}"

say "Deploying policy"
# __constructor(env, admin, risk_pool)
POLICY_ID="$(deploy_one policy.wasm -- --admin "${ADMIN}" --risk_pool "${RISK_POOL_ID}")"
echo "policy: ${POLICY_ID}"

say "Deploying oracle-adapter"
# __constructor(env, admin)
ORACLE_ID="$(deploy_one oracle_adapter.wasm -- --admin "${ADMIN}")"
echo "oracle-adapter: ${ORACLE_ID}"

say "Deploying trigger-engine"
# __constructor(env, admin)
TRIGGER_ID="$(deploy_one trigger_engine.wasm -- --admin "${ADMIN}")"
echo "trigger-engine: ${TRIGGER_ID}"

if [ -z "${TOKEN}" ]; then
  cat >&2 <<EOF

payout-vault needs a settlement token in its constructor, but MVUA_TOKEN is unset.
Set it to a testnet SAC contract id and re-run, e.g.:
  MVUA_TOKEN=<token-contract-id> SOURCE=${SOURCE} ./deploy.sh

The four contracts above are already deployed; their ids are printed here but
addresses.env is only written once all five succeed.
EOF
  exit 1
fi

say "Deploying payout-vault"
# __constructor(env, admin, pool, policy, trigger, token)
VAULT_ID="$(deploy_one payout_vault.wasm -- \
  --admin "${ADMIN}" \
  --pool "${RISK_POOL_ID}" \
  --policy "${POLICY_ID}" \
  --trigger "${TRIGGER_ID}" \
  --token "${TOKEN}")"
echo "payout-vault: ${VAULT_ID}"

# ---------------------------------------------------------------------------
# 3. Wire (Address-only setters, all guardian gated)
# ---------------------------------------------------------------------------
say "Wiring risk-pool -> policy and payout-vault; trigger-engine -> oracle"
# risk-pool.set_policy_contract(policy)
invoke "${RISK_POOL_ID}" set_policy_contract --policy "${POLICY_ID}"
# risk-pool.set_payout_vault(vault)
invoke "${RISK_POOL_ID}" set_payout_vault --vault "${VAULT_ID}"
# trigger-engine.set_oracle(oracle)
invoke "${TRIGGER_ID}" set_oracle --oracle "${ORACLE_ID}"

# ---------------------------------------------------------------------------
# 4. Record addresses for season.sh
# ---------------------------------------------------------------------------
cat > "${OUT}" <<EOF
# Written by deploy.sh on $(date -u +%Y-%m-%dT%H:%M:%SZ). Sourced by season.sh.
export MVUA_NETWORK="${NETWORK}"
export MVUA_SOURCE="${SOURCE}"
export MVUA_ADMIN="${ADMIN}"
export MVUA_TOKEN="${TOKEN}"
export RISK_POOL_ID="${RISK_POOL_ID}"
export POLICY_ID="${POLICY_ID}"
export ORACLE_ID="${ORACLE_ID}"
export TRIGGER_ID="${TRIGGER_ID}"
export VAULT_ID="${VAULT_ID}"
EOF

say "Deployed and wired. Addresses written to ${OUT}"
echo "Next: run the staged season with season.sh (see README.md)."
