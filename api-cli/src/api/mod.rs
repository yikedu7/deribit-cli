//! Profile-bound access to the pinned Deribit API.

mod catalog;
mod policy;
mod socket;
#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use clap::{Arg, ArgAction, ArgMatches, Command, ValueEnum};
use serde::Deserialize;
use serde_json::{Map, Value};
use thiserror::Error;

use catalog::{CATALOG_JSON, Catalog, Effect};
use policy::{Scope, is_mainnet_action};
use socket::Session;

const INPUT_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Profile {
    Mainnet,
    Testnet,
}

impl ValueEnum for Profile {
    fn value_variants<'a>() -> &'a [Self] {
        &[Self::Mainnet, Self::Testnet]
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        Some(
            match self {
                Self::Mainnet => "mainnet",
                Self::Testnet => "testnet",
            }
            .into(),
        )
    }
}

impl Profile {
    fn endpoint(self) -> &'static str {
        match self {
            Self::Mainnet => "wss://www.deribit.com/ws/api/v2",
            Self::Testnet => "wss://test.deribit.com/ws/api/v2",
        }
    }

    fn accepts(self, response: &Value) -> bool {
        response.get("testnet").and_then(Value::as_bool) == Some(self == Self::Testnet)
    }
}

#[derive(Debug, Error)]
enum Failure {
    #[error("{0}")]
    Input(&'static str),
    #[error("could not load the selected profile's credentials")]
    Credentials,
    #[error("could not establish a TLS WebSocket connection to the selected Deribit profile")]
    Connect,
    #[error("Deribit connection closed or failed")]
    Connection,
    #[error("Deribit operation timed out")]
    Timeout,
    #[error("Deribit response failed JSON-RPC or environment validation")]
    Protocol,
    #[error("Deribit returned a JSON-RPC error")]
    Remote,
    #[error(
        "write outcome is unknown; reconcile state on the selected profile before attempting another write"
    )]
    UncertainWrite,
    #[error("could not write output")]
    Output,
    #[error("Deribit response did not satisfy the session expectation")]
    UnexpectedResult,
    #[error("requested or granted scope is insufficient for this Mainnet operation")]
    InsufficientScope,
}

