# deribit-cli

A Rust CLI for Deribit and an agent skill for public market data and options analysis.

Current release: v0.1.0.

Query market data directly from your terminal, or let an agent compose CLI calls to build option chains and find contracts nearest a target Delta. Public market-data queries require no API keys.

## v0.1.0 features

- 38 public methods covering instruments, tickers, order books, trades, volatility, funding, and published combos.
- Native JSON-RPC output for scripts and table output for terminal use.
- Mainnet by default, with explicit Testnet support.
- An English-language [agent skill](skills/deribit-cli/SKILL.md) with reusable Bash/jq examples for common queries and option analysis.
- Read-only access: no trading, account access, or private APIs.

## Download

Download the archive for your platform from [GitHub Releases](https://github.com/yikedu7/deribit-cli/releases/latest), extract it, and place `deribit-cli` (or `deribit-cli.exe` on Windows) on your `PATH`.

Supported release targets:

- macOS: Apple Silicon and Intel
- Linux: x86-64 and ARM64
- Windows: x86-64

Use the attached `SHA256SUMS` file to verify the download.

## Build the v1 public CLI from source

Rust 1.85 or newer is required. Run these commands from the repository root. The binary is `target/release/deribit-cli` with the default Cargo target directory.

```sh
cargo build --release --locked
```

The repository contains the product unit tests plus public CLI, manifest-correspondence, and read-only security tests:

```sh
cargo test --locked --all-targets
```

## Quick start

```sh
deribit-cli version
deribit-cli coverage
deribit-cli public get-index-price --index-name btc_usd
deribit-cli public ticker --instrument-name BTC-PERPETUAL
deribit-cli --output table public get-order-book --instrument-name BTC-PERPETUAL
deribit-cli public get-instruments --currency BTC --kind option --expired false
deribit-cli public --help
```

Mainnet is the default. Add `--env testnet` before `public` to use Deribit Testnet.

Place global options before `public` and method parameters after the method name. Use `deribit-cli public <method> --help` for supported parameters.

JSON is the default output format. Successful responses retain the JSON-RPC envelope, with data in `.result`. Check the exit status and `.error` before processing results; use `--output table` only for human-readable output. Each invocation makes one request, with no automatic retries or pagination.

## General API CLI (source build)

Enable `general-api` in the root Cargo package to add commands organized by task, exact JSON-RPC calls, and WebSocket subscriptions. The default build and published v0.1.0 binaries provide the 38 public read-only methods. The released `public` syntax and output also work in the feature build.

Build from the repository root. The executable is named `deribit-cli.exe` on Windows.

```sh
cargo build --release --locked --features general-api
cli=./target/release/deribit-cli
```

Choose `--env mainnet` or `--env testnet` before the command group for every new network command. Each profile uses its fixed official WebSocket endpoint, at `www.deribit.com` or `test.deribit.com` respectively, with no endpoint override or fallback.

### Commands by task

| Group | Use it for | Example |
| --- | --- | --- |
| `market` | Public instruments, prices, order books, and trades | `market ticker --instrument-name BTC-PERPETUAL` |
| `orders` | Your open orders | `orders list` |
| `portfolio` | Your positions | `portfolio positions --currency BTC` |
| `account` | Account details and announcements | `account summary --currency BTC` |
| `wallet` | Transfer history | `wallet transfers --currency BTC` |
| `stream` | Channel subscriptions and notifications | `stream subscribe ticker.BTC-PERPETUAL.100ms` |
| `rpc` | Exact RPC methods and sequences on one connection | `rpc call public/get_time` |
| `catalog` | Local RPC and channel discovery | `catalog method public/ticker` |

Use `<group> --help` and `<group> <command> --help` to inspect typed flags. Named commands and `rpc call` share the same method catalog, permission checks, and execution path. Both of these calls send the same method and parameters.

```sh
"$cli" --env testnet market ticker --instrument-name BTC-PERPETUAL
printf '%s\n' '{"instrument_name":"BTC-PERPETUAL"}' | "$cli" --env testnet rpc call public/ticker --params-stdin
```

Use the exact official method name for `rpc call`, including its `public/` or `private/` prefix. Supply a JSON parameter object using `--params-file` or `--params-stdin`. Use `rpc` for operations without a named command, such as Mass Quote, or for official parameters without a named flag. For draft scripts, replace `api --env testnet rpc public/get_time` or `testnet rpc public/get_time` with `--env testnet rpc call public/get_time`.

### Authentication and output

Public reads need no credentials. For private calls, add `--authenticate` to use credentials for the selected profile. Mainnet uses `DERIBIT_MAINNET_CLIENT_ID` and `DERIBIT_MAINNET_CLIENT_SECRET`. Testnet defaults to `DERIBIT_TESTNET_CLIENT_ID` and `DERIBIT_TESTNET_CLIENT_SECRET`. Credential-variable overrides and the FIFO mount are Testnet-only. Mainnet authentication requires an explicit `--scope`, such as `connection account:read`. Check the required scope with `catalog method`. The CLI checks the granted Mainnet scope before dispatch. Writes require `--allow-write`, Mainnet writes also require `--allow-mainnet-write`, and special actions require `--allow-special`. Deribit authorizes each operation.

Named commands and `rpc call` return one JSON-RPC envelope per request. A successful implicit login stays out of that output. Check the exit status and any `.error` before processing results. `stream subscribe` and `rpc session` emit JSON Lines, including the redacted login response when applicable. A subscription acknowledgment confirms the channel registration. Later notifications carry the events.

Use `rpc session --file steps.json` to run a JSON array of RPC calls and observation steps on one WebSocket connection. Successful early steps remain applied if a later step fails. Reconcile the selected profile after an uncertain write before sending another write.

### Local catalog and verification

Run `catalog` for the full JSON catalog or use an exact method or channel template to inspect one entry. These lookups and help run locally without an environment or credentials. The catalog records official groups, supported transports, authentication requirements, and Testnet prerequisites. Where a named command is available, `cli_command` gives its command group and name. `coverage` still describes the 38 released public methods.

```sh
"$cli" catalog
"$cli" catalog method private/move_positions
"$cli" catalog channel 'ticker.(instrument_name).(interval)'
```

The catalog contains 187 RPC methods and 43 WebSocket channel templates. It reports the historical snapshot date as `testnet_evidence_as_of`, method coverage as `testnet_verified`, and channel coverage as `testnet_acknowledged` and `testnet_event_received`. Each flag records whether the snapshot includes that evidence for the entry. A recorded success applies to the tested scenario. Previously recorded Testnet runs verified 145/187 methods, including 37/37 Trading methods, and received subscription acknowledgments for 41/43 templates and events for 27/43. Mainnet private and write paths were checked with offline fixtures.

With dependencies cached, run the default and feature-enabled suites offline. The general API tests use local fixtures and loopback streams.

```sh
cargo test --offline --locked --all-targets
cargo test --offline --locked --all-targets --features general-api
```

## Agent skill

The repository includes a [deribit-cli skill](skills/deribit-cli/SKILL.md) that teaches an agent how to discover public commands, combine queries, and interpret results. The skill uses the original public-data commands for agent-side composition and analysis.

To use it:

1. Install the CLI from the release downloads above, or build it from source.
2. Point your agent's skill loader at `skills/deribit-cli`, or install that complete directory using your agent's supported skill-installation mechanism. Keep its `references` and `agents` subdirectories together with `SKILL.md`.
3. Ask your agent to use the `deribit-cli` skill for a public market-data query. The reference snippets use Bash and `jq`; agents can use other available JSON-processing tools.

Example requests:

- "Show the BTC perpetual ticker and top five order-book levels."
- "List the available BTC option expirations on mainnet."
- "For this exact expiry, find the five BTC puts closest to Delta -0.25 and report missing data and the collection window."

Option analysis uses exact expiration metadata and signed Delta distance. Per-instrument responses are collected in a single batch and then analyzed without refreshing individual winners; the batch is not a simultaneous exchange snapshot.

See [common query examples](skills/deribit-cli/references/queries.md) and [option-chain and Delta examples](skills/deribit-cli/references/options.md) for adaptable code. The skill is included in the source repository, not in the standalone binary archives.

## Scope and safety

The v0.1.0 command set covers 38 Deribit public market-data methods. It is read-only, does not accept credentials, and does not expose trading, account, or private API methods.

## License

MIT License. See [LICENSE](LICENSE).

## Disclaimer

This project is not affiliated with Deribit.
