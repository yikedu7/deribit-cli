use std::{
    ffi::OsStr,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use deribit_cli::transport::{Transport, TransportError, TransportRequest, TransportResponse};
use deribit_cli::{
    ClientConfiguration, ConnectTimeout, ManifestRegistry, OperationTimeout, ProcessExit,
    ProxySettings, ResponseBodyLimit, parse_from_with_stdin, run_parsed_with_transport,
};
use serde_json::{Value, json};

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
    assert_eq!(version.stdout, b"deribit-cli 0.1.0\n");

    let version = run_binary(["version"], b"ignored stdin", None);
    assert_process(&version, 0, false, true);
    let version_json: Value = serde_json::from_slice(&version.stdout).unwrap();
    assert_eq!(version_json["tool_version"], "0.1.0");
    assert_eq!(version_json["protocol"], "json-rpc-2.0");
    assert!(version_json.pointer("/endpoints/mainnet").is_some());
    assert!(version_json.pointer("/endpoints/testnet").is_some());

    let coverage = run_binary(["coverage"], b"ignored stdin", None);
    assert_process(&coverage, 0, false, true);
    let coverage_json: Value = serde_json::from_slice(&coverage.stdout).unwrap();
    assert_eq!(coverage_json["methods"].as_array().unwrap().len(), 38);
    assert!(
        coverage_json["methods"]
            .as_array()
            .unwrap()
            .iter()
            .all(|method| {
                method["auth"] == false
                    && method["command"].as_str().unwrap().starts_with("public ")
                    && method["api_method"]
                        .as_str()
                        .unwrap()
                        .starts_with("public/")
            })
    );

    let registry = ManifestRegistry::embedded().unwrap();
    for shell in ["bash", "zsh", "fish"] {
        let completion = run_binary(["completion", shell], b"ignored stdin", None);
        assert_process(&completion, 0, false, true);
        let text = std::str::from_utf8(&completion.stdout).unwrap();
        assert!(text.contains("deribit-cli"));
        assert!(text.contains("--params-stdin"));
        for operation in registry.operations() {
            assert!(text.contains(operation.command_tokens()[1].as_str()));
        }
        assert!(!text.contains("private/"));
        assert!(!text.contains("--method"));
        assert!(!text.contains("--endpoint"));
    }
}

#[test]
fn product_binary_argv_stdin_file_and_help_obey_process_contract() {
    for args in [
        vec!["--help"],
        vec!["public", "--help"],
        vec!["public", "get-time", "--help"],
    ] {
        let help = run_binary(args, b"ignored stdin", None);
        assert_process(&help, 0, false, true);
    }

    let unknown = run_binary(["private", "get-position"], b"", None);
    assert_process(&unknown, 2, true, false);

    let misplaced = run_binary(["public", "get-time", "--env", "testnet"], b"", None);
    assert_process(&misplaced, 2, true, false);

    let duplicate = run_binary(
        ["--env", "mainnet", "--env", "testnet", "public", "get-time"],
        b"",
        None,
    );
    assert_process(&duplicate, 2, true, false);

    let invalid_inline = run_binary(
        [
            "--params",
            "{\"instrument_name\":",
            "public",
            "get-order-book",
        ],
        b"",
        None,
    );
    assert_process(&invalid_inline, 3, true, false);

    let invalid_stdin = run_binary(
        ["--params-stdin", "public", "get-order-book"],
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
            OsStr::new("public"),
            OsStr::new("get-order-book"),
        ],
        b"",
        Some(workspace.path()),
    );
    assert_process(&invalid_file, 3, true, false);

    let missing_file = run_binary(
        ["--params-file", "missing.json", "public", "get-order-book"],
        b"",
        Some(workspace.path()),
    );
    assert_process(&missing_file, 6, true, false);

    let valid_file = workspace
        .write(
            "valid.json",
            br#"{"instrument_name":"BTC-PERPETUAL","depth":5}"#,
        )
        .unwrap();
    let proxy_policy = [(
        "HTTPS_PROXY",
        "http://fixture-user:sentinel-value@127.0.0.1",
    )];
    let direct = run_binary_with_env(
        [
            "--env",
            "testnet",
            "--timeout",
            "1s",
            "public",
            "get-order-book",
            "--instrument-name",
            "BTC-PERPETUAL",
            "--depth",
            "5",
        ],
        b"",
        Some(workspace.path()),
        &proxy_policy,
    );
    let inline = run_binary_with_env(
        [
            "--params",
            "{\"instrument_name\":\"BTC-PERPETUAL\",\"depth\":5}",
            "public",
            "get-order-book",
        ],
        b"",
        Some(workspace.path()),
        &proxy_policy,
    );
    let file = run_binary_with_env(
        [
            OsStr::new("--params-file"),
            valid_file.as_os_str(),
            OsStr::new("public"),
            OsStr::new("get-order-book"),
        ],
        b"",
        Some(workspace.path()),
        &proxy_policy,
    );
    let stdin = run_binary_with_env(
        ["--params-stdin", "public", "get-order-book"],
        br#"{"instrument_name":"BTC-PERPETUAL","depth":5}"#,
        Some(workspace.path()),
        &proxy_policy,
    );
    for output in [direct, inline, file, stdin] {
        assert_process(&output, 2, true, false);
        let diagnostic = String::from_utf8(output.stderr).unwrap();
        assert!(diagnostic.contains("proxy userinfo"));
        assert!(!diagnostic.contains("sentinel-value"));
    }
}

