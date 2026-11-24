//! Command discovery and argument decoding for the general Deribit CLI.

use std::{io, path::PathBuf};

use clap::{Arg, ArgAction, ArgMatches, Command, builder::PossibleValuesParser};
use serde_json::{Map, Value};

use crate::api_application::{Call, Failure};
use crate::commands::parameter_arg;
use crate::parameters::resolve_parameters;
use crate::{
    CliBehavior, CompletionShell, Environment, ManifestRegistry, OutputFormat, ParameterInput,
    ParameterType, ValidatedOperation,
};

const NAMED_GROUPS: &[(&str, &str)] = &[
    (
        "market",
        "Query public prices, instruments, order books and trades",
    ),
    ("orders", "Inspect open orders"),
    ("portfolio", "Inspect positions"),
    ("account", "Inspect account information and announcements"),
    ("wallet", "Inspect wallet transfer history"),
];

const WALLET_CURRENCIES: &[&str] = &["BTC", "ETH", "USDC", "USDT", "EURR"];
const POSITION_CURRENCIES: &[&str] = &["BTC", "ETH", "USDC", "USDT", "EURR", "any"];
const ACCOUNT_CURRENCIES: &[&str] = &[
    "BTC", "ETH", "STETH", "ETHW", "USDC", "USDT", "EURR", "SOL", "XRP", "USYC", "PAXG", "BNB",
    "USDE",
];
const POSITION_KINDS: &[&str] = &["future", "option", "future_combo", "option_combo"];
const ORDER_KINDS: &[&str] = &["future", "option", "spot", "future_combo", "option_combo"];

struct PrivateRead {
    group: &'static str,
    name: &'static str,
    method: &'static str,
    help: &'static str,
    choices: &'static [Choice],
    pagination: bool,
}

struct Choice {
    name: &'static str,
    required: bool,
    values: &'static [&'static str],
}

// Named private reads expose a reviewed subset of their documented parameters.
const PRIVATE_READS: &[PrivateRead] = &[
    PrivateRead {
        group: "orders",
        name: "list",
        method: "private/get_open_orders",
        help: "List open orders across currencies (trade:read)",
        choices: &[Choice {
            name: "kind",
            required: false,
            values: ORDER_KINDS,
        }],
        pagination: false,
    },
    PrivateRead {
        group: "portfolio",
        name: "positions",
        method: "private/get_positions",
        help: "List open positions (trade:read)",
        choices: &[
            Choice {
                name: "currency",
                required: false,
                values: POSITION_CURRENCIES,
            },
            Choice {
                name: "kind",
                required: false,
                values: POSITION_KINDS,
            },
        ],
        pagination: false,
    },
    PrivateRead {
        group: "account",
        name: "summary",
        method: "private/get_account_summary",
        help: "Show the authenticated account summary (account:read)",
        choices: &[Choice {
            name: "currency",
            required: true,
            values: ACCOUNT_CURRENCIES,
        }],
        pagination: false,
    },
    PrivateRead {
        group: "wallet",
        name: "transfers",
        method: "private/get_transfers",
        help: "Show transfer history (wallet:read)",
        choices: &[Choice {
            name: "currency",
            required: true,
            values: WALLET_CURRENCIES,
        }],
        pagination: true,
    },
];

pub(crate) struct Display {
    pub output: OutputFormat,
    pub table_width: Option<u16>,
    pub color: crate::ColorPolicy,
}

