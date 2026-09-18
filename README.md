# deribit-cli

A read-only Rust CLI and agent skill for Deribit public market data and options analysis.

Current release: v0.1.0.

Query market data directly from your terminal, or let an agent compose CLI calls to build option chains and find contracts nearest a target Delta. No API keys required.

## Features

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

## Build from source

Rust 1.85 or newer is required.

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

## Agent skill

The repository includes a [deribit-cli skill](skills/deribit-cli/SKILL.md) that teaches an agent how to discover available commands, combine queries, and interpret results. The agent handles composition; the CLI remains a set of public-data primitives, with no additional analysis commands or dedicated analysis runtime.

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
