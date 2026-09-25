# Mvua Protocol: Index Definitions

- **Status:** v0 (Sprint 0.2, P0.2.3)
- **Purpose:** define the index formulas the trigger engine evaluates, the data they consume, the staleness rules that gate them, and the severity curve that maps an index breach to a payout fraction. This is the contract facing specification. The full economic methodology and backtests come later (P2.4, P2.5.1); values marked *provisional* are placeholders until those backtests set them.
- **Determinism requirement:** all math here is integer only. Soroban has no floating point. Ratios are expressed in basis points (bps, 1 bps = 1 ten thousandth) and computed with checked integer arithmetic. Rounding direction is specified for every division (FR-TRG-3, NFR-SEC-1).

## 1. Common definitions

- **Region:** an identified geography with its own observation series and index parameters.
- **Metric:** a measured quantity, for example daily rainfall in tenths of a millimeter (integer). Units are fixed per metric and never floating point.
- **Coverage window:** the closed interval of days a policy covers, `[start_day, end_day]`.
- **Observation:** a signed, verified daily value for a region and metric, stored by `oracle-adapter` (see ARCHITECTURE section 5.3).
- **Median series:** for each day, the median of the last N valid observations for that region and metric, as aggregated by `oracle-adapter` (FR-ORC-3). The trigger engine reads this series; it never reads raw single publisher values.
- **Scale constant:** `BPS = 10_000`. A ratio `r` is stored as `round(r * BPS)`.

## 2. Data sources

| Source | Role | Notes |
|---|---|---|
| Open-Meteo API | Primary daily rainfall and temperature for pilot regions | Free tier; consumed by the publisher service, never by contracts |
| NOAA and national meteorological archives | Historical baselines (20 to 30 year medians) and backtests | Baselines are computed off chain and pinned into the index definition |
| Sentinel (ESA) NDVI | Vegetation index for the feasibility spike | Gated by the P2.4.5 go or no go decision |

Baselines (for example the 20 year seasonal median rainfall) are not fetched live. They are computed off chain from historical data, reviewed, and written into the `IndexDef` as fixed parameters. This keeps evaluation deterministic and cheap, and makes every baseline auditable.

## 3. Staleness rules (gate before evaluation)

Before any index is evaluated for a given day, the oracle adapter's staleness check must pass (FR-ORC-4):

1. There must be at least one valid observation within the staleness bound `X` hours of the day being evaluated. Provisional `X = 48` hours.
2. The median for a day requires at least `M` valid observations from distinct publishers. Provisional `M = 2` at pilot, rising as the publisher set grows.
3. If either condition fails, the index for that day is `stale`; the trigger engine returns `StaleIndex` (401) and no trigger can fire. This is fail safe: absence of data never causes a payout and never silently denies one, it blocks and surfaces the gap for operations to resolve.

## 4. Index 1: Rainfall shortfall ratio

Fires when cumulative rainfall over the coverage window falls far enough below the historical baseline for that window.

**Inputs (from `IndexDef` and the median series):**

- `baseline_bps`: the historical seasonal cumulative rainfall for this region and window, in the metric's integer unit. A fixed parameter.
- `actual`: the sum of the median daily rainfall over `[start_day, end_day]`, in the same unit.
- `trigger_ratio_bps`: the shortfall threshold in bps. Provisional `7_500` (fires when actual is below 75 percent of baseline).
- `exhaustion_ratio_bps`: the point of maximum payout. Provisional `4_000` (full payout at or below 40 percent of baseline).

**Computation (integer only):**

```
actual_ratio_bps = actual * BPS / baseline      // integer division, rounds toward zero
```

`actual_ratio_bps` is the realized rainfall as a fraction of baseline, in bps. Rounding toward zero here is conservative for the pool on the trigger test because a lower ratio is closer to triggering; the severity calculation (section 6) specifies its own rounding to favor the pool on payout size.

**Trigger condition:**

```
triggered = actual_ratio_bps < trigger_ratio_bps
```

**Worked example.** Baseline seasonal rainfall 800 mm, window actual 520 mm.

```
actual_ratio_bps = 520 * 10000 / 800 = 6500 bps  (65 percent)
6500 < 7500  ->  triggered
```

At 520 mm the index is triggered; severity is computed in section 6.

## 5. Index 2: Consecutive dry days

Fires when the longest run of consecutive dry days within the window reaches a threshold. A day is dry if its median rainfall is at or below a small dryness floor.

**Inputs:**

- `dry_floor`: daily rainfall at or below which a day counts as dry, in the metric unit. Provisional `10` (1.0 mm expressed in tenths).
- `trigger_days`: run length that fires the index. Provisional `21`.
- `exhaustion_days`: run length at maximum payout. Provisional `35`.

**Computation:**

