//! Core contracts for the frozen Deribit public-read v1 snapshot.
//!
//! The signed S2 modules preserve the pure-core boundary: immutable manifest
//! registry, policy validation, configuration, runtime limits, error
//! classification, and coverage metadata. The S4 transport/RPC modules consume
//! that boundary without changing its semantics or accepting a user-supplied
//! manifest.

#![forbid(unsafe_code)]

pub mod application;
pub mod cli;
pub mod commands;
pub mod configuration;
pub mod coverage;
pub mod errors;
pub mod manifest;
pub mod manifest_validator;
pub mod native;
pub mod parameters;
pub mod presentation;
pub mod reqwest_transport;
pub mod rpc;
pub mod runtime;
pub mod table_profiles;
pub mod transport;

pub use application::{
    ApplicationError, ProcessExit, ProcessOutput, execute_public, render_public_result,
    render_table_result, run_parsed_with_transport, run_process,
};
pub use cli::{
    CliCommand, CliError, ColorPolicy, CompletionShell, GlobalOptions, OutputFormat, ParsedCli,
    PublicInvocation, parse_from, parse_from_with_stdin,
};
pub use commands::build_cli;
pub use configuration::{
    ClientConfiguration, ConnectTimeout, Environment, OperationTimeout, ProxyEnvironment,
    ProxySettings, ResponseBodyLimit, TlsPolicy,
};
pub use coverage::{CoverageMetadata, CoverageMethod};
pub use errors::{CoreError, ErrorClass};
pub use manifest::{
    CliBehavior, DefaultKind, EMBEDDED_MANIFEST_SHA256, EMBEDDED_SNAPSHOT_ID,
    EMBEDDED_SNAPSHOT_SHA256, ManifestRegistry, ParameterDescriptor, ParameterType,
    ValidatedOperation,
};
pub use manifest_validator::{
    CommandCorrespondence, validate_command_correspondence, validate_operation_binding,
};
pub use native::{NativeKind, NativeResponse};
pub use parameters::{
    MAX_PARAMETER_BYTES, MAX_PARAMETER_DEPTH, ParameterError, ParameterInput, resolve_parameters,
};
pub use presentation::{
    PresentationContext, PresentationError, TableOutput, display_width, render_native, render_value,
};
pub use runtime::{Deadline, RequestId, ResponseByteBudget};
pub use table_profiles::{FieldKind, TableProfile};
