//! One-operation application runner and process stream/exit handoff.

use std::{
    cmp,
    ffi::OsString,
    io::{self, IsTerminal, Read, Write},
};

use serde::Serialize;
use serde_json::{Map, Value};
use thiserror::Error;

use crate::{
    CliCommand, CliError, CompletionShell, ConnectTimeout, CoreError, CoverageMetadata, Deadline,
    ErrorClass, ManifestRegistry, OperationTimeout, OutputFormat, ParsedCli, PresentationContext,
    PresentationError, ProxySettings, PublicInvocation, RequestId, ResponseBodyLimit, TableProfile,
    native::NativeResponse,
    reqwest_transport::ReqwestTransport,
    rpc::{RpcError, RpcRequest},
    transport::{Transport, TransportError},
};

/// Frozen process exit codes from command-syntax v1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum ProcessExit {
    Success = 0,
    Usage = 2,
    Input = 3,
    ApiNative = 4,
    Transport = 5,
    LocalIo = 6,
    Internal = 70,
    Timeout = 124,
}

impl ProcessExit {
    /// Maps an internal error classification to the public process contract.
    pub const fn from_class(class: ErrorClass) -> Self {
        match class {
            ErrorClass::Usage => Self::Usage,
            ErrorClass::Input => Self::Input,
            ErrorClass::Transport => Self::Transport,
            ErrorClass::LocalIo => Self::LocalIo,
            ErrorClass::ApiNative => Self::ApiNative,
            ErrorClass::Timeout => Self::Timeout,
            ErrorClass::Internal => Self::Internal,
        }
    }

    /// Returns the operating-system exit value.
    pub const fn code(self) -> i32 {
        self as i32
    }
}

/// Fully buffered process output, keeping machine output and diagnostics apart.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit: ProcessExit,
}

impl ProcessOutput {
    fn new(stdout: Vec<u8>, stderr: Vec<u8>, exit: ProcessExit) -> Self {
        Self {
            stdout,
            stderr,
            exit,
        }
    }

    /// Bytes destined only for standard output.
    pub fn stdout(&self) -> &[u8] {
        &self.stdout
    }

    /// Bytes destined only for standard error.
    pub fn stderr(&self) -> &[u8] {
        &self.stderr
    }

    /// Frozen exit classification for this outcome.
    pub const fn exit(&self) -> ProcessExit {
        self.exit
    }

    /// Writes each stream exactly once. A write failure is a local-I/O failure.
    pub fn write_to(
        &self,
        stdout: &mut dyn Write,
        stderr: &mut dyn Write,
    ) -> Result<(), io::Error> {
        stdout.write_all(&self.stdout)?;
        stderr.write_all(&self.stderr)?;
        Ok(())
    }
}

/// Failures after CLI parsing and before a complete native response exists.
#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error(transparent)]
    Rpc(#[from] RpcError),
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error(transparent)]
    Presentation(#[from] PresentationError),
}

impl ApplicationError {
    /// Returns the user-facing process classification.
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Core(error) => error.class(),
            Self::Transport(error) => error.class(),
            Self::Presentation(_) => ErrorClass::Internal,
            Self::Rpc(RpcError::Configuration(error)) => error.class(),
            Self::Rpc(RpcError::InvalidJsonResponse)
            | Self::Rpc(RpcError::HttpStatusWithoutApiError { .. })
            | Self::Rpc(RpcError::Protocol(_)) => ErrorClass::Transport,
            Self::Rpc(RpcError::Internal(_)) => ErrorClass::Internal,
        }
    }
}

