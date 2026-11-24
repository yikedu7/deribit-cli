---
name: deribit-cli
description: Use deribit-cli to query Deribit public market data, instruments, options, volatility, funding, and published combos; combine queries, summarize option chains, and select options nearest a target Delta within a specified expiry. For Deribit data queries and analysis, not trading, account access, or CLI source development.
---

# Deribit Public Market Data

Use the installed `deribit-cli` for official public data. Keep every call in this skill read-only and unauthenticated. Do not add `--authenticate`, write switches, credentials, or private methods for market analysis.

Network commands require an explicit environment before the command group.

## Locate the CLI

- Prefer `command -v deribit-cli` and record its absolute path as `DERIBIT_BIN`.
- If it is absent from `PATH`, resolve this skill's real directory and check `target/release/deribit-cli` and `target/debug/deribit-cli` two levels above it.
- Run `"$DERIBIT_BIN" --version` and `"$DERIBIT_BIN" catalog method public/ticker`. Both are local and do not use the network.
- Build with `cargo build --locked --offline` only in the owning repository and only with the toolchain named by `rust-toolchain.toml`. Do not install a toolchain or download a package automatically when one is missing.

## Choose the Command

| User intent | Command |
| --- | --- |
| Index, ticker, order book | `market index-price`, `market ticker`, `market order-book` |
| Instruments and expiries | `market instruments`, `market expirations` |
| Option market summary | `market book-summary-by-currency` |
| Option chains and Delta | Read [option workflow](references/options.md) |
| DVOL and historical volatility | `market volatility-index-data`, `market historical-volatility` |
| Perpetual funding | `market funding-rate-history`, `market funding-rate-value` |
| Published combos | `market combos`, `market combo-ids`, `market combo-details` |

Confirm an unfamiliar flag with `"$DERIBIT_BIN" market <command> --help`. Use `catalog method <official/method>` to see its official group, scope, and whether a named command exists. Do not copy a stale 38-method list into the answer.

## Call Rules

- The form is `deribit-cli --env mainnet|testnet [OUTPUT] market <command> [FLAGS]`.
- There is no implicit environment. Keep one batch entirely on the environment the user requested.
- JSON is the default. `--output table` is for a person to read; do not parse it.
- Put global options before `market` and method flags after the command. Official method names use underscores, while command and flag names use hyphens.
- A command makes one request. It does not retry or advance pagination automatically.
- Check the exit status before reading `.result`. Exit `0` can still be an empty result. Exit `4` means the response contains an API error. Exit `5` is connection or protocol failure, and `124` is a timeout.
- On a rate limit, network error, or timeout, stop the batch and report the completed portion.

## Combined Analysis

Resolve a natural-language underlying through instrument metadata before choosing `currency`. Filter a target Delta with `abs(delta - target_delta)` and preserve the sign. Interpret an unqualified “25 Delta Put” as `-0.25` and say that explicitly.

Collect one batch, record its environment and start and finish times, then freeze the raw responses. Do not refresh individual winners and mix them back into the batch. Match an expiry with `expiration_timestamp`; do not substitute a nearby date. Preserve missing values as unknown, and do not rank contracts without a numeric Delta.

Label prices, IV, Greeks, and open interest with their official units. Do not compare different settlement currencies without a conversion. Mark derived ranking or aggregation as analysis rather than exchange output.