pub fn command(registry: &ManifestRegistry) -> Command {
    let mut command = Command::new("deribit-cli")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Deribit CLI for market data, account queries, and WebSocket operations")
        .after_help(
            "Put --env mainnet or --env testnet before every network command.\n\
             Help, completion, and catalog stay local and need neither an environment nor credentials.",
        )
        .subcommand_required(true)
        .arg_required_else_help(true)
        .disable_help_subcommand(true)
        .arg(
            Arg::new("output")
                .long("output")
                .value_parser(["json", "table"])
                .default_value("json")
                .help("Render one response as JSON or a table"),
        )
        .arg(
            Arg::new("table-width")
                .long("table-width")
                .value_parser(clap::value_parser!(u16).range(20..=240))
                .help("Set the table width; requires --output table"),
        )
        .arg(
            Arg::new("color")
                .long("color")
                .value_parser(["auto", "always", "never"])
                .help("Color tables; requires --output table"),
        )
        .arg(
            Arg::new("env")
                .long("env")
                .value_name("mainnet|testnet")
                .value_parser(clap::value_parser!(Environment))
                .help("Select the fixed official Deribit endpoint"),
        )
        .arg(
            Arg::new("timeout-seconds")
                .long("timeout-seconds")
                .value_parser(clap::value_parser!(u64).range(1..=300))
                .default_value("15")
                .help("Deadline per connection attempt and RPC"),
        )
        .arg(switch(
            "authenticate",
            "Use credentials for the selected environment",
        ))
        .arg(
            Arg::new("scope")
                .long("scope")
                .requires("authenticate")
                .help("Requested authentication scope, required on Mainnet"),
        )
        .arg(
            Arg::new("credentials-mount")
                .long("credentials-mount")
                .value_parser(clap::value_parser!(PathBuf))
                .requires("authenticate")
                .help("Absolute path of an existing 1Password .env FIFO mount (Testnet only)"),
        )
        .arg(credential_variable("client-id-variable"))
        .arg(credential_variable("client-secret-variable"))
        .arg(switch("allow-write", "Allow explicitly requested writes"))
        .arg(switch(
            "allow-special",
            "Allow explicitly authorized special actions",
        ))
        .arg(switch(
            "allow-mainnet-write",
            "Allow explicitly requested Mainnet actions",
        ));

    for &(name, help) in NAMED_GROUPS {
        let mut group = command_group(name, help);
        for (operation_group, method, operation) in public_commands(registry) {
            if operation_group != name {
                continue;
            }
            let subcommand = Command::new(method.to_owned())
                .about(format!("Call {}", operation.method()))
                .args(operation.parameters().map(|parameter| {
                    parameter_arg(parameter)
                        .required(parameter.cli_behavior() == CliBehavior::MustSupply)
                }));
            group = group.subcommand(subcommand);
        }
        for read in PRIVATE_READS.iter().filter(|read| read.group == name) {
            group = group.subcommand(read.command());
        }
        command = command.subcommand(group);
    }

    command
        .subcommand(
            command_group("stream", "Subscribe and receive WebSocket events").subcommand(
                Command::new("subscribe")
                    .about("Subscribe to concrete channels and stream JSON lines")
                    .arg(Arg::new("channel").required(true).num_args(1..))
                    .arg(
                        Arg::new("observe-seconds")
                            .long("observe-seconds")
                            .value_parser(clap::value_parser!(u64).range(1..=300))
                            .default_value("10"),
                    ),
            ),
        )
        .subcommand(
            command_group(
                "rpc",
                "Run exact Deribit methods or a sequence on one connection",
            )
            .subcommand(
                Command::new("call")
                    .about("Call one exact JSON-RPC method")
                    .arg(Arg::new("method").required(true))
                    .arg(
                        Arg::new("params-file")
                            .long("params-file")
                            .value_parser(clap::value_parser!(PathBuf))
                            .conflicts_with("params-stdin"),
                    )
                    .arg(
                        Arg::new("params-stdin")
                            .long("params-stdin")
                            .action(ArgAction::SetTrue),
                    ),
            )
            .subcommand(
                Command::new("session")
                    .about("Run a JSON array of RPC and observe steps on one WebSocket")
                    .arg(
                        Arg::new("file")
                            .long("file")
                            .value_parser(clap::value_parser!(PathBuf)),
                    )
                    .arg(
                        Arg::new("continue-read-errors")
                            .long("continue-read-errors")
                            .action(ArgAction::SetTrue)
                            .help("Continue after a server error for independent read-only calls"),
                    ),
            ),
        )
        .subcommand(
            Command::new("catalog")
                .about("Inspect the pinned API and subscription catalog locally")
                .subcommand(
                    Command::new("method")
                        .about("Describe an exact RPC method")
                        .arg(Arg::new("method").required(true)),
                )
                .subcommand(
                    Command::new("channel")
                        .about("Describe an exact WebSocket channel template")
                        .arg(Arg::new("channel").required(true)),
                ),
        )
        .subcommand(
            Command::new("completion")
                .about("Generate a static shell completion script")
                .arg(
                    Arg::new("shell")
                        .required(true)
                        .value_parser(["bash", "zsh", "fish"]),
                ),
        )
}

