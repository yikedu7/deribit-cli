//! Pure CLI adapter from argv plus an explicit input stream to one operation.

use std::{
    collections::HashMap,
    ffi::OsString,
    io::{self, IsTerminal, Read},
    path::PathBuf,
    time::Duration,
};

use clap::{ArgMatches, error::ErrorKind};
use serde_json::Value;
use thiserror::Error;

use crate::{
    CoreError, ErrorClass, ManifestRegistry, ParameterError, ParameterInput, build_cli,
    parameters::resolve_parameters,
};

/// Output selection parsed before the command path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputFormat {
    Json,
    Table,
}

/// Color policy parsed before the command path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorPolicy {
    Auto,
    Always,
    Never,
}

/// Fully validated global display settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GlobalOptions {
    pub output: OutputFormat,
    pub table_width: Option<u16>,
    pub color: ColorPolicy,
}

/// Supported completion targets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

/// One transport-free public operation ready for S6 orchestration.
#[derive(Clone, Debug, PartialEq)]
pub struct PublicInvocation {
    pub operation_id: String,
    pub method: String,
    pub environment: crate::Environment,
    pub timeout: Duration,
    pub params: Value,
}

/// The parsed v1 command without any network side effect.
#[derive(Clone, Debug, PartialEq)]
pub enum CliCommand {
    Public(PublicInvocation),
    Coverage,
    Version,
    Completion(CompletionShell),
}

/// A complete transport-free parse result.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedCli {
    pub global: GlobalOptions,
    pub command: CliCommand,
}

