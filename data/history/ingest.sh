#!/usr/bin/env bash
# ingest.sh - fetch the committed daily rainfall history for each backtest region
# (P2.4.1, FR-DATA-1, DR-0036).
#
# Source: ERA5 reanalysis via the Open-Meteo historical archive (keyless, daily
# from 1940, CC BY 4.0). NOAA GHCN-Daily station records were too sparse for
# these regions (see README.md), so stations are not used here.
#
# Writes <REGION>.csv with a `date,rain_mm` header, one row per UTC day. Rerun to
# refresh; the output is deterministic for a closed date range.
#
#   ./ingest.sh                 # all regions, START..END below
set -euo pipefail

START="1991-01-01"
END="2024-12-31"

# region  latitude  longitude  (point the archive resolves to its nearest grid cell)
REGIONS=(
  "KE_NAK -0.3031 36.0800"   # Nakuru, Kenya: bimodal rains, Rift Valley maize
  "NG_KAN 12.0480 8.5250"    # Kano, Nigeria: single Jun to Sep season, Sudan savanna grains
)

cd "$(dirname "${BASH_SOURCE[0]}")"
for entry in "${REGIONS[@]}"; do
  read -r region lat lon <<<"${entry}"
  url="https://archive-api.open-meteo.com/v1/archive?latitude=${lat}&longitude=${lon}&start_date=${START}&end_date=${END}&daily=precipitation_sum&timezone=UTC&format=csv"
  raw="$(curl -fsSL --retry 3 "${url}")"
  # The response opens with a location block (header, values, blank line), then
  # the `time,precipitation_sum (mm)` header and the daily rows.
  echo "${region}: grid cell $(sed -n 2p <<<"${raw}" | cut -d, -f1-3) (lat,lon,elevation)"
  {
    echo "date,rain_mm"
    sed -n '5,$p' <<<"${raw}"
  } >"${region}.csv"
  echo "${region}: $(($(wc -l <"${region}.csv") - 1)) days written to ${region}.csv"
done