impl Failure {
    fn code(&self) -> i32 {
        match self {
            Self::Input(_) | Self::Credentials | Self::InsufficientScope => 2,
            Self::Remote => 3,
            Self::Output => 6,
            Self::Timeout => 124,
            Self::Connect
            | Self::Connection
            | Self::Protocol
            | Self::UncertainWrite
            | Self::UnexpectedResult => 5,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Step {
    Rpc(Call),
    Observe(Observation),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    method: String,
    #[serde(default)]
    params: Map<String, Value>,
    #[serde(default, deserialize_with = "deserialize_expected_result")]
    expect_result: Option<Value>,
}

fn deserialize_expected_result<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    observe_seconds: u64,
}

struct Permissions {
    authenticate: bool,
    allow_write: bool,
    allow_special: bool,
    allow_mainnet_write: bool,
}

struct Authentication<'a> {
    enabled: bool,
    requested: Option<&'a String>,
    mainnet_scope: Option<Scope<'a>>,
}

fn command(legacy_testnet: bool) -> Command {
    let command = if legacy_testnet {
        Command::new("testnet")
            .bin_name("deribit-cli testnet")
            .about("Compatibility entry point fixed to official Deribit Testnet")
    } else {
        Command::new("api")
            .bin_name("deribit-cli api")
            .about("Use the pinned catalog over WebSocket with an explicit Deribit environment")
            .arg(
                Arg::new("env")
                    .long("env")
                    .required(true)
                    .value_parser(clap::value_parser!(Profile))
                    .help("Select the fixed official Deribit endpoint"),
            )
            .arg(switch(
                "allow-mainnet-write",
                "Allow explicitly requested Mainnet actions",
            ))
    };
    command
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg(
            Arg::new("timeout-seconds")
                .long("timeout-seconds")
                .value_parser(clap::value_parser!(u64).range(1..=300))
                .default_value("15")
                .global(true)
                .help("Deadline per connection attempt and RPC"),
        )
        .arg(switch("authenticate", "Use credentials for the selected profile"))
        .arg(
            Arg::new("scope")
                .long("scope")
                .global(true)
                .requires("authenticate")
                .help("Requested authentication scope, required on Mainnet"),
        )
        .arg(
            Arg::new("credentials-mount")
                .long("credentials-mount")
                .value_parser(clap::value_parser!(PathBuf))
                .global(true)
                .requires("authenticate")
                .help("Absolute path of an existing 1Password .env FIFO mount (Testnet only)"),
        )
        .arg(credential_variable("client-id-variable"))
        .arg(credential_variable("client-secret-variable"))
        .arg(switch("allow-write", "Allow explicitly requested writes"))
        .arg(switch("allow-special", "Allow explicitly authorized special actions"))
        .subcommand(Command::new("catalog").about("Print the pinned API and subscription matrix"))
        .subcommand(
            Command::new("rpc")
                .about("Call one exact JSON-RPC method over WebSocket")
                .arg(Arg::new("method").required(true))
                .arg(
                    Arg::new("params-file")
                        .long("params-file")
                        .value_parser(clap::value_parser!(PathBuf))
                        .conflicts_with("params-stdin"),
                )
                .arg(Arg::new("params-stdin").long("params-stdin").action(ArgAction::SetTrue)),
        )
        .subcommand(
            Command::new("session")
                .about("Execute a JSON array of RPC and observe steps on one WebSocket")
                .arg(Arg::new("file").long("file").value_parser(clap::value_parser!(PathBuf)))
                .arg(
                    Arg::new("continue-read-errors").long("continue-read-errors")
                        .action(ArgAction::SetTrue)
                        .help("Continue after a server error for independent read-only calls"),
                ),
        )
        .subcommand(
            Command::new("subscribe")
                .about("Subscribe to concrete channels and stream JSON-RPC notifications as JSON lines")
                .arg(Arg::new("channel").required(true).num_args(1..))
                .arg(
                    Arg::new("observe-seconds")
                        .long("observe-seconds")
                        .value_parser(clap::value_parser!(u64).range(1..=300))
                        .default_value("10"),
                ),
        )
}

fn switch(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .global(true)
        .action(ArgAction::SetTrue)
        .help(help)
}

fn credential_variable(name: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .global(true)
        .help("Select a supported Testnet credential variable name")
}

/// Run the common command family from the binary entrypoint.
pub fn run(args: impl IntoIterator<Item = OsString>, legacy_testnet: bool) -> i32 {
    let matches = match command(legacy_testnet).try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return code;
        }
    };
    let profile = if legacy_testnet {
        Profile::Testnet
    } else {
        *matches.get_one::<Profile>("env").expect("required profile")
    };
    let mut output = io::stdout().lock();
    match run_command(profile, matches, legacy_testnet, &mut output) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            error.code()
        }
    }
}