/// Executes exactly one already-parsed public operation through the supplied port.
pub fn execute_public<T: Transport>(
    registry: &ManifestRegistry,
    invocation: &PublicInvocation,
    configuration: &crate::ClientConfiguration,
    transport: &T,
) -> Result<NativeResponse, ApplicationError> {
    let operation = registry.operation(&invocation.operation_id)?;
    if operation.method() != invocation.method {
        return Err(CoreError::Internal(
            "parsed operation method no longer matches the embedded registry".into(),
        )
        .into());
    }
    if configuration.environment() != invocation.environment
        || configuration.operation_timeout().duration() != invocation.timeout
    {
        return Err(CoreError::Internal(
            "application configuration does not match the parsed invocation".into(),
        )
        .into());
    }
    let params = invocation.params.as_object().cloned().ok_or_else(|| {
        CoreError::Internal("validated public parameters are not a JSON object".into())
    })?;
    execute_rpc(operation, params, configuration, transport)
}

fn execute_rpc<T: Transport>(
    operation: crate::ValidatedOperation<'_>,
    params: Map<String, Value>,
    configuration: &crate::ClientConfiguration,
    transport: &T,
) -> Result<NativeResponse, ApplicationError> {
    let request = RpcRequest::new(operation, RequestId::new_v4(), params)?;
    let transport_request = request.to_transport_request(configuration)?;
    let deadline = Deadline::from_now(configuration.operation_timeout());
    let response = transport.execute(&transport_request, deadline)?;
    Ok(NativeResponse::from_rpc(request.parse_response(response)?))
}

/// Parses argv using the embedded registry and runs the product adapter.
pub fn run_process<I, T>(args: I, stdin: &mut dyn Read, stdin_is_terminal: bool) -> ProcessOutput
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let registry = match ManifestRegistry::embedded() {
        Ok(registry) => registry,
        Err(error) => return render_application_error(error.into()),
    };
    let parsed = match crate::parse_from_with_stdin(&registry, args, stdin, stdin_is_terminal) {
        Ok(parsed) => parsed,
        Err(error) => return render_cli_error(error),
    };
    run_parsed_product(&registry, parsed)
}

fn run_parsed_product(registry: &ManifestRegistry, parsed: ParsedCli) -> ProcessOutput {
    match &parsed.command {
        CliCommand::Public(invocation) => {
            let configuration = match configuration_for(invocation) {
                Ok(configuration) => configuration,
                Err(error) => return render_application_error(error),
            };
            let transport = ReqwestTransport::new(configuration.clone());
            let result = execute_public(registry, invocation, &configuration, &transport);
            if parsed.global.output == OutputFormat::Json {
                render_public_result(result)
            } else {
                render_table_result(
                    result,
                    match public_presentation_context(registry, invocation, parsed.global) {
                        Ok(context) => context,
                        Err(error) => return render_application_error(error),
                    },
                )
            }
        }
        _ => render_local_command(registry, &parsed),
    }
}

/// Runs one parsed command with a deterministic transport for contract tests.
///
/// Local commands ignore the port. Public commands still enforce the same
/// immutable configuration and stream/exit handoff as the product adapter.
pub fn run_parsed_with_transport<T: Transport>(
    registry: &ManifestRegistry,
    parsed: &ParsedCli,
    configuration: &crate::ClientConfiguration,
    transport: &T,
) -> ProcessOutput {
    match &parsed.command {
        CliCommand::Public(invocation) => {
            let result = execute_public(registry, invocation, configuration, transport);
            if parsed.global.output == OutputFormat::Json {
                render_public_result(result)
            } else {
                render_table_result(
                    result,
                    match public_presentation_context(registry, invocation, parsed.global) {
                        Ok(context) => context,
                        Err(error) => return render_application_error(error),
                    },
                )
            }
        }
        _ => render_local_command(registry, parsed),
    }
}

fn configuration_for(
    invocation: &PublicInvocation,
) -> Result<crate::ClientConfiguration, ApplicationError> {
    let operation_timeout = OperationTimeout::new(invocation.timeout)?;
    let connect_duration = cmp::min(
        ConnectTimeout::DEFAULT.duration(),
        operation_timeout.duration(),
    );
    let connect_timeout = ConnectTimeout::new(connect_duration)?;
    let proxies = ProxySettings::from_process_environment()?;
    Ok(crate::ClientConfiguration::new(
        invocation.environment,
        operation_timeout,
        connect_timeout,
        ResponseBodyLimit::DEFAULT,
        proxies,
    )?)
}