```
run = 0
max_run = 0
for day in [start_day .. end_day]:
    if median_rain(day) <= dry_floor:
        run = run + 1
        if run > max_run: max_run = run
    else:
        run = 0
triggered = max_run >= trigger_days
```

The loop is bounded by the window length, is order fixed (ascending day), and reads only the median series, so it is deterministic.

**Worked example.** A window with a longest dry run of 26 days, `trigger_days = 21`.

```
max_run = 26,  26 >= 21  ->  triggered
```

## 6. Severity curve and payout

A trigger is not binary in payout: once triggered, severity scales the payout linearly from zero at the trigger point to full at the exhaustion point. This reduces basis risk and cliff effects. The curve maps an index to `severity_bps` in `[0, BPS]`, then the per policy payout is `min(coverage, coverage * severity_bps / BPS)` (FR-TRG-5), with the division rounding down to favor the pool.

**Rainfall shortfall severity:**

```
if actual_ratio_bps >= trigger_ratio_bps:
    severity_bps = 0
else if actual_ratio_bps <= exhaustion_ratio_bps:
    severity_bps = BPS
else:
    severity_bps = (trigger_ratio_bps - actual_ratio_bps) * BPS
                   / (trigger_ratio_bps - exhaustion_ratio_bps)   // rounds down
```

**Consecutive dry days severity:**

```
if max_run < trigger_days:
    severity_bps = 0
else if max_run >= exhaustion_days:
    severity_bps = BPS
else:
    severity_bps = (max_run - trigger_days) * BPS
                   / (exhaustion_days - trigger_days)              // rounds down
```

**Worked example (continuing section 4).** `actual_ratio_bps = 6500`, `trigger = 7500`, `exhaustion = 4000`, coverage 100 USDC (as 100_0000000 stroops equivalent, shown here in USDC for clarity).

```
severity_bps = (7500 - 6500) * 10000 / (7500 - 4000)
             = 1000 * 10000 / 3500
             = 2857 bps            (rounds down from 2857.14)
payout = 100 * 2857 / 10000 = 28.57 -> 28 USDC (rounds down)
```

The curve is monotonic: a worse index never yields a smaller payout. `severity_bps` is always clamped to `[0, BPS]`, so payout never exceeds coverage.

## 7. Index definition object

Stored as `IndexDef(index_id)` in `trigger-engine` (ARCHITECTURE section 5.4). A definition is immutable once policies reference it; changes require a new `index_id` and the timelock, so existing policies keep the terms they were sold under.

| Field | Meaning |
|---|---|
| `index_id` | Stable identifier referenced by policies |
| `kind` | RainfallShortfall or ConsecutiveDryDays (extensible) |
| `region` | Region this definition applies to |
| `metric` | The metric consumed (for example daily rainfall) |
| `window_len` | Coverage window length in days |
| `baseline` | Historical baseline for the window (RainfallShortfall) |
| `trigger_ratio_bps` / `trigger_days` | Trigger threshold for the kind |
| `exhaustion_ratio_bps` / `exhaustion_days` | Maximum payout threshold |
| `dry_floor` | Dryness threshold (ConsecutiveDryDays) |
| `data_source_ref` | Documentation pointer to the methodology entry |

## 8. Golden test vectors

These vectors are the source of truth for unit tests in `trigger-engine` (P1.7.2). Values use the provisional parameters above.

| Case | Index | Inputs | Expected |
|---|---|---|---|
| G1 | RainfallShortfall | baseline 800, actual 800 | ratio 10000, not triggered |
| G2 | RainfallShortfall | baseline 800, actual 600 | ratio 7500, not triggered (boundary, strict less than) |
| G3 | RainfallShortfall | baseline 800, actual 520 | ratio 6500, triggered, severity 2857 bps |
| G4 | RainfallShortfall | baseline 800, actual 320 | ratio 4000, triggered, severity 10000 bps (full) |
| G5 | ConsecutiveDryDays | max run 20 | not triggered |
| G6 | ConsecutiveDryDays | max run 21 | triggered, severity 0 bps (boundary) |
| G7 | ConsecutiveDryDays | max run 28 | triggered, severity 5000 bps |
| G8 | ConsecutiveDryDays | max run 35 | triggered, severity 10000 bps (full) |

Note on G2: the trigger test is strict (`<`), so exactly 75 percent does not trigger. G6 shows a triggered index with zero severity at the trigger boundary, which yields a zero payout; this is intentional and documents the curve's lower edge.

## 9. Open items (resolved by P2.4 backtests)

1. All *provisional* parameters (`X`, `M`, ratios, day thresholds, baselines) are set from historical backtests before any pool uses them (FR-DATA-2, FR-DATA-3).
2. Additional index kinds (consecutive extreme heat days, NDVI drop) are specified when their data feasibility is confirmed (P1.7.5, P2.4.5).
3. Whether severity uses a piecewise or smooth curve is revisited if backtests show the linear curve mis prices tail years; any change is a decision record.
