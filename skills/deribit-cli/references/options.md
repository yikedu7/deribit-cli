# Agent Combined Option Queries

This file provides adaptable Bash + jq examples. These examples do not install a separate analysis program or add Rust CLI commands. The Agent selects inputs, schedules requests, checks results, and interprets data. If `jq` is unavailable, use existing tools to implement the same workflow.

## First Determine the Instrument Set

Use `get-instruments` to obtain metadata; the API `currency` is a query group and may not equal the underlying named by the user. When uncertain, use `--currency any --kind option --expired false`, then inspect `base_currency`, `quote_currency`, `settlement_currency`, and `instrument_name`. When the same underlying has multiple products, explicitly choose the product scope and do not silently mix settlement currencies.

After saving the successful response as `instruments.json`, first list the expiration times in the metadata (the example below reads only the file and makes no API request):

```sh
jq '[.result[] | {base_currency, quote_currency, settlement_currency, expiration_timestamp, expiration_utc: (.expiration_timestamp / 1000 | todateiso8601)}] | unique' instruments.json
```

Confirm which `expiration_timestamp` corresponds to the user's specified date; resolve ambiguity first when there are multiple matches, and do not round to the nearest day. Collection parameters: `DERIBIT_CURRENCY` (an API group permitted by help, possibly `any`), `DERIBIT_BASE`, `DERIBIT_QUOTE`, `DERIBIT_SETTLEMENT`, `DERIBIT_EXPIRY_MS` (exact milliseconds), and `DERIBIT_SIDE` (`call`/`put`/`both`, default `both`).

## Batch Collection and Option-Chain Aggregation

Run the following snippet in a separate Bash process after setting the parameters above and `DERIBIT_BIN`. The default is mainnet; set `DERIBIT_ENV=testnet` to switch explicitly. Call ticker once per candidate; the request count is one metadata request plus N ticker requests, so estimate N from the filter scope first. Narrowing the scope must match the user's intent; do not arbitrarily truncate candidates to save requests.

```bash
set -euo pipefail
: "${DERIBIT_BIN:?set absolute CLI path}"
: "${DERIBIT_CURRENCY:?set API currency group}"
: "${DERIBIT_BASE:?set underlying base currency}"
: "${DERIBIT_QUOTE:?set quote currency}"
: "${DERIBIT_SETTLEMENT:?set settlement currency}"
: "${DERIBIT_EXPIRY_MS:?set exact expiration timestamp in milliseconds}"
option_side=${DERIBIT_SIDE:-both}
option_env=${DERIBIT_ENV:-mainnet}
case "$option_side" in call|put|both) ;; *) printf 'invalid option side\n' >&2; exit 2 ;; esac
case "$option_env" in mainnet|testnet) ;; *) printf 'invalid environment\n' >&2; exit 2 ;; esac
jq -en --argjson expiry "$DERIBIT_EXPIRY_MS" '$expiry | type == "number" and . > 0 and floor == .' >/dev/null
batch_dir=$(mktemp -d)
printf 'batch_dir=%s\n' "$batch_dir"
batch_started=$(date -u +%Y-%m-%dT%H:%M:%SZ)
jq -n --arg started "$batch_started" --arg env "$option_env" --arg currency "$DERIBIT_CURRENCY" --arg base "$DERIBIT_BASE" --arg quote "$DERIBIT_QUOTE" --arg settlement "$DERIBIT_SETTLEMENT" --arg side "$option_side" --argjson expiry "$DERIBIT_EXPIRY_MS" '{environment:$env, started_at:$started, simultaneous:false, status:"collecting", filters:{currency:$currency,base_currency:$base,quote_currency:$quote,settlement_currency:$settlement,expiration_timestamp:$expiry,option_type:$side}}' > "$batch_dir/batch.json"
"$DERIBIT_BIN" version > "$batch_dir/version.json"
if "$DERIBIT_BIN" --env "$option_env" --output json public get-instruments --currency "$DERIBIT_CURRENCY" --kind option --expired false > "$batch_dir/instruments.json" 2> "$batch_dir/instruments.stderr"; then
  jq -e 'has("result") and (has("error") | not) and (.result | type == "array")' "$batch_dir/instruments.json" >/dev/null
else
  batch_rc=$?
  printf 'metadata failed: exit=%s; partial evidence=%s\n' "$batch_rc" "$batch_dir" >&2
  exit "$batch_rc"
fi
jq --arg base "$DERIBIT_BASE" --arg quote "$DERIBIT_QUOTE" --arg settlement "$DERIBIT_SETTLEMENT" --arg side "$option_side" --argjson expiry "$DERIBIT_EXPIRY_MS" '[.result[] | select(.kind == "option" and .is_active == true and .base_currency == $base and .quote_currency == $quote and .settlement_currency == $settlement and .expiration_timestamp == $expiry) | select($side == "both" or .option_type == $side)] | sort_by(.strike, .option_type, .instrument_name)' "$batch_dir/instruments.json" > "$batch_dir/candidates.json"
jq -r '.[].instrument_name' "$batch_dir/candidates.json" > "$batch_dir/names.txt"
printf 'ticker requests planned: %s\n' "$(jq 'length' "$batch_dir/candidates.json")"
mkdir "$batch_dir/tickers"
batch_i=0
while IFS= read -r instrument; do
  batch_i=$((batch_i + 1))
  if "$DERIBIT_BIN" --env "$option_env" --output json public ticker --instrument-name "$instrument" > "$batch_dir/tickers/$batch_i.json" 2> "$batch_dir/tickers/$batch_i.stderr"; then
    jq -e --arg instrument "$instrument" '(has("error") | not) and (.result | type == "object") and (.result.instrument_name == $instrument) and (.result.timestamp | type == "number")' "$batch_dir/tickers/$batch_i.json" >/dev/null
  else
    batch_rc=$?
    printf 'ticker failed: instrument=%s exit=%s; incomplete batch=%s\n' "$instrument" "$batch_rc" "$batch_dir" >&2
    exit "$batch_rc"
  fi
done < "$batch_dir/names.txt"
if [ "$batch_i" -eq 0 ]; then
  jq -n '[]' > "$batch_dir/tickers.json"
else
  jq -s '[.[].result]' "$batch_dir"/tickers/*.json > "$batch_dir/tickers.json"
fi
jq -n --slurpfile instruments "$batch_dir/candidates.json" --slurpfile tickers "$batch_dir/tickers.json" '($tickers[0] | INDEX(.instrument_name)) as $quotes | [$instruments[0][] | . as $i | ($quotes[$i.instrument_name] // {}) as $q | {instrument_name:$i.instrument_name, expiration_timestamp:$i.expiration_timestamp, strike:$i.strike, option_type:$i.option_type, base_currency:$i.base_currency, quote_currency:$i.quote_currency, settlement_currency:$i.settlement_currency, contract_size:$i.contract_size, best_bid_price:$q.best_bid_price, best_ask_price:$q.best_ask_price, mark_price:$q.mark_price, mark_iv:$q.mark_iv, greeks:$q.greeks, open_interest:$q.open_interest, volume:$q.stats.volume, timestamp:$q.timestamp}]' > "$batch_dir/chain.json"
batch_finished=$(date -u +%Y-%m-%dT%H:%M:%SZ)
jq --arg finished "$batch_finished" --argjson count "$batch_i" '. + {finished_at:$finished,status:"complete",ticker_count:$count}' "$batch_dir/batch.json" > "$batch_dir/complete.json"
printf 'complete batch=%s; chain=%s\n' "$batch_dir" "$batch_dir/chain.json"
```

