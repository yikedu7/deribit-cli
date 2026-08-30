# deribit-cli

A read-only command-line interface for Deribit public market data.

Current release: v0.1.0.

## Download

Download the archive for your platform from [GitHub Releases](https://github.com/yikedu7/deribit-cli/releases/latest), extract it, and place `deribit-cli` (or `deribit-cli.exe` on Windows) on your `PATH`.

Supported release targets:

- macOS: Apple Silicon and Intel
- Linux: x86-64 and ARM64
- Windows: x86-64

Use the attached `SHA256SUMS` file to verify the download.

## Quick start

```sh
deribit-cli public ticker --instrument-name BTC-PERPETUAL
deribit-cli --output table public get-order-book --instrument-name BTC-PERPETUAL
deribit-cli public --help
```

Mainnet is the default. Add `--env testnet` before `public` to use Deribit Testnet.

## Scope and safety

The v0.1.0 command set covers 38 Deribit public market-data methods. It is read-only, does not accept credentials, and does not expose trading, account, or private API methods.

## License

MIT License. See [LICENSE](LICENSE).

## Disclaimer

This project is not affiliated with Deribit.