fn run_command(
    profile: Profile,
    matches: ArgMatches,
    legacy_testnet: bool,
    output: &mut impl Write,
) -> Result<(), Failure> {
    let (command, options) = matches.subcommand().expect("required subcommand");
    let permissions = Permissions {
        authenticate: options.get_flag("authenticate"),
        allow_write: options.get_flag("allow-write"),
        allow_special: options.get_flag("allow-special"),
        allow_mainnet_write: !legacy_testnet && options.get_flag("allow-mainnet-write"),
    };
    if profile != Profile::Mainnet && permissions.allow_mainnet_write {
        return Err(Failure::Input(
            "--allow-mainnet-write requires --env mainnet",
        ));
    }
    if command == "catalog" {
        return output
            .write_all(CATALOG_JSON.as_bytes())
            .map_err(|_| Failure::Output);
    }
    let catalog = Catalog::load();
    let continue_read_errors = command == "session" && options.get_flag("continue-read-errors");
    let requested = options.get_one::<String>("scope");
    let mainnet_scope = if profile == Profile::Mainnet && permissions.authenticate {
        Some(Scope::request(requested.ok_or(Failure::Input(
            "Mainnet authentication requires explicit --scope",
        ))?)?)
    } else {
        None
    };
    if mainnet_scope.as_ref().is_some_and(Scope::is_named) && !permissions.allow_mainnet_write {
        return Err(Failure::Input(
            "Mainnet named sessions require --allow-mainnet-write",
        ));
    }

    let mut steps = match command {
        "rpc" => {
            let file = options.get_one::<PathBuf>("params-file");
            let params = if file.is_some() || options.get_flag("params-stdin") {
                read_json(file)?
            } else {
                Map::new()
            };
            vec![Step::Rpc(Call {
                method: options.get_one::<String>("method").unwrap().to_owned(),
                params,
                expect_result: None,
            })]
        }
        "session" => read_json(options.get_one::<PathBuf>("file"))?,
        "subscribe" => subscription_steps(&catalog, options)?,
        _ => unreachable!("only declared commands are accepted"),
    };
    if steps.is_empty() || steps.len() > 100 {
        return Err(Failure::Input("a session needs between 1 and 100 steps"));
    }
    let step_count = steps.len();
    for (index, step) in steps.iter_mut().enumerate() {
        match step {
            Step::Rpc(call) => {
                validate_call(&catalog, &permissions, call)?;
                if profile == Profile::Mainnet {
                    if is_mainnet_action(call, &catalog) && !permissions.allow_mainnet_write {
                        return Err(Failure::Input(
                            "Mainnet action requires explicit --allow-mainnet-write",
                        ));
                    }
                    if call.method == "public/fork_token" && !permissions.allow_special {
                        return Err(Failure::Input(
                            "token forking requires --allow-special on Mainnet",
                        ));
                    }
                    if let Some(scope) = &mainnet_scope {
                        scope.prepare(call, &catalog)?;
                    }
                } else if call.method == "public/auth" {
                    apply_auth_scope(&mut call.params, requested);
                }
                if call.method == "private/logout" && index + 1 != step_count {
                    return Err(Failure::Input(
                        "private/logout must be the last session step",
                    ));
                }
            }
            Step::Observe(observation) if !(1..=300).contains(&observation.observe_seconds) => {
                return Err(Failure::Input("observe_seconds must be between 1 and 300"));
            }
            Step::Observe(_) => {}
        }
    }
    if continue_read_errors
        && steps.iter().any(|step| match step {
            Step::Rpc(call) => {
                catalog.method(&call.method).expect("prevalidated").effect != Effect::Read
            }
            Step::Observe(_) => true,
        })
    {
        return Err(Failure::Input(
            "--continue-read-errors accepts only independent read calls",
        ));
    }
    let credentials = if permissions.authenticate {
        Some(socket::Credentials::load(
            profile,
            options
                .get_one::<PathBuf>("credentials-mount")
                .map(PathBuf::as_path),
            options
                .get_one::<String>("client-id-variable")
                .map(String::as_str),
            options
                .get_one::<String>("client-secret-variable")
                .map(String::as_str),
        )?)
    } else {
        None
    };
    let deadline = Duration::from_secs(*options.get_one::<u64>("timeout-seconds").unwrap());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| Failure::Connect)?;
    runtime.block_on(async {
        let session = Session::connect(profile, deadline, credentials).await?;
        run_steps(
            session,
            &catalog,
            &Authentication {
                enabled: permissions.authenticate,
                requested,
                mainnet_scope,
            },
            steps,
            continue_read_errors,
            output,
        )
        .await
    })
}