/// Converts a native/application result into the frozen stdout/stderr/exit tuple.
pub fn render_public_result(result: Result<NativeResponse, ApplicationError>) -> ProcessOutput {
    match result {
        Ok(native) => {
            let exit = if native.is_api_error() {
                ProcessExit::ApiNative
            } else {
                ProcessExit::Success
            };
            match json_line(native.value()) {
                Ok(stdout) => ProcessOutput::new(stdout, Vec::new(), exit),
                Err(error) => render_application_error(error),
            }
        }
        Err(error) => render_application_error(error),
    }
}

/// Converts a native/application result into the frozen table-or-native-error tuple.
pub fn render_table_result(
    result: Result<NativeResponse, ApplicationError>,
    context: PresentationContext,
) -> ProcessOutput {
    match result {
        Ok(native) if native.is_api_error() => match json_line(native.value()) {
            Ok(stdout) => ProcessOutput::new(
                stdout,
                api_native_diagnostic(&native),
                ProcessExit::ApiNative,
            ),
            Err(error) => render_application_error(error),
        },
        Ok(native) => match crate::presentation::render_native(&native, &context) {
            Ok(table) => ProcessOutput::new(table.into_bytes(), Vec::new(), ProcessExit::Success),
            Err(error) => render_application_error(error.into()),
        },
        Err(error) => render_application_error(error),
    }
}

fn api_native_diagnostic(native: &NativeResponse) -> Vec<u8> {
    let code = native
        .value()
        .pointer("/error/code")
        .map(Value::to_string)
        .unwrap_or_else(|| "unknown".into());
    diagnostic(
        ErrorClass::ApiNative,
        &format!("upstream returned JSON-RPC error code={code}"),
    )
}

fn public_presentation_context(
    registry: &ManifestRegistry,
    invocation: &PublicInvocation,
    global: crate::GlobalOptions,
) -> Result<PresentationContext, ApplicationError> {
    let operation = registry.operation(&invocation.operation_id)?;
    let profile = TableProfile::from_manifest(operation.table_profile_id()).ok_or_else(|| {
        CoreError::Internal(format!(
            "operation {} references an unknown table profile",
            operation.operation_id()
        ))
    })?;
    Ok(PresentationContext::from_process(
        operation.command_tokens()[1].clone(),
        profile,
        Some(invocation.environment.as_str()),
        global,
        io::stdout().is_terminal(),
    ))
}

fn render_local_command(registry: &ManifestRegistry, parsed: &ParsedCli) -> ProcessOutput {
    match parsed.command {
        CliCommand::Coverage => render_local_metadata(
            "coverage",
            CoverageMetadata::from_registry(registry),
            parsed,
        ),
        CliCommand::Version => {
            render_local_metadata("version", VersionMetadata::new(registry), parsed)
        }
        CliCommand::Completion(shell) => ProcessOutput::new(
            completion_script(registry, shell).into_bytes(),
            Vec::new(),
            ProcessExit::Success,
        ),
        CliCommand::Public(_) => unreachable!("public commands are handled by the runner"),
    }
}

fn render_local_metadata<T: Serialize>(
    operation: &str,
    metadata: T,
    parsed: &ParsedCli,
) -> ProcessOutput {
    let value = match serde_json::to_value(metadata) {
        Ok(value) => value,
        Err(error) => {
            return render_application_error(
                CoreError::Internal(format!("local metadata serialization failed: {error}")).into(),
            );
        }
    };
    if parsed.global.output == OutputFormat::Json {
        match json_line(&value) {
            Ok(stdout) => ProcessOutput::new(stdout, Vec::new(), ProcessExit::Success),
            Err(error) => render_application_error(error),
        }
    } else {
        let context = PresentationContext::from_process(
            operation,
            TableProfile::Generic,
            None::<String>,
            parsed.global,
            io::stdout().is_terminal(),
        );
        ProcessOutput::new(
            crate::presentation::render_value(&value, &context).into_bytes(),
            Vec::new(),
            ProcessExit::Success,
        )
    }
}

