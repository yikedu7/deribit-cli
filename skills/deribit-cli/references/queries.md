# Common Query Examples

These are code snippets selected by the Agent for a task, not new CLI functionality. Locate the binary as described by the Skill and store its absolute path in `DERIBIT_BIN`. Do not run the whole page; run only the calls the user needs. Shell commands do not use backslash continuations.

## Single Methods

```sh
"$DERIBIT_BIN" public get-index-price --index-name btc_usd
"$DERIBIT_BIN" public ticker --instrument-name BTC-PERPETUAL
"$DERIBIT_BIN" --timeout 5s public get-order-book --instrument-name BTC-PERPETUAL --depth 5
"$DERIBIT_BIN" --output table --color never public ticker --instrument-name BTC-PERPETUAL
"$DERIBIT_BIN" public get-instruments --currency BTC --kind option --expired false
"$DERIBIT_BIN" public get-expirations --currency BTC --kind option
"$DERIBIT_BIN" public get-book-summary-by-currency --currency BTC --kind option
"$DERIBIT_BIN" public get-historical-volatility --currency BTC
"$DERIBIT_BIN" public get-combo-ids --currency BTC --state active
```

`BTC-PERPETUAL` is only an official naming example; obtain expiring-option identifiers from current metadata and do not reuse stale example names. Obtain the real `combo_id` for combo details from the list.

## Native Parameters and Error Checking

The following three lines are alternative invocation forms; `order-book-params.json` should contain `{"instrument_name":"BTC-PERPETUAL","depth":5}`.

```sh
"$DERIBIT_BIN" --params '{"instrument_name":"BTC-PERPETUAL","depth":5}' public get-order-book
"$DERIBIT_BIN" --params-file ./order-book-params.json public get-order-book
printf '%s\n' '{"instrument_name":"BTC-PERPETUAL","depth":5}' | "$DERIBIT_BIN" --params-stdin public get-order-book
```

Use a separate temporary directory to retain the native response and diagnostics. Check the exit code before reading `.result`; do not pipe the CLI directly into `jq` while ignoring the CLI exit status.

```sh
query_dir=$(mktemp -d)
if "$DERIBIT_BIN" --output json public ticker --instrument-name BTC-PERPETUAL > "$query_dir/response.json" 2> "$query_dir/stderr.txt"; then
  jq -e 'if has("error") or (has("result") | not) then error("not a successful RPC response") else .result end' "$query_dir/response.json"
else
  query_rc=$?
  printf 'deribit-cli failed: exit=%s; evidence=%s\n' "$query_rc" "$query_dir" >&2
fi
```

This snippet only illustrates result handling; when embedded in automation, propagate `query_rc` in the failure branch and do not continue calculating. `jq -e` returns a nonzero status for valid `false`/`null` results; the generic success check across methods should be `has("result") and (has("error") | not)`, followed by separate payload extraction. The successful result for this ticker example should be an object.

## Historical Ranges

First convert the user-specified start and end times (including timezone) to Unix milliseconds and assign them to `DERIBIT_START_MS` and `DERIBIT_END_MS`; confirm that the start is earlier than the end. Do not pass local date strings directly. The following is a Bash required-variable check; run it in a separate shell so it does not change the interactive shell's error-handling settings.

```bash
: "${DERIBIT_START_MS:?set start Unix milliseconds}"
: "${DERIBIT_END_MS:?set end Unix milliseconds}"
"$DERIBIT_BIN" public get-funding-rate-history --instrument-name BTC-PERPETUAL --start-timestamp "$DERIBIT_START_MS" --end-timestamp "$DERIBIT_END_MS"
"$DERIBIT_BIN" public get-volatility-index-data --currency BTC --start-timestamp "$DERIBIT_START_MS" --end-timestamp "$DERIBIT_END_MS" --resolution 3600
```

These are different queries; use whichever is needed. DVOL's `resolution` uses seconds or `1D`; do not apply the candle endpoint's minute-based resolution. Use the response continuation/time range to determine whether another page is needed, and avoid claiming that one result covers the entire historical interval.

## Two Levels of Option-Chain Detail

- Quote summary only: save one `get-instruments` response and one `get-book-summary-by-currency` response, join them by `instrument_name`, and filter first by the metadata expiration and underlying. Keep contracts with missing market data and their missing status; do not treat missing bid/ask as zero.
- Greeks/Delta required: use the per-instrument ticker collection in the [option-chain example](options.md). Do not assume that book summary contains Greeks, and do not take only part of the strike range to reduce requests while claiming the closest result across the full chain.
