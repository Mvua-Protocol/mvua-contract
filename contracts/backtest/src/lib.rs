//! Index backtest runner (P2.4.2, FR-DATA-2).
//!
//! Replays historical daily rainfall through the real `trigger-engine` contract
//! on a local Soroban test host: one index definition, one window per season,
//! each day folded with `record_day`, then `evaluate`, and for a triggered season
//! `finalize` after the challenge window and `payout_for`. Every trigger,
//! severity, and payout is therefore produced by the code deployed on chain, so
//! a backtest cannot drift from the contract the way a reimplementation could.

use soroban_sdk::{testutils::Address as _, Address, Env};
use test_utils::{advance_time, new_env};
use trigger_engine::{IndexParams, TriggerEngine, TriggerEngineClient, TriggerStatus};

/// The trigger-engine challenge window, mirrored so `finalize` is reachable.
const CHALLENGE_SECS: u64 = 24 * 60 * 60;

/// One season's daily values, already scaled to the contract's integer units.
pub struct Season {
    pub year: i32,
    pub values: Vec<i128>,
}

/// What the contract decided for one season.
pub struct Outcome {
    pub year: i32,
    /// Sum of the season's scaled daily values.
    pub total: i128,
    pub triggered: bool,
    /// Locked severity in basis points (0 when not triggered).
    pub severity_bps: u32,
    /// `payout_for(coverage)`: the amount one policy of `coverage` would receive.
    pub payout: i128,
}

/// Parse a `date,rain_mm` CSV (as in `data/history/`) and cut one season per
/// year: `days` consecutive days starting on `start` (`MM-DD`). Each millimetre
/// value is scaled by `scale` and rounded, the same conversion the publisher
/// applies before signing. Seasons that run past the end of the data are dropped.
pub fn load_seasons(
    csv: &str,
    start: &str,
    days: usize,
    scale: f64,
) -> Result<Vec<Season>, String> {
    let mut rows: Vec<(&str, i128)> = Vec::new();
    for (n, line) in csv.lines().skip(1).enumerate() {
        let (date, mm) = line
            .split_once(',')
            .ok_or_else(|| format!("line {}: expected date,rain_mm", n + 2))?;
        let mm: f64 = mm
            .trim()
            .parse()
            .map_err(|_| format!("line {}: bad rain_mm {mm:?}", n + 2))?;
        if !mm.is_finite() || mm < 0.0 {
            return Err(format!("line {}: invalid rainfall {mm}", n + 2));
        }
        rows.push((date, (mm * scale).round() as i128));
    }
    let mut seasons = Vec::new();
    for (i, (date, _)) in rows.iter().enumerate() {
        if date.get(5..) == Some(start) && i + days <= rows.len() {
            let year = date[..4]
                .parse()
                .map_err(|_| format!("bad date {date:?}"))?;
            let values = rows[i..i + days].iter().map(|(_, v)| *v).collect();
            seasons.push(Season { year, values });
        }
    }
    Ok(seasons)
}

/// Run the index `params` builds over every season through the real contract
/// and return one outcome per season, in input order. `params` is given the
/// test host's `Env`, because Soroban values (`Symbol`, `BytesN`) belong to the
/// environment that created them. Its `window_len` must equal each season's length.
pub fn simulate(
    seasons: &[Season],
    coverage: i128,
    params: impl FnOnce(&Env) -> IndexParams,
) -> Vec<Outcome> {
    let env: Env = new_env();
    env.cost_estimate().budget().reset_unlimited();
    let admin = Address::generate(&env);
    let id = env.register(TriggerEngine, (admin,));
    let te = TriggerEngineClient::new(&env, &id);
    let index_id = te.create_index(&params(&env));

    seasons
        .iter()
        .map(|season| {
            // One window per season, as a deployed index serves many seasons.
            let window_id = season.year as u64;
            for (day, value) in season.values.iter().enumerate() {
                te.record_day(&index_id, &window_id, &(day as u64 + 1), value);
            }
            let total = season.values.iter().sum();
            match te.evaluate(&index_id, &window_id) {
                TriggerStatus::Triggered(_) => {
                    advance_time(&env, CHALLENGE_SECS + 1);
                    let severity_bps = match te.finalize(&index_id, &window_id) {
                        TriggerStatus::Finalized(_, severity) => severity,
                        other => panic!("finalize returned {other:?}"),
                    };
                    let payout = te.payout_for(&index_id, &window_id, &coverage);
                    Outcome {
                        year: season.year,
                        total,
                        triggered: true,
                        severity_bps,
                        payout,
                    }
                }
                _ => Outcome {
                    year: season.year,
                    total,
                    triggered: false,
                    severity_bps: 0,
                    payout: 0,
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod test;