/// Parser, input, local-I/O, or frozen-core failure.
#[derive(Debug, Error)]
pub enum CliError {
    #[error(transparent)]
    Parser(#[from] clap::Error),
    #[error(transparent)]
    Parameters(#[from] ParameterError),
    #[error(transparent)]
    Core(#[from] CoreError),
}

impl CliError {
    /// Returns None for successful clap display control-flow (`--help`/`--version`).
    pub fn class(&self) -> Option<ErrorClass> {
        match self {
            Self::Parser(error)
                if matches!(
                    error.kind(),
                    ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
                ) =>
            {
                None
            }
            Self::Parser(_) => Some(ErrorClass::Usage),
            Self::Parameters(error) => Some(error.class()),
            Self::Core(error) => Some(error.class()),
        }
    }
}

/// Parses process argv and reads stdin only when `--params-stdin` was selected.
pub fn parse_from<I, T>(registry: &ManifestRegistry, args: I) -> Result<ParsedCli, CliError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let stdin = io::stdin();
    let terminal = stdin.is_terminal();
    parse_from_with_stdin(registry, args, &mut stdin.lock(), terminal)
}

/// Testable parser entry with explicit stdin bytes and terminal state.
pub fn parse_from_with_stdin<I, T>(
    registry: &ManifestRegistry,
    args: I,
    stdin: &mut dyn Read,
    stdin_is_terminal: bool,
) -> Result<ParsedCli, CliError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let command = build_cli(registry)?;
    let mut argv = args.into_iter().map(Into::into).collect::<Vec<OsString>>();
    validate_help_and_version_shape(&command, &argv)?;
    normalize_separator(&command, &mut argv)?;
    let matches = command.clone().try_get_matches_from(argv)?;
    let global = parse_display_options(&command, &matches)?;
    let raw_source = parameter_source(&matches);

    let cli_command = match matches.subcommand() {
        Some(("public", public)) => {
            let (command_name, method_matches) = public.subcommand().ok_or_else(|| {
                usage_error(&command, "a canonical public method command is required")
            })?;
            let operation = registry.canonical_command(&format!("public {command_name}"))?;
            let typed = operation
                .parameters()
                .filter_map(|parameter| {
                    method_matches
                        .get_one::<String>(parameter.name())
                        .map(|value| (parameter.name().to_owned(), value.clone()))
                })
                .collect::<HashMap<_, _>>();
            if raw_source.is_some() && !typed.is_empty() {
                return Err(usage_error(
                    &command,
                    "raw JSON parameter sources cannot be combined with method parameter flags",
                ));
            }
            let input = raw_source.unwrap_or(ParameterInput::Typed(typed));
            let params = resolve_parameters(operation, input, stdin, stdin_is_terminal)?;
            CliCommand::Public(PublicInvocation {
                operation_id: operation.operation_id().to_owned(),
                method: operation.method().to_owned(),
                environment: parse_environment(&matches),
                timeout: parse_timeout(
                    matches
                        .get_one::<String>("timeout")
                        .expect("clap default supplies timeout"),
                )
                .map_err(|message| usage_error(&command, &message))?,
                params,
            })
        }
        Some(("coverage", _)) => {
            reject_method_only_options(&command, &matches, "coverage")?;
            CliCommand::Coverage
        }
        Some(("version", _)) => {
            reject_method_only_options(&command, &matches, "version")?;
            CliCommand::Version
        }
        Some(("completion", completion)) => {
            reject_all_global_options(&command, &matches, "completion")?;
            let shell = match completion
                .get_one::<String>("shell")
                .expect("clap requires shell")
                .as_str()
            {
                "bash" => CompletionShell::Bash,
                "zsh" => CompletionShell::Zsh,
                "fish" => CompletionShell::Fish,
                _ => unreachable!("clap restricts completion shells"),
            };
            CliCommand::Completion(shell)
        }
        _ => return Err(usage_error(&command, "a command is required")),
    };

    Ok(ParsedCli {
        global,
        command: cli_command,
    })
}

fn validate_help_and_version_shape(
    command: &clap::Command,
    argv: &[OsString],
) -> Result<(), CliError> {
    let help_positions = argv
        .iter()
        .enumerate()
        .filter_map(|(index, token)| (token == "--help").then_some(index))
        .collect::<Vec<_>>();
    if !help_positions.is_empty() {
        let legal = help_positions.len() == 1
            && match help_positions[0] {
                1 => argv.len() == 2,
                2 => argv.len() == 3 && argv.get(1).is_some_and(|token| token == "public"),
                3 => argv.len() == 4 && argv.get(1).is_some_and(|token| token == "public"),
                _ => false,
            };
        if !legal {
            return Err(usage_error(
                command,
                "--help must be the only option at root, public, or one known method",
            ));
        }
    }

    if let Some(position) = argv.iter().position(|token| token == "--version") {
        if position != 1 || argv.len() != 2 {
            return Err(usage_error(
                command,
                "--version is a root-only single-token invocation",
            ));
        }
    }
    Ok(())
}

fn normalize_separator(command: &clap::Command, argv: &mut Vec<OsString>) -> Result<(), CliError> {
    let positions = argv
        .iter()
        .enumerate()
        .filter_map(|(index, token)| (token == "--").then_some(index))
        .collect::<Vec<_>>();
    if positions.len() > 1 {
        return Err(usage_error(command, "-- may appear at most once"));
    }
    let Some(index) = positions.first().copied() else {
        return Ok(());
    };
    let starts_command = argv.get(index + 1).is_some_and(|token| {
        matches!(
            token.to_str(),
            Some("public" | "coverage" | "version" | "completion")
        )
    });
    let ends_public_method = index + 1 == argv.len()
        && argv
            .iter()
            .position(|token| token == "public")
            .is_some_and(|public| public + 1 < index);
    if starts_command || ends_public_method {
        argv.remove(index);
    }
    Ok(())
}

fn parse_display_options(
    command: &clap::Command,
    matches: &ArgMatches,
) -> Result<GlobalOptions, CliError> {
    let output = match matches
        .get_one::<String>("output")
        .expect("clap default supplies output")
        .as_str()
    {
        "json" => OutputFormat::Json,
        "table" => OutputFormat::Table,
        _ => unreachable!("clap restricts output"),
    };
    let table_width = matches.get_one::<u16>("table-width").copied();
    let color = match matches
        .get_one::<String>("color")
        .map(String::as_str)
        .unwrap_or("auto")
    {
        "auto" => ColorPolicy::Auto,
        "always" => ColorPolicy::Always,
        "never" => ColorPolicy::Never,
        _ => unreachable!("clap restricts color"),
    };
    if output == OutputFormat::Json && (table_width.is_some() || matches.contains_id("color")) {
        return Err(usage_error(
            command,
            "--table-width and --color require --output table",
        ));
    }
    Ok(GlobalOptions {
        output,
        table_width,
        color,
    })
}

fn parameter_source(matches: &ArgMatches) -> Option<ParameterInput> {
    if let Some(text) = matches.get_one::<String>("params") {
        Some(ParameterInput::Inline(text.clone()))
    } else if let Some(path) = matches.get_one::<String>("params-file") {
        Some(ParameterInput::File(PathBuf::from(path)))
    } else if matches.get_flag("params-stdin") {
        Some(ParameterInput::Stdin)
    } else {
        None
    }
}

fn parse_environment(matches: &ArgMatches) -> crate::Environment {
    match matches
        .get_one::<String>("env")
        .expect("clap default supplies environment")
        .as_str()
    {
        "mainnet" => crate::Environment::Mainnet,
        "testnet" => crate::Environment::Testnet,
        _ => unreachable!("clap restricts environment"),
    }
}

fn parse_timeout(value: &str) -> Result<Duration, String> {
    let (digits, multiplier, maximum) = if let Some(digits) = value.strip_suffix("ms") {
        (digits, 1_u64, 120_000_u64)
    } else if let Some(digits) = value.strip_suffix('s') {
        (digits, 1_000_u64, 120_u64)
    } else {
        return Err("--timeout must end in ms or s".into());
    };
    if digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
    {
        return Err("--timeout requires a canonical positive integer".into());
    }
    let amount = digits
        .parse::<u64>()
        .map_err(|_| "--timeout is out of range".to_owned())?;
    if amount == 0 || amount > maximum {
        return Err("--timeout is outside the frozen range".into());
    }
    Ok(Duration::from_millis(amount * multiplier))
}

fn reject_method_only_options(
    command: &clap::Command,
    matches: &ArgMatches,
    local_command: &str,
) -> Result<(), CliError> {
    for id in ["env", "timeout", "params", "params-file", "params-stdin"] {
        if matches.value_source(id) == Some(clap::parser::ValueSource::CommandLine) {
            return Err(usage_error(
                command,
                &format!("--{id} is not valid with {local_command}"),
            ));
        }
    }
    Ok(())
}

fn reject_all_global_options(
    command: &clap::Command,
    matches: &ArgMatches,
    local_command: &str,
) -> Result<(), CliError> {
    for id in [
        "output",
        "table-width",
        "color",
        "env",
        "timeout",
        "params",
        "params-file",
        "params-stdin",
    ] {
        if matches.value_source(id) == Some(clap::parser::ValueSource::CommandLine) {
            return Err(usage_error(
                command,
                &format!("--{id} is not valid with {local_command}"),
            ));
        }
    }
    Ok(())
}

fn usage_error(command: &clap::Command, message: &str) -> CliError {
    CliError::Parser(command.clone().error(ErrorKind::ArgumentConflict, message))
}
