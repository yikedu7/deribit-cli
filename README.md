# deribit-cli

A Rust CLI for Deribit market data, account queries, exact JSON-RPC calls, and WebSocket subscriptions.

Current release: v0.2.0. Download the archive for your platform and `SHA256SUMS` from the [GitHub Release](https://github.com/yikedu7/deribit-cli/releases/tag/v0.2.0). Supported targets are macOS Apple Silicon and Intel, Linux x86-64 and ARM64, and Windows x86-64.

The current source build uses one command tree. Named commands and `rpc call` create the same call plan, then share permission checks and one WebSocket execution path. `catalog` and shell completion stay local.

## Install from source

Rust 1.85 or newer is required. From the repository root:

```sh
cargo build --release --locked
cli=./target/release/deribit-cli
```

The executable is `deribit-cli.exe` on Windows. With the dependency cache available, run the offline suite before publishing a source revision:

```sh
cargo test --offline --locked --all-targets
```

Published v0.1.0 archives remain the previous read-only release. Rebuild that release only from its release commit and its own `Cargo.lock`; do not treat this source tree as a drop-in replacement for those binaries.

## Commands

Put `--env mainnet` or `--env testnet` before every network command. There is no default environment, endpoint override, or fallback to the other profile.

| Group | Use it for | Example |
| --- | --- | --- |
| `market` | Public prices, instruments, order books, and trades | `market ticker --instrument-name BTC-PERPETUAL` |
| `orders` | Open orders | `orders list` |
| `portfolio` | Positions and margin | `portfolio positions --currency BTC` |
| `account` | Account information and announcements | `account summary --currency BTC` |
| `wallet` | Transfer history | `wallet transfers --currency BTC` |
| `stream` | Channel subscriptions and events | `stream subscribe ticker.BTC-PERPETUAL.100ms` |
| `rpc` | Exact methods and ordered steps on one connection | `rpc call public/get_time` |
| `catalog` | Local method, channel, scope, and evidence lookup | `catalog method private/move_positions` |

```sh
"$cli" --help
"$cli" catalog method public/ticker
"$cli" --env testnet market ticker --instrument-name BTC-PERPETUAL
printf '%s
' '{"instrument_name":"BTC-PERPETUAL"}' | "$cli" --env testnet rpc call public/ticker --params-stdin
```

The two ticker commands send the same method and parameters. Use `rpc call` or `rpc session` for methods that do not have a named command, including Mass Quote, Block Trade, and position moves. Add a named command only after its workflow, parameter contract, and focused tests are explicit.

## Authentication and safety

Public reads need no credentials. Private calls require `--authenticate`. Mainnet credentials are `DERIBIT_MAINNET_CLIENT_ID` and `DERIBIT_MAINNET_CLIENT_SECRET`; Testnet defaults to `DERIBIT_TESTNET_CLIENT_ID` and `DERIBIT_TESTNET_CLIENT_SECRET`. Credential-variable overrides and the 1Password FIFO mount are Testnet-only.

Mainnet authentication also requires an explicit `--scope`, such as `connection account:read`. The CLI checks the granted scope before dispatch. Writes require `--allow-write`, Mainnet actions also require `--allow-mainnet-write`, and special actions require `--allow-special`. These switches authorize the request; Deribit still authorizes the account.

If a write is sent and the connection fails before its outcome is known, the command exits without retry. Reconcile that profile before sending another write. A session is ordered: earlier successful steps stay applied when a later step fails.

## Output

A single call prints one redacted JSON-RPC response by default. `--output table` renders a named query for reading; API errors remain JSON. `stream subscribe` and `rpc session` print JSON Lines. Subscription acknowledgements and later events are separate lines. Diagnostics go to stderr.

Exit codes are `0` success, `2` usage, `3` input, `4` API error, `5` connection or protocol failure, `6` local I/O, `70` internal failure, and `124` timeout or an uncertain write.

## Local catalog

`catalog`, help, and `completion bash|zsh|fish` do not open a connection or read credentials. The catalog contains 187 RPC methods and 43 channel templates. It keeps the official method group, while `cli_command` records the named command when one exists. `testnet_verified`, `testnet_acknowledged`, and `testnet_event_received` describe historical Testnet evidence only. The recorded snapshot verified 145/187 methods, including 37/37 Trading methods, and received acknowledgements for 41/43 channel templates and events for 27/43.

## Agent skill

The [deribit-cli skill](skills/deribit-cli/SKILL.md) uses the same command tree for public market data and option analysis. It does not authorize account access or trading. Point an agent at the complete `skills/deribit-cli` directory so `SKILL.md`, `references`, and `agents` stay together.

## License

MIT License. See [LICENSE](LICENSE).

## Disclaimer

This project is not affiliated with Deribit.