async fn run_steps<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    mut session: Session<S>,
    catalog: &Catalog,
    authentication: &Authentication<'_>,
    steps: Vec<Step>,
    continue_read_errors: bool,
    output: &mut impl Write,
) -> Result<(), Failure> {
    let explicit_auth =
        matches!(steps.first(), Some(Step::Rpc(call)) if call.method == "public/auth");
    if authentication.enabled && !explicit_auth {
        let mut params = Map::new();
        if let Some(scope) = &authentication.mainnet_scope {
            scope.apply(&mut params);
        } else {
            apply_auth_scope(&mut params, authentication.requested);
        }
        let auth = session.authenticate(params, output).await?;
        // Token fields are already redacted. Emit the granted scope so a
        // live harness can distinguish permission from server support.
        write_line(output, &auth)?;
        if auth.get("error").is_some() {
            return Err(Failure::Remote);
        }
    }
    let mut remote_error = false;
    for step in steps {
        match step {
            Step::Rpc(call) => {
                let method = catalog.method(&call.method).expect("prevalidated");
                if let Some(scope) = &authentication.mainnet_scope {
                    if call.method != "public/auth" {
                        scope.check(&call, catalog, session.granted_scope())?;
                    }
                }
                let response = if call.method == "public/auth" {
                    session.authenticate(call.params, output).await?
                } else {
                    session
                        .call(&call.method, call.params, method.auth, output)
                        .await
                        .map_err(|failure| uncertain_write(method.effect, failure))?
                };
                write_line(output, &response)
                    .map_err(|failure| uncertain_write(method.effect, failure))?;
                if response.get("error").is_some() {
                    remote_error = true;
                    if !continue_read_errors || call.expect_result.is_some() {
                        return Err(Failure::Remote);
                    }
                }
                if let Some(expected) = call.expect_result {
                    verify_expected_result(&response, &expected)?;
                }
            }
            Step::Observe(value) => {
                session
                    .observe(Duration::from_secs(value.observe_seconds), output)
                    .await?;
            }
        }
    }
    session.close().await;
    if remote_error {
        Err(Failure::Remote)
    } else {
        Ok(())
    }
}

fn apply_auth_scope(params: &mut Map<String, Value>, requested: Option<&String>) {
    if let Some(scope) = requested {
        params
            .entry("scope".to_owned())
            .or_insert_with(|| Value::String(scope.clone()));
    }
}

fn validate_call(catalog: &Catalog, permissions: &Permissions, call: &Call) -> Result<(), Failure> {
    let method = catalog.method(&call.method).ok_or(Failure::Input(
        "method is absent from the pinned Deribit API catalog",
    ))?;
    if !method
        .transports
        .iter()
        .any(|transport| transport == "websocket")
    {
        return Err(Failure::Input("method is unavailable over WebSocket"));
    }
    if method.auth && !permissions.authenticate {
        return Err(Failure::Input("method requires explicit --authenticate"));
    }
    if matches!(method.effect, Effect::Write | Effect::Special) && !permissions.allow_write {
        return Err(Failure::Input("method requires explicit --allow-write"));
    }
    if (method.effect == Effect::Special || call.method == "public/exchange_token")
        && !permissions.allow_special
    {
        return Err(Failure::Input("method requires explicit --allow-special"));
    }
    if [
        "access_token",
        "refresh_token",
        "client_id",
        "client_secret",
    ]
    .iter()
    .any(|key| call.params.contains_key(*key))
    {
        return Err(Failure::Input(
            "authentication fields are supplied within the selected profile's process",
        ));
    }
    if call.method == "public/auth"
        && ["signature", "timestamp", "nonce"]
            .iter()
            .any(|key| call.params.contains_key(*key))
    {
        return Err(Failure::Input(
            "authentication signature inputs are generated locally",
        ));
    }
    if matches!(
        call.method.as_str(),
        "public/subscribe" | "private/subscribe" | "public/unsubscribe" | "private/unsubscribe"
    ) {
        let channels = call
            .params
            .get("channels")
            .and_then(Value::as_array)
            .ok_or(Failure::Input(
                "channels must be an array of concrete channel names",
            ))?;
        if channels.is_empty() {
            return Err(Failure::Input("at least one concrete channel is required"));
        }
        for channel in channels {
            let concrete = channel
                .as_str()
                .ok_or(Failure::Input("invalid channel name"))?;
            let channel = catalog
                .channel(concrete)
                .ok_or(Failure::Input("channel is absent from the pinned catalog"))?;
            if channel.auth && !call.method.starts_with("private/") {
                return Err(Failure::Input(
                    "private channels require private/subscribe or private/unsubscribe",
                ));
            }
            if channel.auth_for_raw && concrete.ends_with(".raw") && !permissions.authenticate {
                return Err(Failure::Input(
                    "raw channels require explicit --authenticate",
                ));
            }
        }
    }
    if call.method == "private/logout"
        && call.params.get("invalidate_token") != Some(&Value::Bool(false))
        && !permissions.allow_special
    {
        return Err(Failure::Input(
            "token invalidation requires explicit --allow-special",
        ));
    }
    Ok(())
}

