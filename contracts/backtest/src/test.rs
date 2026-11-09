extern crate std;

use super::*;
use soroban_sdk::{symbol_short, BytesN};
use trigger_engine::IndexKind;

fn rainfall(env: &Env, window_len: u32, baseline: i128) -> IndexParams {
    IndexParams {
        kind: IndexKind::RainfallShortfall,
        region: symbol_short!("KE_NAK"),
        metric: symbol_short!("rain_mm"),
        window_len,
        baseline,
        trigger_ratio_bps: 7_500,
        exhaustion_ratio_bps: 4_000,
        trigger_days: 0,
        exhaustion_days: 0,
        dry_floor: 0,
        data_source_ref: BytesN::from_array(env, &[0u8; 32]),
    }
}

// Golden vector G3 (INDEX-SPEC section 8) through the backtest path: four days
// of 130 against a baseline of 800 lock severity 2857 and pay 2857000 on one
// coverage unit, while a wet season pays nothing.
#[test]
fn simulate_reproduces_golden_vector_and_a_healthy_season() {
    let seasons = [
        Season {
            year: 2001,
            values: std::vec![130; 4],
        },
        Season {
            year: 2002,
            values: std::vec![250; 4],
        },
    ];
    let out = simulate(&seasons, 10_000_000, |env| rainfall(env, 4, 800));

    assert_eq!(out.len(), 2);
    assert_eq!((out[0].year, out[0].total), (2001, 520));
    assert!(out[0].triggered);
    assert_eq!(out[0].severity_bps, 2857);
    assert_eq!(out[0].payout, 2_857_000);

    assert_eq!((out[1].year, out[1].total), (2002, 1000));
    assert!(!out[1].triggered);
    assert_eq!((out[1].severity_bps, out[1].payout), (0, 0));
}

#[test]
fn load_seasons_cuts_one_window_per_year_and_scales() {
    let csv = "date,rain_mm\n\
               2000-12-31,9.99\n\
               2001-01-01,1.25\n\
               2001-01-02,0.00\n\
               2001-01-03,2.004\n\
               2002-01-01,3.50\n\
               2002-01-02,0.10\n";
    let seasons = load_seasons(csv, "01-01", 2, 100.0).unwrap();
    assert_eq!(seasons.len(), 2);
    assert_eq!(
        (seasons[0].year, seasons[0].values.clone()),
        (2001, std::vec![125, 0])
    );
    assert_eq!(
        (seasons[1].year, seasons[1].values.clone()),
        (2002, std::vec![350, 10])
    );

    // 2001-01-02 has three days of data left; 2002-01-02 runs past the end and
    // is dropped.
    let partial = load_seasons(csv, "01-02", 3, 100.0).unwrap();
    assert_eq!(partial.len(), 1);
    assert_eq!(partial[0].year, 2001);

    assert!(load_seasons("date,rain_mm\n2001-01-01,-1\n", "01-01", 1, 100.0).is_err());
    assert!(load_seasons("date,rain_mm\n2001-01-01\n", "01-01", 1, 100.0).is_err());
}
