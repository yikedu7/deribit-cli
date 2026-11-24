use std::{
    ffi::OsStr,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use deribit_cli::ManifestRegistry;
use serde_json::Value;

#[path = "../common/process.rs"]
#[allow(dead_code)]
mod process_harness;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_deribit-cli"))
}

fn run_binary<I, S>(args: I, stdin: &[u8], current_dir: Option<&Path>) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_binary_with_env(args, stdin, current_dir, &[])
}

fn run_binary_with_env<I, S>(
    args: I,
    stdin: &[u8],
    current_dir: Option<&Path>,
    environment: &[(&str, &str)],
) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(binary());
    command
        .args(args)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(directory) = current_dir {
        command.current_dir(directory);
    }
    for (name, value) in environment {
        command.env(name, value);
    }
    let mut child = command.spawn().unwrap();
    if !stdin.is_empty() {
        child.stdin.take().unwrap().write_all(stdin).unwrap();
    }
    child.wait_with_output().unwrap()
}

fn assert_process(output: &Output, exit: i32, stdout_empty: bool, stderr_empty: bool) {
    assert_eq!(output.status.code(), Some(exit));
    assert_eq!(output.stdout.is_empty(), stdout_empty);
    assert_eq!(output.stderr.is_empty(), stderr_empty);
}

#[test]
fn product_binary_local_commands_and_completion_are_snapshot_bound() {
    let version = run_binary(["--version"], b"ignored stdin", None);
    assert_process(&version, 0, false, true);
    assert_eq!(
        version.stdout,
        b"deribit-cli 0.2.0
"
    );

    let catalog = run_binary(["catalog"], b"ignored stdin", None);
    assert_process(&catalog, 0, false, true);
    let catalog_json: Value = serde_json::from_slice(&catalog.stdout).unwrap();
    assert_eq!(catalog_json["methods"].as_array().unwrap().len(), 187);
    assert_eq!(catalog_json["channels"].as_array().unwrap().len(), 43);

    let registry = ManifestRegistry::embedded().unwrap();
    for shell in ["bash", "zsh", "fish"] {
        let completion = run_binary(["completion", shell], b"ignored stdin", None);
        assert_process(&completion, 0, false, true);
        let text = std::str::from_utf8(&completion.stdout).unwrap();
        assert!(text.contains("deribit-cli"));
        assert!(text.contains("market"));
        assert!(text.contains("rpc"));
        assert!(text.contains("catalog"));
        assert!(!text.contains(" public "));
        assert!(!text.contains("coverage"));
        for operation in registry.operations() {
            let token = operation.command_tokens()[1].as_str();
            let named = token.strip_prefix("get-").unwrap_or(token);
            if !matches!(
                operation.method(),
                "public/get_time" | "public/status" | "public/test"
            ) {
                assert!(text.contains(named), "{named}");
            }
        }
        assert!(!text.contains("--method"));
        assert!(!text.contains("--endpoint"));
    }
}

#[test]
fn product_binary_argv_stdin_file_and_help_obey_process_contract() {
    for args in [
        vec!["--help"],
        vec!["market", "--help"],
        vec!["market", "ticker", "--help"],
    ] {
        let help = run_binary(args, b"ignored stdin", None);
        assert_process(&help, 0, false, true);
    }

    let unknown = run_binary(["private", "get-position"], b"", None);
    assert_process(&unknown, 2, true, false);

    let misplaced = run_binary(["market", "ticker", "--env", "testnet"], b"", None);
    assert_process(&misplaced, 2, true, false);

    let duplicate = run_binary(
        [
            "--env",
            "mainnet",
            "--env",
            "testnet",
            "rpc",
            "call",
            "public/get_time",
        ],
        b"",
        None,
    );
    assert_process(&duplicate, 2, true, false);

    let invalid_inline = run_binary(
        ["--params", "{\"instrument_name\":", "market", "order-book"],
        b"",
        None,
    );
    assert_process(&invalid_inline, 2, true, false);

    let invalid_stdin = run_binary(
        [
            "--env",
            "testnet",
            "rpc",
            "call",
            "public/get_order_book",
            "--params-stdin",
        ],
        b"{\"instrument_name\":",
        None,
    );
    assert_process(&invalid_stdin, 3, true, false);

    let workspace = process_harness::TestWorkspace::new().unwrap();
    let invalid_file = workspace.write("invalid.json", b"[]").unwrap();
    let invalid_file = run_binary(
        [
            OsStr::new("--params-file"),
            invalid_file.as_os_str(),
            OsStr::new("market"),
            OsStr::new("order-book"),
        ],
        b"",
        Some(workspace.path()),
    );
    assert_process(&invalid_file, 2, true, false);

    let missing_file = run_binary(
        ["--params-file", "missing.json", "market", "order-book"],
        b"",
        Some(workspace.path()),
    );
    assert_process(&missing_file, 2, true, false);

    let _workspace = workspace;
}
