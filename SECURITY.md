# Security Policy

Mvua contracts are financial infrastructure for vulnerable users. Security issues are our top priority.

## Reporting a vulnerability

**Do not open a public issue for a security problem.**

Use GitHub's private vulnerability reporting on this repository (Security tab: Report a vulnerability), or contact the maintainers directly through a private channel if you prefer.

Include:

1. Affected contract or crate and commit hash or release tag.
2. Network (testnet or mainnet) and, if deployed, contract ID.
3. Steps or a proof of concept reproducing the issue.
4. Your assessment of impact and severity.

## Our commitment

| Severity | Acknowledgment | Fix target |
|---|---|---|
| Critical (funds at risk) | 48 hours | As fast as safely possible; emergency pause if deployed |
| High | 72 hours | 30 days |
| Medium | 1 week | 60 days |
| Low | 2 weeks | Best effort, next release |

We will credit reporters in the release notes unless you prefer to stay anonymous.

## Scope

In scope: this repository's Soroban contracts, deployment scripts, oracle signature verification, and anything that could lead to loss of funds, wrong payouts, or broken invariants (pool solvency, trigger determinism).

Out of scope: the web application repository (report there), testnet faucet or third party service availability, and theoretical issues without a realistic path to impact.

## Current status

Pre alpha on testnet. Treat deployed testnet contracts as unaudited. An external audit is planned before any mainnet deployment; audit reports will be linked here.

## Safe harbor

We consider good faith security research conducted in line with this policy to be authorized, and we will not pursue action against researchers who respect testnet boundaries and report privately.
