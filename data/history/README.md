# Historical rainfall for backtests (P2.4.1)

Daily rainfall history for the candidate pilot regions, committed so that
every backtest report is reproducible from the repository alone (FR-DATA-1,
DR-0036). Index simulation (P2.4.2) reads these files. Kenya and Nigeria are
the first two candidates; the two Tanzanian regions extend coverage to a second
Swahili speaking country.

| Region | Place | Season | Requested point | Resolved grid cell (lat, lon, elevation m) | Days |
|---|---|---|---|---|---|
| `KE_NAK` | Nakuru, Kenya | Mar to May (long rains) | -0.3031, 36.0800 | -0.3163, 36.0703, 1805 | 12,419 |
| `NG_KAN` | Kano, Nigeria | Jun to Sep | 12.0480, 8.5250 | 12.0562, 8.4906, 469 | 12,419 |
| `TZ_DOD` | Dodoma, Tanzania (central plateau) | Dec to Apr | -6.1630, 35.7516 | -6.1511, 35.7143, 1114 | 12,419 |
| `TZ_MBE` | Mbeya, Tanzania (southern highlands) | Nov to Apr | -8.9094, 33.4608 | -8.8928, 33.4197, 1708 | 12,419 |

Coverage: 1991-01-01 to 2024-12-31 (34 years), one row per UTC day, no gaps.
A season that crosses New Year (Tanzania) is labelled by its start year, so the
last complete one starts in 2023 (33 seasons).
Format: `date,rain_mm` (`rain_mm` is the daily precipitation sum in millimetres).

## Source

ERA5 reanalysis, served by the [Open-Meteo historical weather API](https://open-meteo.com/en/docs/historical-weather-api)
(`daily=precipitation_sum`). Open-Meteo data is licensed
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/); attribution: weather data by
Open-Meteo.com, generated using Copernicus Climate Change Service information (ERA5).

Regenerate with `./ingest.sh`. The date range is closed, so a rerun is expected
to reproduce the same values unless the upstream archive is revised.

## Why not weather stations

NOAA GHCN-Daily is the publisher's live cross-check source (DR-0030), but its
station records in these regions are far too sparse for a 30 year backtest.
Share of 1991 to 2024 days with a precipitation value (measured 2026-11-09):

| Station | Name | Days with PRCP |
|---|---|---|
| `NIM00065046` | Kano, Mallam Aminu International | 0.8% |
| `KE000063740` | Nairobi, Jomo Kenyatta International | 46.8% |
| `KEM00063686` | Eldoret International | 30.8% |
| `KEM00063741` | Nairobi, Dagoretti | 25.0% |
| Nakuru | (no GHCN-Daily file) | 0% |

## Known limits

- ERA5 is a model reanalysis at roughly 25 km, not a rain gauge. It smooths
  local storms, and in the East African highlands it tends to under-report
  totals (Nakuru's median year here is about 694 mm, against roughly 900 to
  1000 mm in station climatologies). Backtests measure index behaviour on this
  series, and the methodology document (P2.5.1) must state that basis.
- A satellite and gauge blend such as CHIRPS (daily from 1981, about 5 km) is the
  usual reference for African index insurance and is the candidate for a second
  series once the simulation runner exists.
- Mbeya's Nov to Apr mean (about 1,771 mm) is high for the southern highlands,
  and all four of its shortfall seasons fall after 2015. Either may be an ERA5
  artefact at this mountainous cell; check both against a second series before
  calibrating an index there.