If a failure occurs, `complete.json` is not generated; retain the native responses and stderr in the directory, and have the Agent report the number collected, the failure point, and the number incomplete. This example fails fast and does not retry; do not claim the data collected before failure is a complete chain. `jq` also stops when response fields are malformed, so use the last response to locate the cause. No contracts is a normal empty chain and must not be misreported as an API failure.

`complete.json` means the candidate requests are complete; it does not guarantee that every response contains every quote field. Preserve null as unknown; record missing Delta separately in the next step. `chain.json` is an Agent-derived table, with the various raw JSON-RPC envelopes saved alongside it. Read `greeks` directly when gamma, vega, theta, or similar values are needed. Explain the units of quote, volume, and other fields using the product metadata.

## Filter Delta on Fixed Data

Set `DERIBIT_BATCH_DIR` to the directory just printed and `DERIBIT_TARGET_DELTA` to a signed target, such as the common Put target `-0.25`. The default is 5 candidates; `DERIBIT_TOP` can be changed to a positive integer. This snippet reads only the completed batch and makes no market-data requests; selecting a new target also reuses this batch.

```bash
set -euo pipefail
: "${DERIBIT_BATCH_DIR:?set completed batch directory}"
: "${DERIBIT_TARGET_DELTA:?set signed target delta}"
option_top=${DERIBIT_TOP:-5}
jq -e '.status == "complete"' "$DERIBIT_BATCH_DIR/complete.json" >/dev/null
jq -n --argjson target "$DERIBIT_TARGET_DELTA" --argjson top "$option_top" --slurpfile batch "$DERIBIT_BATCH_DIR/complete.json" --slurpfile chain "$DERIBIT_BATCH_DIR/chain.json" '
  if ($target | type) != "number" or ($top | type) != "number" then error("target and top must be numeric")
  elif $top < 1 or ($top | floor) != $top then error("top must be a positive integer")
  else
    $chain[0] as $rows |
    [$rows[] | select((.greeks.delta | type) == "number") | . + {delta_distance: ((.greeks.delta - $target) | if . < 0 then -. else . end)}] as $eligible |
    {batch:$batch[0], target_delta:$target, top:$top, candidate_count:($rows | length), eligible_count:($eligible | length), excluded:[$rows[] | select((.greeks.delta | type) != "number") | {instrument_name,reason:"missing or nonnumeric delta"}], candidates:($eligible | sort_by(.delta_distance, .instrument_name) | .[:$top])}
  end'
```

The primary sort key is `abs(delta - target)`, with stable tie-breaking by instrument name; do not add liquidity or spread filters the user did not request. The result is “closest to the target among candidates with valid Delta in this batch”; when `excluded` is nonempty, it cannot be guaranteed to cover the full chain. Data may be stale; report the original collection window and do not describe the re-sorting time as the market-data time.
