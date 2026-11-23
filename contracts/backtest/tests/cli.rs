//! The `backtest` CLI's report formats (P2.4.4). A tiny fixture (four 10 day
//! seasons, one of them dry) keeps the replay fast in a debug build.

use std::process::Command;

fn run(extra: &[&str]) -> (bool, String) {
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/TINY.csv");
    let out = Command::new(env!("CARGO_BIN_EXE_backtest"))
        .args(["--data", data, "--start", "01-01", "--days", "10"])
        .args([
            "--trigger-bps",
            "7500",
            "--exhaustion-bps",
            "4000",
            "--paths",
            "50",
        ])
        .args(extra)
        .output()
        .expect("run backtest");
    let text = String::from_utf8_lossy(if out.status.success() {
        &out.stdout
    } else {
        &out.stderr
    })
    .into_owned();
    (out.status.success(), text)
}

#[test]
fn markdown_is_the_default() {
    let (ok, text) = run(&[]);
    assert!(ok);
    assert!(text.starts_with("# Backtest: TINY"));
    assert!(text.contains("Triggered: 1 (25.0%) in 2021."));
    assert!(text.contains("## Solvency"));
}

#[test]
fn csv_has_a_header_and_one_row_per_season() {
    let (ok, text) = run(&["--format", "csv"]);
    assert!(ok);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "year,season_mm,pct_of_baseline,triggered,severity_bps,payout"
    );
    assert_eq!(lines.len(), 1 + 4);
    assert_eq!(lines[2], "2021,20.0,25.81,true,10000,10000000");
}

#[test]
fn json_is_one_object_with_summary_and_solvency() {
    let (ok, text) = run(&["--format", "json"]);
    assert!(ok);
    let line = text.trim();
    assert!(line.starts_with('{') && line.ends_with('}'));
    assert_eq!(text.lines().count(), 1);
    assert!(line.contains(r#""region":"TINY""#));
    assert!(line.contains(r#""trigger_years":[2021]"#));
    assert!(line.contains(r#""burning_cost_bps":2500"#));
    assert!(line.contains(r#""solvency":{"premium_bps":110"#));
}

#[test]
fn an_unknown_format_is_rejected() {
    let (ok, text) = run(&["--format", "xml"]);
    assert!(!ok);
    assert!(text.contains("--format: expected md, csv or json"));
}