fn subscription_steps(catalog: &Catalog, options: &ArgMatches) -> Result<Vec<Step>, Failure> {
    let mut public = Vec::new();
    let mut private = Vec::new();
    for concrete in options.get_many::<String>("channel").unwrap() {
        let row = catalog
            .channel(concrete)
            .ok_or(Failure::Input("channel is absent from the pinned catalog"))?;
        let list = if row.auth { &mut private } else { &mut public };
        list.push(Value::String(concrete.to_owned()));
    }
    let mut steps: Vec<Step> = [("public/subscribe", public), ("private/subscribe", private)]
        .into_iter()
        .filter(|(_, channels)| !channels.is_empty())
        .map(|(method, channels)| {
            Step::Rpc(Call {
                method: method.to_owned(),
                params: Map::from_iter([("channels".to_owned(), Value::Array(channels))]),
                expect_result: None,
            })
        })
        .collect();
    steps.push(Step::Observe(Observation {
        observe_seconds: *options.get_one("observe-seconds").unwrap(),
    }));
    Ok(steps)
}

fn read_json<T: serde::de::DeserializeOwned>(path: Option<&PathBuf>) -> Result<T, Failure> {
    let source: Box<dyn Read> = if let Some(path) = path {
        Box::new(File::open(path).map_err(|_| Failure::Input("could not open JSON input"))?)
    } else {
        let stdin = io::stdin();
        if stdin.is_terminal() {
            return Err(Failure::Input(
                "provide JSON input through a file or redirected stdin",
            ));
        }
        Box::new(stdin.lock())
    };
    let mut data = Vec::new();
    source
        .take(INPUT_LIMIT as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|_| Failure::Input("could not read JSON input"))?;
    if data.len() > INPUT_LIMIT {
        return Err(Failure::Input("JSON input exceeds 1 MiB"));
    }
    let text = std::str::from_utf8(&data).map_err(|_| Failure::Input("input must be UTF-8"))?;
    deribit_cli::parameters::audit_json(text)
        .map_err(|_| Failure::Input("invalid or ambiguous JSON input"))?;
    serde_json::from_str(text).map_err(|_| Failure::Input("invalid JSON input shape"))
}

fn write_line(output: &mut impl Write, value: &Value) -> Result<(), Failure> {
    serde_json::to_writer(&mut *output, value).map_err(|_| Failure::Output)?;
    output
        .write_all(b"\n")
        .and_then(|_| output.flush())
        .map_err(|_| Failure::Output)
}

fn verify_expected_result(response: &Value, expected: &Value) -> Result<(), Failure> {
    let Some(actual) = response.get("result") else {
        return Err(Failure::UnexpectedResult);
    };
    let matches = match expected {
        Value::Object(fields) => actual.as_object().is_some_and(|object| {
            fields
                .iter()
                .all(|(key, value)| object.get(key) == Some(value))
        }),
        _ => actual == expected,
    };
    if matches {
        Ok(())
    } else {
        Err(Failure::UnexpectedResult)
    }
}

#[cfg(test)]
#[test]
fn session_result_expectations_match_exact_fields_after_transport_validation() {
    use serde_json::json;
    let response = json!({"testnet": true, "result": {"id": 9, "available_funds": 0}});
    assert!(verify_expected_result(&response, &json!({"id": 9})).is_ok());
    assert!(verify_expected_result(&response, &json!({"id": "9"})).is_err());
    assert!(verify_expected_result(&response, &json!({"id": 8})).is_err());
    assert!(verify_expected_result(&json!({"testnet": false, "result": []}), &json!([])).is_ok());
    assert!(verify_expected_result(&json!({"testnet": true, "result": []}), &json!([])).is_ok());
    let call: Call = serde_json::from_value(json!({
        "method": "private/get_account_summary", "expect_result": null
    }))
    .unwrap();
    assert_eq!(call.expect_result, Some(Value::Null));
}

fn uncertain_write(effect: Effect, failure: Failure) -> Failure {
    if matches!(effect, Effect::Special | Effect::Write)
        && matches!(
            failure,
            Failure::Timeout | Failure::Protocol | Failure::Connection | Failure::Output
        )
    {
        Failure::UncertainWrite
    } else {
        failure
    }
}
