---
name: deribit-cli
description: Use deribit-cli to query Deribit public market data, instruments, options, volatility, funding, and published combos; combine queries, summarize option chains, and select options nearest a target Delta within a specified expiry. For Deribit data queries and analysis, not trading, account access, or CLI source development.
---

# Deribit Public Market Data

Use the existing Rust CLI to obtain official public data; the Agent combines calls and analyzes them according to the user's question. All access is read-only and unauthenticated; do not read, request, or forward credentials. This Skill provides operating instructions and examples; it does not add CLI subcommands or a dedicated analysis runtime.

## Locate the CLI

- Prefer the executable found by `command -v deribit-cli` and record its absolute path as `DERIBIT_BIN`.
- If it is not in PATH, resolve this Skill's real source directory (following discovery links); the repository is two levels above it. Check that repository's `target/release/deribit-cli` and `target/debug/deribit-cli`; do not rely on the current task's working directory.
- Run `"$DERIBIT_BIN" version` and `"$DERIBIT_BIN" coverage` to confirm the version, snapshot, and available commands. These entry points read only local metadata and do not access the network. The repository-built snapshot/hash must match its embedded manifest; do not trust a stale build based only on its filename.
- If no usable binary exists, run `cargo build --locked --offline` in the owning repository using the toolchain specified by `rust-toolchain.toml`, then use the debug binary. First check `cargo --version` and `rustc --version`; an old Homebrew tool in PATH may bypass rustup. Use `rustup which --toolchain 1.85.0 cargo` to locate the installed pinned toolchain (use the version in the repository file), put the returned path's parent first in PATH for this build, and ensure Cargo, rustc, and Cargo subcommands come from the same toolchain. `rustup run` alone may still invoke an old subcommand from PATH. If offline dependencies or the toolchain are missing, report what is missing and do not automatically install a toolchain. Do not assume crates.io has published the package or automatically download an unknown package with the same name.
- `jq` is an optional JSON-processing tool used only in reference examples; if it is unavailable, the Agent should use data-processing capabilities already present in the environment. Do not automatically add Python or other project dependencies to run the Skill.

## Choose Queries

First confirm supported methods with `coverage`, then read `public <method> --help` as needed. Do not duplicate the full 38-method catalog, to avoid drifting from the installed version.

| User intent | Existing command entry points |
| --- | --- |
| Index, ticker, order book | `get-index-price`, `ticker`, `get-order-book` |
| Instruments, expiries | `get-instruments`, `get-expirations` |
| Option market summary | `get-book-summary-by-currency`, joined to metadata by instrument |
| Option chains, Greeks, target Delta | Read [option workflow and examples](references/options.md) as needed |
| DVOL, historical volatility | `get-volatility-index-data`, `get-historical-volatility` |
| Perpetual funding | `get-funding-rate-history`, `get-funding-rate-value` |
| Published combos | `get-combos`, `get-combo-ids`, `get-combo-details` |

All of these methods are in the `public` command group. [Common query examples](references/queries.md) provide parameter entry points, historical ranges, and error-handling examples; read them only for relevant queries.

## Essential Call Rules

- The full form is `deribit-cli [GLOBAL_OPTIONS] public <method> [PARAM_FLAGS]`. Put `--env`, `--output`, `--timeout`, and `--params*` before `public`; put method parameters after the method name.
- The defaults are mainnet, JSON, and a 10-second timeout. When the user specifies testnet, switch the entire batch and label the environment; never mix data from the two environments.
- Convert official underscores to hyphens in method names and flags; retain official underscores in JSON keys. Preserve parameter-value case and instrument identifiers exactly.
- When using JSON input, choose exactly one parameter entry point: `--params`, `--params-file`, or `--params-stdin`; do not combine it with method parameter flags. Pass a parameter object, not a JSON-RPC envelope.
- Write boolean parameters explicitly as `true` or `false`; use the units stated by help for time parameters, with current timestamp parameters in Unix milliseconds. Omitting an optional parameter delegates to the upstream default.
- Retain the complete JSON-RPC envelope for successful JSON, with data in `.result`; do not treat every JSON value on stdout as success. API errors also go to stdout and diagnostics to stderr; check the process exit code and `.error` first.
- Exit codes: `0` success (possibly empty); `2` usage; `3` JSON input; `4` API; `5` transport/protocol; `6` local I/O; `70` internal error; `124` timeout. Retain stdout/stderr on failure; do not mask failure with an empty array.
- Each CLI call makes one request, with no automatic pagination or retry. The Agent combines calls only within the explicit scope needed for the user's task and estimates request volume; on rate limits, network errors, or timeouts, stop the batch and report the completed portion, without unbounded retries. Advance pagination explicitly using the method's official cursor, sequence, or time parameters, stopping at the requested range or when the cursor does not advance.
- Always use JSON for machine processing. `--output table` is for human reading only; do not parse tables for subsequent calculations.

## Combined Analysis and Result Interpretation

First clarify necessary conditions that change the query (underlying, exact expiry, signed target Delta; Call/Put is optional), then choose the rest from context. Do not treat a natural-language underlying directly as the API `currency`: it may belong to USDC-settled contracts, for example. Use metadata such as `get-instruments --currency any --kind option` to confirm the instrument and base/quote/settlement currencies.

Filter Delta with `abs(delta - target_delta)` and retain the Delta sign; do not use `abs(abs(delta) - abs(target))`. When the user says only “25 Delta Put,” explicitly state that it is interpreted as `-0.25`; if the user gives a positive target with a Put condition, retain the target and do not silently change its sign.

Use a single batch collected instrument by instrument: record the environment, collection start and end times, each ticker timestamp, and the filtering criteria. Once collection is complete, freeze the raw responses; reuse the same batch for aggregation, sorting, and display, and do not refresh winners separately and mix them into the batch. State that these are data from the collection window, not an exchange snapshot taken at one instant. Treat any later refresh as a new batch.

Match expiration exactly using the contract metadata's `expiration_timestamp`; when the user gives a date, first confirm its corresponding UTC expiration date, and do not substitute the nearest expiration. Zero matches are a normal empty result. Preserve missing/null fields as unknown rather than filling them with zero; contracts without a numeric Delta do not participate in ranking, and report the number and reasons for omissions. If a request fails, report only the partial collection status and do not claim a chain-wide optimum.

Results include the source, environment, data time/collection window, filtering criteria, candidate count/missing items, and necessary units. Preserve the official meanings of fields such as price, IV, Greeks, and open interest; do not directly compare different settlement currencies without conversion, and mark units as unknown when uncertain. Clearly label derived results as Agent analysis rather than presenting them as native API responses.
