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

Build the `deribit-cli-api` crate from a repository checkout to get the original public commands plus `api` for the shared JSON-RPC and WebSocket catalog. Its manifest is at `api-cli/Cargo.toml`. Update build scripts that used `testnet-v2/Cargo.toml` to use this path. The root Cargo package and published v0.1.0 archives contain the public CLI.

Both builds produce an executable named `deribit-cli` (`deribit-cli.exe` on Windows). The command below puts the general API build in its own Cargo target directory. Invoke that binary by path to select it.

```sh
cargo build --release --locked --manifest-path api-cli/Cargo.toml --target-dir api-cli/target
cli=./api-cli/target/release/deribit-cli
```

Help and catalog inspection are local and require no credentials. Use `api --help` to identify the general API build. The `coverage` command still reports the 38 public methods.

```sh
"$cli" api --help
"$cli" api --env mainnet catalog
"$cli" api --env testnet catalog
"$cli" testnet catalog
```

With dependencies cached, run the general API suite offline. It uses local fixtures and loopback servers.

```sh
cargo test --offline --locked --all-targets --manifest-path api-cli/Cargo.toml --target-dir api-cli/target
```

`api` requires `--env mainnet` or `--env testnet`. The profiles use the fixed official WebSocket endpoints at `www.deribit.com` and `test.deribit.com` respectively. A failed request stays on the selected profile. The general API binary also supports the legacy `testnet` command, which always selects Testnet.

Public reads need no credentials. For private calls, `--authenticate` reads the selected profile’s variables from the process environment. Mainnet uses `DERIBIT_MAINNET_CLIENT_ID` and `DERIBIT_MAINNET_CLIENT_SECRET`. Testnet defaults to `DERIBIT_TESTNET_CLIENT_ID` and `DERIBIT_TESTNET_CLIENT_SECRET`. The credential-variable overrides and FIFO mount are available only for Testnet. Mainnet authentication also requires an explicit `--scope`, for example `connection account:read`, and the CLI checks the granted scope before private calls. Write calls require `--allow-write`, Mainnet writes additionally require `--allow-mainnet-write`, and special actions require `--allow-special`. Deribit authorizes each operation.

The catalog contains 187 RPC methods and 43 WebSocket channel templates. Its `testnet_prerequisite` fields describe the setup needed to validate entries on Testnet. Recorded Testnet runs verified 145/187 RPC methods, including 37/37 Trading methods, and received subscription acknowledgments for 41/43 templates and events for 27/43. These totals cover the tested scenarios for each method or template. Mainnet private and write paths were checked with offline fixtures.

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
