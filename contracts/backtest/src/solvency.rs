//! Solvency simulation (P2.4.3, FR-DATA-3).
//!
//! A one region pool: every policy shares the same index, so a season's payout
//! hits the whole book at once. All amounts are basis points of total coverage.
//! Each season the pool collects `premium_bps` and pays that season's loss (the
//! payout fraction the backtest measured). It is ruined if reserves go below
//! zero, i.e. it could not pay a claim in full.
//!
//! Synthetic loss years are drawn by resampling the historical seasons with
//! replacement (a bootstrap), from a seeded generator so every report is
//! reproducible.

/// The result of simulating many synthetic futures.
pub struct Summary {
    /// Share of paths that were ruined within the horizon, in percent.
    pub ruin_pct: f64,
    /// Reserves at the horizon (bps of coverage), at the 5th, 50th and 95th
    /// percentile; a ruined path counts with its negative reserves.
    pub p5: i64,
    pub p50: i64,
    pub p95: i64,
}

/// Reserves after each season of `losses`, in order, starting from `capital_bps`.
pub fn reserve_path(losses: &[i64], premium_bps: i64, capital_bps: i64) -> Vec<i64> {
    let mut reserves = capital_bps;
    losses
        .iter()
        .map(|loss| {
            reserves += premium_bps - loss;
            reserves
        })
        .collect()
}

/// Simulate `paths` synthetic futures of `horizon` seasons each.
pub fn simulate(
    losses: &[i64],
    premium_bps: i64,
    capital_bps: i64,
    horizon: usize,
    paths: usize,
    seed: u64,
) -> Summary {
    let mut rng = seed.max(1);
    let mut ruined = 0;
    let mut ends = Vec::with_capacity(paths);
    for _ in 0..paths {
        let mut reserves = capital_bps;
        let mut broke = false;
        for _ in 0..horizon {
            rng = xorshift(rng);
            reserves += premium_bps - losses[(rng % losses.len() as u64) as usize];
            broke |= reserves < 0;
        }
        ruined += broke as usize;
        ends.push(reserves);
    }
    ends.sort_unstable();
    let at = |q: usize| ends[(paths - 1) * q / 100];
    Summary {
        ruin_pct: ruined as f64 * 100.0 / paths as f64,
        p5: at(5),
        p50: at(50),
        p95: at(95),
    }
}

/// The lowest premium (bps of coverage) whose ruin probability is at most
/// `max_ruin_pct`, found by bisection over 0..=10000 with the same seed, so
/// every candidate rate faces the same synthetic years. Ruin can only fall as
/// the premium rises, which makes the bisection valid.
pub fn required_premium_bps(
    losses: &[i64],
    capital_bps: i64,
    horizon: usize,
    paths: usize,
    seed: u64,
    max_ruin_pct: f64,
) -> i64 {
    let (mut lo, mut hi) = (0, 10_000);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if simulate(losses, mid, capital_bps, horizon, paths, seed).ruin_pct <= max_ruin_pct {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

/// xorshift64: a tiny deterministic generator; statistical quality is ample
/// for picking which historical season comes next.
fn xorshift(mut x: u64) -> u64 {
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn no_losses_never_ruin_and_total_loss_always_does() {
        let calm = simulate(&[0, 0, 0], 100, 0, 10, 1_000, 7);
        assert_eq!(calm.ruin_pct, 0.0);
        assert_eq!((calm.p5, calm.p50, calm.p95), (1_000, 1_000, 1_000));

        // A full loss every season with a premium below it eats any capital.
        let doom = simulate(&[10_000], 500, 20_000, 10, 1_000, 7);
        assert_eq!(doom.ruin_pct, 100.0);
    }

    #[test]
    fn reserve_path_tracks_premium_minus_loss() {
        assert_eq!(reserve_path(&[0, 3_000, 0], 1_000, 500), [1_500, -500, 500]);
    }

    #[test]
    fn required_premium_is_deterministic_and_sufficient() {
        // One season in four pays 40% of coverage: a 10% average loss.
        let losses = [0, 0, 0, 4_000];
        let rate = required_premium_bps(&losses, 5_000, 10, 2_000, 42, 1.0);
        assert_eq!(
            rate,
            required_premium_bps(&losses, 5_000, 10, 2_000, 42, 1.0)
        );
        assert!(simulate(&losses, rate, 5_000, 10, 2_000, 42).ruin_pct <= 1.0);
        assert!(simulate(&losses, rate - 1, 5_000, 10, 2_000, 42).ruin_pct > 1.0);
        // Six or more hits in ten seasons has about a 2% chance, above the 1%
        // target, so the pool must survive six: 5000 + 10 * rate >= 6 * 4000,
        // i.e. rate >= 1900, well above the 10% mean loss. A rate of 4000 covers
        // the worst season outright, so it is always enough.
        assert!((1_900..=4_000).contains(&rate), "rate {rate}");
    }
}
