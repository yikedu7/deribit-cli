//! Manifest-backed clap command tree.

use clap::{
    Arg, ArgAction, ArgGroup, Command, ValueHint,
    builder::{PossibleValuesParser, ValueParser},
};

use crate::{
    CliBehavior, CoreError, DefaultKind, ManifestRegistry, ParameterDescriptor, ParameterType,
};

const OUTPUT_VALUES: [&str; 2] = ["json", "table"];
const COLOR_VALUES: [&str; 3] = ["auto", "always", "never"];
const ENVIRONMENT_VALUES: [&str; 2] = ["mainnet", "testnet"];
const COMPLETION_SHELLS: [&str; 3] = ["bash", "zsh", "fish"];

/// Builds the complete v1 syntax tree from the validated embedded manifest.
pub fn build_cli(registry: &ManifestRegistry) -> Result<Command, CoreError> {
    let mut public = Command::new("public")
        .about("Call one frozen public read-only Deribit method")
        .subcommand_required(true)
        .arg_required_else_help(true);

    for operation in registry.operations() {
        let tokens = operation.command_tokens();
        if tokens.len() != 2 || tokens[0] != "public" {
            return Err(CoreError::CommandCorrespondence(format!(
                "{} is not a two-token public command",
                operation.canonical_command()
            )));
        }

        let mut method = Command::new(tokens[1].clone())
            .about(format!("Invoke {}", operation.method()))
            .disable_version_flag(true);
        for parameter in operation.parameters() {
            method = method.arg(parameter_arg(parameter));
        }
        public = public.subcommand(method);
    }

    Ok(Command::new("deribit-cli")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Read-only Deribit public market-data CLI")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .disable_help_subcommand(true)
        .arg(
            Arg::new("output")
                .long("output")
                .value_name("json|table")
                .value_parser(OUTPUT_VALUES)
                .default_value("json")
                .action(ArgAction::Set),
        )
        .arg(
            Arg::new("table-width")
                .long("table-width")
                .value_name("20..240")
                .value_parser(clap::value_parser!(u16).range(20..=240))
                .action(ArgAction::Set),
        )
        .arg(
            Arg::new("color")
                .long("color")
                .value_name("auto|always|never")
                .value_parser(COLOR_VALUES)
                .action(ArgAction::Set),
        )
        .arg(
            Arg::new("env")
                .long("env")
                .value_name("mainnet|testnet")
                .value_parser(ENVIRONMENT_VALUES)
                .default_value("mainnet")
                .action(ArgAction::Set),
        )
        .arg(
            Arg::new("timeout")
                .long("timeout")
                .value_name("1..120000ms|1..120s")
                .default_value("10s")
                .action(ArgAction::Set),
        )
        .arg(
            Arg::new("params")
                .long("params")
                .value_name("JSON_OBJECT")
                .action(ArgAction::Set),
        )
        .arg(
            Arg::new("params-file")
                .long("params-file")
                .value_name("PATH")
                .value_hint(ValueHint::FilePath)
                .action(ArgAction::Set),
        )
        .arg(
            Arg::new("params-stdin")
                .long("params-stdin")
                .action(ArgAction::SetTrue),
        )
        .group(
            ArgGroup::new("parameter-source")
                .args(["params", "params-file", "params-stdin"])
                .multiple(false),
        )
        .subcommand(public)
        .subcommand(Command::new("coverage").about("Show frozen method coverage metadata"))
        .subcommand(Command::new("version").about("Show local version metadata"))
        .subcommand(
            Command::new("completion")
                .about("Generate a static shell completion script")
                .arg(
                    Arg::new("shell")
                        .required(true)
                        .value_parser(COMPLETION_SHELLS)
                        .action(ArgAction::Set),
                ),
        ))
}

fn parameter_arg(parameter: ParameterDescriptor<'_>) -> Arg {
    let long = parameter.flag().trim_start_matches("--").to_owned();
    let value_parser = parameter_value_parser(parameter);
    Arg::new(parameter.name().to_owned())
        .long(long)
        .value_name(parameter.value_type().label())
        .help(parameter_help(parameter))
        .required(false)
        .num_args(1)
        .action(ArgAction::Set)
        .value_parser(value_parser)
}

fn parameter_value_parser(parameter: ParameterDescriptor<'_>) -> ValueParser {
    let values = if parameter.value_type() == ParameterType::Boolean {
        vec!["true".to_owned(), "false".to_owned()]
    } else {
        parameter
            .enum_values()
            .iter()
            .map(|value| match value {
                serde_json::Value::String(value) => value.clone(),
                value => value.to_string(),
            })
            .collect::<Vec<_>>()
    };
    if values.is_empty() {
        ValueParser::string()
    } else {
        PossibleValuesParser::new(values).into()
    }
}

fn parameter_help(parameter: ParameterDescriptor<'_>) -> String {
    let requirement = match parameter.cli_behavior() {
        CliBehavior::MustSupply => "required",
        CliBehavior::OmitWhenAbsent => "optional; omitted when absent",
    };
    let mut details = vec![
        requirement.to_owned(),
        format!("type={}", parameter.value_type().label()),
        format!("unit={}", parameter.unit()),
    ];
    if !parameter.enum_values().is_empty() {
        details.push(format!(
            "enum={}",
            parameter
                .enum_values()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("|")
        ));
    }
    match parameter.default_kind() {
        DefaultKind::NotDeclared => details.push("default=not declared".to_owned()),
        DefaultKind::Literal => details.push(format!(
            "upstream default={}",
            parameter
                .literal_default()
                .map(ToString::to_string)
                .unwrap_or_else(|| "invalid manifest".to_owned())
        )),
        DefaultKind::Dynamic => details.push(format!(
            "upstream dynamic default={}",
            parameter
                .dynamic_default_description()
                .unwrap_or("documented by upstream")
        )),
    }
    details.join("; ")
}

trait ParameterTypeLabel {
    fn label(self) -> &'static str;
}

impl ParameterTypeLabel for ParameterType {
    fn label(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::Boolean => "true|false",
        }
    }
}