pub(crate) fn display(matches: &ArgMatches) -> Result<Display, Failure> {
    let output = match matches.get_one::<String>("output").map(String::as_str) {
        Some("json") => OutputFormat::Json,
        Some("table") => OutputFormat::Table,
        _ => unreachable!("output has a parser default"),
    };
    let table_width = matches.get_one::<u16>("table-width").copied();
    let color = match matches.get_one::<String>("color").map(String::as_str) {
        None => crate::ColorPolicy::Auto,
        Some("auto") => crate::ColorPolicy::Auto,
        Some("always") => crate::ColorPolicy::Always,
        Some("never") => crate::ColorPolicy::Never,
        _ => unreachable!("color values are restricted"),
    };
    if output == OutputFormat::Json && (table_width.is_some() || matches.contains_id("color")) {
        return Err(Failure::Usage(
            "--table-width and --color require --output table",
        ));
    }
    Ok(Display {
        output,
        table_width,
        color,
    })
}

pub(crate) fn completion_shell(name: &str) -> CompletionShell {
    match name {
        "bash" => CompletionShell::Bash,
        "zsh" => CompletionShell::Zsh,
        "fish" => CompletionShell::Fish,
        _ => unreachable!("completion shells are restricted"),
    }
}

fn command_group(name: &'static str, help: &'static str) -> Command {
    Command::new(name)
        .about(help)
        .subcommand_required(true)
        .arg_required_else_help(true)
}

fn switch(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .action(ArgAction::SetTrue)
        .help(help)
}

fn credential_variable(name: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .help("Select a supported Testnet credential variable name")
}

fn public_commands(
    registry: &ManifestRegistry,
) -> impl Iterator<Item = (&'static str, &str, ValidatedOperation<'_>)> {
    registry.operations().filter_map(|operation| {
        let group = match operation.method() {
            "public/get_announcements" => "account",
            "public/get_time" | "public/status" | "public/test" => return None,
            _ => "market",
        };
        let name = operation.command_tokens()[1].as_str();
        Some((group, name.strip_prefix("get-").unwrap_or(name), operation))
    })
}

pub(crate) fn named_command<'a>(
    registry: &'a ManifestRegistry,
    method: &str,
) -> Option<[&'a str; 2]> {
    public_commands(registry)
        .find(|(_, _, operation)| operation.method() == method)
        .map(|(group, name, _)| [group, name])
        .or_else(|| {
            PRIVATE_READS
                .iter()
                .find(|read| read.method == method)
                .map(|read| [read.group, read.name])
        })
}

pub(crate) fn friendly_call(
    registry: &ManifestRegistry,
    group: &str,
    options: &ArgMatches,
) -> Result<Option<Call>, Failure> {
    if !NAMED_GROUPS.iter().any(|(name, _)| *name == group) {
        return Ok(None);
    }
    let (name, args) = options.subcommand().expect("named subcommand required");
    if let Some((_, _, operation)) =
        public_commands(registry).find(|(g, n, _)| *g == group && *n == name)
    {
        let typed = operation
            .parameters()
            .filter_map(|parameter| {
                args.get_one::<String>(parameter.name())
                    .map(|value| (parameter.name().to_owned(), value.clone()))
            })
            .collect();
        let params = resolve_parameters(
            operation,
            ParameterInput::Typed(typed),
            &mut io::empty(),
            false,
        )
        .map_err(|_| Failure::Input("invalid parameters for the selected named method"))?;
        let Value::Object(params) = params else {
            unreachable!("the parameter codec returns an object")
        };
        return Ok(Some(Call::new(operation.method(), params)));
    }

    let read = PRIVATE_READS
        .iter()
        .find(|read| read.group == group && read.name == name)
        .expect("named command comes from the command tree");
    Ok(Some(Call::new(read.method, read.params(args))))
}

