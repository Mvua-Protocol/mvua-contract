//! `backtest`: run an index definition over committed rainfall history and
//! print a Markdown report of triggers, severities, and payouts (P2.4.2).
//!
//! ```text
//! cargo run -p backtest -- --data data/history/NG_KAN.csv --start 06-01 --days 122
//! ```
//!
//! Flags (values in the contract's scaled units, `--scale` per millimetre):
//!   --data PATH            `date,rain_mm` CSV; its file name is the region
//!   --start MM-DD          first day of the season window (required)
//!   --days N               window length in days (required)
//!   --kind rainfall|dry-days               default rainfall
//!   --baseline N           rainfall: default is the mean season total
//!   --trigger-bps N        rainfall: default 7500
//!   --exhaustion-bps N     rainfall: default 4000
//!   --trigger-days N       dry-days: longest dry run that triggers
//!   --exhaustion-days N    dry-days: run that pays in full
//!   --dry-floor N          dry-days: a day at or below this is dry, default 100 (1 mm)
//!   --coverage N           policy size for payouts, default 10000000 (one unit)
//!   --scale N              units per millimetre, default 100 (the publisher's)

use std::{collections::HashMap, env, fs, path::Path, process};

use backtest::{load_seasons, simulate};
use soroban_sdk::{BytesN, Env, Symbol};
use trigger_engine::{IndexKind, IndexParams};

fn main() {
    if let Err(e) = run() {
        eprintln!("backtest: {e}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut flags = HashMap::new();
    for pair in args.chunks(2) {
        match pair {
            [k, v] if k.starts_with("--") => flags.insert(k[2..].to_string(), v.clone()),
            _ => return Err(format!("expected --flag value pairs, got {pair:?}")),
        };
    }
    let get = |k: &str| flags.get(k).map(String::as_str);
    let num = |k: &str, default: Option<i128>| -> Result<i128, String> {
        match get(k) {
            Some(v) => v.parse().map_err(|_| format!("--{k}: not a number: {v}")),
            None => default.ok_or(format!("--{k} is required")),
        }
    };

    let path = get("data").ok_or("--data is required")?;
    let start = get("start").ok_or("--start is required")?;
    let days = num("days", None)? as usize;
    let scale = num("scale", Some(100))?;
    let coverage = num("coverage", Some(10_000_000))?;
    let region = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("--data needs a file name")?;

    let csv = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let seasons = load_seasons(&csv, start, days, scale as f64)?;
    if seasons.is_empty() {
        return Err(format!(
            "no complete {days} day season starting {start} in {path}"
        ));
    }
    let totals: Vec<i128> = seasons.iter().map(|s| s.values.iter().sum()).collect();
    let mean_total = totals.iter().sum::<i128>() / totals.len() as i128;

    let env = Env::default();
    let rainfall = get("kind").unwrap_or("rainfall") == "rainfall";
    let params = IndexParams {
        kind: if rainfall {
            IndexKind::RainfallShortfall
        } else {
            IndexKind::ConsecutiveDryDays
        },
        region: Symbol::new(&env, region),
        metric: Symbol::new(&env, "rain_mm"),
        window_len: days as u32,
        baseline: if rainfall {
            num("baseline", Some(mean_total))?
        } else {
            0
        },
        trigger_ratio_bps: if rainfall {
            num("trigger-bps", Some(7_500))? as u32
        } else {
            0
        },
        exhaustion_ratio_bps: if rainfall {
            num("exhaustion-bps", Some(4_000))? as u32
        } else {
            0
        },
        trigger_days: if rainfall {
            0
        } else {
            num("trigger-days", None)? as u32
        },
        exhaustion_days: if rainfall {
            0
        } else {
            num("exhaustion-days", None)? as u32
        },
        dry_floor: if rainfall {
            0
        } else {
            num("dry-floor", Some(100))?
        },
        data_source_ref: BytesN::from_array(&env, &[0u8; 32]),
    };

    // Soroban values belong to the Env that made them, so rebuild the object
    // fields inside the simulation host; the numeric fields carry over.
    let outcomes = simulate(&seasons, coverage, |host| IndexParams {
        region: Symbol::new(host, region),
        metric: Symbol::new(host, "rain_mm"),
        data_source_ref: BytesN::from_array(host, &[0u8; 32]),
        ..params.clone()
    });
    let mm = |v: i128| format!("{:.1}", v as f64 / scale as f64);

    println!("# Backtest: {region}, {start} for {days} days\n");
    if rainfall {
        println!(
            "Rainfall shortfall. Baseline {} mm ({}), trigger below {:.1}%, full payout at or below {:.1}%. Coverage {coverage}.\n",
            mm(params.baseline),
            if get("baseline").is_some() { "given" } else { "mean season total" },
            params.trigger_ratio_bps as f64 / 100.0,
            params.exhaustion_ratio_bps as f64 / 100.0,
        );
        println!("| Year | Season mm | % of baseline | Severity bps | Payout |");
        println!("|---|---|---|---|---|");
        for o in &outcomes {
            println!(
                "| {} | {} | {:.1}% | {} | {} |",
                o.year,
                mm(o.total),
                o.total as f64 * 100.0 / params.baseline as f64,
                o.severity_bps,
                o.payout
            );
        }
    } else {
        println!(
            "Consecutive dry days (a day at or below {} mm). Triggers at a {} day run, pays in full at {}. Coverage {coverage}.\n",
            mm(params.dry_floor),
            params.trigger_days,
            params.exhaustion_days,
        );
        println!("| Year | Season mm | Severity bps | Payout |");
        println!("|---|---|---|---|");
        for o in &outcomes {
            println!(
                "| {} | {} | {} | {} |",
                o.year,
                mm(o.total),
                o.severity_bps,
                o.payout
            );
        }
    }

    let hits: Vec<String> = outcomes
        .iter()
        .filter(|o| o.triggered)
        .map(|o| o.year.to_string())
        .collect();
    let n = outcomes.len();
    let paid: i128 = outcomes.iter().map(|o| o.payout).sum();
    let mean_payout = paid / n as i128;
    println!(
        "\nSeasons: {n}. Triggered: {} ({:.1}%){}.",
        hits.len(),
        hits.len() as f64 * 100.0 / n as f64,
        if hits.is_empty() {
            String::new()
        } else {
            format!(" in {}", hits.join(", "))
        }
    );
    println!(
        "Mean payout per season: {mean_payout} ({:.2}% of coverage, the burning cost before loading).",
        mean_payout as f64 * 100.0 / coverage as f64
    );
    if let Some(worst) = outcomes
        .iter()
        .max_by_key(|o| o.payout)
        .filter(|o| o.payout > 0)
    {
        println!(
            "Largest payout: {} in {} (severity {} bps).",
            worst.payout, worst.year, worst.severity_bps
        );
    }
    Ok(())
}