fn render_cli_error(error: CliError) -> ProcessOutput {
    match error.class() {
        None => ProcessOutput::new(
            error.to_string().into_bytes(),
            Vec::new(),
            ProcessExit::Success,
        ),
        Some(class) => ProcessOutput::new(
            Vec::new(),
            diagnostic(class, &error.to_string()),
            ProcessExit::from_class(class),
        ),
    }
}

fn render_application_error(error: ApplicationError) -> ProcessOutput {
    let class = error.class();
    ProcessOutput::new(
        Vec::new(),
        diagnostic(class, &error.to_string()),
        ProcessExit::from_class(class),
    )
}

fn diagnostic(class: ErrorClass, message: &str) -> Vec<u8> {
    format!("{}: {}\n", class.as_str(), message.trim_end()).into_bytes()
}

fn json_line<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, ApplicationError> {
    let mut bytes = serde_json::to_vec(value)
        .map_err(|error| CoreError::Internal(format!("JSON output failed: {error}")))?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[derive(Serialize)]
struct VersionMetadata<'a> {
    tool_version: &'static str,
    api_snapshot: &'a str,
    protocol: &'static str,
    endpoints: VersionEndpoints,
}

#[derive(Serialize)]
struct VersionEndpoints {
    mainnet: &'static str,
    testnet: &'static str,
}

impl<'a> VersionMetadata<'a> {
    fn new(registry: &'a ManifestRegistry) -> Self {
        Self {
            tool_version: env!("CARGO_PKG_VERSION"),
            api_snapshot: registry.snapshot_id(),
            protocol: "json-rpc-2.0",
            endpoints: VersionEndpoints {
                mainnet: "https://www.deribit.com/api/v2",
                testnet: "https://test.deribit.com/api/v2",
            },
        }
    }
}

fn completion_script(registry: &ManifestRegistry, shell: CompletionShell) -> String {
    let words = completion_words(registry).join(" ");
    match shell {
        CompletionShell::Bash => format!(
            "_deribit_cli_complete() {{\n  COMPREPLY=( $(compgen -W '{words}' -- \"${{COMP_WORDS[COMP_CWORD]}}\") )\n}}\ncomplete -F _deribit_cli_complete deribit-cli\n"
        ),
        CompletionShell::Zsh => {
            format!("#compdef deribit-cli\n_arguments '*:deribit-cli:(({words}))'\n")
        }
        CompletionShell::Fish => {
            format!("complete -c deribit-cli -f -a '{words}'\n")
        }
    }
}

fn completion_words(registry: &ManifestRegistry) -> Vec<String> {
    let mut words = vec![
        "public".into(),
        "coverage".into(),
        "version".into(),
        "completion".into(),
        "bash".into(),
        "zsh".into(),
        "fish".into(),
        "--output".into(),
        "--table-width".into(),
        "--color".into(),
        "--env".into(),
        "--timeout".into(),
        "--params".into(),
        "--params-file".into(),
        "--params-stdin".into(),
        "--help".into(),
        "--version".into(),
        "json".into(),
        "table".into(),
        "mainnet".into(),
        "testnet".into(),
    ];
    for operation in registry.operations() {
        if let Some(method_command) = operation.command_tokens().get(1) {
            words.push(method_command.clone());
        }
        for parameter in operation.parameters() {
            words.push(parameter.flag().to_owned());
            for value in parameter.enum_values() {
                words.push(match value {
                    Value::String(value) => value.clone(),
                    value => value.to_string(),
                });
            }
        }
    }
    words.sort();
    words.dedup();
    words
}

/// Helper for the process entry point to classify stream-write failures.
pub fn local_io_failure(error: &io::Error) -> ProcessOutput {
    ProcessOutput::new(
        Vec::new(),
        diagnostic(ErrorClass::LocalIo, &error.to_string()),
        ProcessExit::LocalIo,
    )
}