enum FailureMode {
    ApiError,
    Transport,
    Internal,
    Timeout,
}

struct ExitTransport(FailureMode);

impl Transport for ExitTransport {
    fn execute(
        &self,
        request: &TransportRequest,
        _deadline: deribit_cli::Deadline,
    ) -> Result<TransportResponse, TransportError> {
        match self.0 {
            FailureMode::ApiError => {
                let request: Value = serde_json::from_slice(request.body()).unwrap();
                Ok(TransportResponse::new(
                    400,
                    serde_json::to_vec(&json!({
                        "jsonrpc": "2.0",
                        "id": request["id"],
                        "error": {"code": 10000, "message": "fixture"}
                    }))
                    .unwrap(),
                ))
            }
            FailureMode::Transport => Err(TransportError::Request),
            FailureMode::Internal => Err(TransportError::Policy("fixture invariant".into())),
            FailureMode::Timeout => Err(TransportError::Timeout),
        }
    }
}

fn parsed_and_configuration() -> (
    ManifestRegistry,
    deribit_cli::ParsedCli,
    ClientConfiguration,
) {
    let registry = ManifestRegistry::embedded().unwrap();
    let parsed = parse_from_with_stdin(
        &registry,
        ["deribit-cli", "public", "get-time"],
        &mut &b""[..],
        false,
    )
    .unwrap();
    let deribit_cli::CliCommand::Public(invocation) = &parsed.command else {
        panic!("expected public invocation")
    };
    let timeout = OperationTimeout::new(invocation.timeout).unwrap();
    let configuration = ClientConfiguration::new(
        invocation.environment,
        timeout,
        ConnectTimeout::new(timeout.duration()).unwrap(),
        ResponseBodyLimit::DEFAULT,
        ProxySettings::default(),
    )
    .unwrap();
    (registry, parsed, configuration)
}

#[test]
fn exit_codes_and_streams_cover_every_frozen_class_without_live_network() {
    let success = run_binary(["version"], b"", None);
    assert_process(&success, ProcessExit::Success.code(), false, true);
    let usage = run_binary(["no-such-command"], b"", None);
    assert_process(&usage, ProcessExit::Usage.code(), true, false);
    let input = run_binary(
        ["--params", "not-json", "public", "get-order-book"],
        b"",
        None,
    );
    assert_process(&input, ProcessExit::Input.code(), true, false);
    let local_io = run_binary(
        [
            "--params-file",
            "does-not-exist",
            "public",
            "get-order-book",
        ],
        b"",
        None,
    );
    assert_process(&local_io, ProcessExit::LocalIo.code(), true, false);

    let (registry, parsed, configuration) = parsed_and_configuration();
    for (mode, exit, stdout_empty, stderr_empty) in [
        (FailureMode::ApiError, ProcessExit::ApiNative, false, true),
        (FailureMode::Transport, ProcessExit::Transport, true, false),
        (FailureMode::Internal, ProcessExit::Internal, true, false),
        (FailureMode::Timeout, ProcessExit::Timeout, true, false),
    ] {
        let output =
            run_parsed_with_transport(&registry, &parsed, &configuration, &ExitTransport(mode));
        assert_eq!(output.exit(), exit);
        assert_eq!(output.stdout().is_empty(), stdout_empty);
        assert_eq!(output.stderr().is_empty(), stderr_empty);
    }
}