/// Validates request fields for which this CLI has a reviewed parameter contract.
pub(crate) fn validate_known_parameters(
    registry: &ManifestRegistry,
    call: &Call,
) -> Result<(), Failure> {
    if call.method == "public/auth" {
        return Ok(());
    }
    if let Some(operation) = registry
        .operations()
        .find(|row| row.method() == call.method)
    {
        for parameter in operation.parameters() {
            // The frozen v1 schema may lag the general API's enum values and
            // required fields. For raw calls, check only the known JSON types.
            if let Some(value) = call.params.get(parameter.name()) {
                let matches = match parameter.value_type() {
                    ParameterType::String => value.is_string(),
                    ParameterType::Integer => value.as_i64().is_some() || value.as_u64().is_some(),
                    ParameterType::Number => value.is_number(),
                    ParameterType::Boolean => value.is_boolean(),
                };
                if !(matches || parameter.nullable() && value.is_null()) {
                    return Err(Failure::Input(
                        "wrong JSON type for a documented public parameter",
                    ));
                }
            }
        }
        return Ok(());
    }

    let Some(read) = PRIVATE_READS.iter().find(|read| read.method == call.method) else {
        return Ok(());
    };
    for choice in read.choices {
        match call.params.get(choice.name) {
            Some(Value::String(_)) => {}
            None if !choice.required => {}
            _ => {
                return Err(Failure::Input(
                    "expected a string for a documented private parameter",
                ));
            }
        }
    }
    if read.pagination
        && (call.params.get("count").is_some_and(|value| {
            !value
                .as_u64()
                .is_some_and(|count| (1..=1000).contains(&count))
        }) || call
            .params
            .get("offset")
            .is_some_and(|value| value.as_u64().is_none()))
    {
        return Err(Failure::Input(
            "wallet count must be between 1 and 1000 and offset must be a non-negative integer",
        ));
    }
    Ok(())
}

pub(crate) fn completion_words(registry: &ManifestRegistry) -> Vec<String> {
    fn append(command: &Command, words: &mut Vec<String>) {
        words.extend(
            command
                .get_arguments()
                .filter_map(|arg| arg.get_long())
                .filter(|name| !matches!(*name, "help" | "version"))
                .map(|name| format!("--{name}")),
        );
        for child in command.get_subcommands() {
            words.push(child.get_name().to_owned());
            append(child, words);
        }
    }
    let mut words = Vec::new();
    append(&command(registry), &mut words);
    words
}

impl PrivateRead {
    fn command(&self) -> Command {
        let mut command = Command::new(self.name).about(self.help);
        for choice in self.choices {
            command = command.arg(
                Arg::new(choice.name)
                    .long(choice.name)
                    .required(choice.required)
                    .value_parser(PossibleValuesParser::new(choice.values.iter().copied())),
            );
        }
        if self.pagination {
            command = command
                .arg(
                    Arg::new("count")
                        .long("count")
                        .value_parser(clap::value_parser!(u64).range(1..=1000)),
                )
                .arg(
                    Arg::new("offset")
                        .long("offset")
                        .value_parser(clap::value_parser!(u64)),
                );
        }
        command
    }

    fn params(&self, args: &ArgMatches) -> Map<String, Value> {
        let mut params: Map<_, _> = self
            .choices
            .iter()
            .filter_map(|choice| {
                args.get_one::<String>(choice.name)
                    .map(|value| (choice.name.to_owned(), Value::String(value.clone())))
            })
            .collect();
        if self.pagination {
            for name in ["count", "offset"] {
                if let Some(value) = args.get_one::<u64>(name) {
                    params.insert(name.to_owned(), Value::from(*value));
                }
            }
        }
        params
    }
}
