//! Immutable runtime registry for the frozen public-read method manifest.
//!
//! The only construction path is the JSON document compiled into this crate.
//! No public constructor accepts a caller-provided manifest, so a CLI adapter
//! cannot use this module to widen the allowed method set.

use std::collections::HashSet;

use serde::Deserialize;
use serde_json::{Number, Value};
use url::Url;

use crate::errors::CoreError;

/// Canonical SHA-256 fixed by the accepted manifest verifier.
pub const EMBEDDED_MANIFEST_SHA256: &str =
    "8c9f6300527e65a102f3c25b90dea4cf0bd07b45c5c3f2cd28a1ce00d3f6685a";
/// Frozen snapshot SHA-256 fixed by the accepted manifest verifier.
pub const EMBEDDED_SNAPSHOT_SHA256: &str =
    "421e1ca06f18d2ad7882aaf4369763a58c3a9629dee62f2173cef57f0c855dc0";
/// Frozen snapshot identifier used by coverage metadata.
pub const EMBEDDED_SNAPSHOT_ID: &str = "deribit-api-v2-mainnet-2026-08-30";

const EMBEDDED_MANIFEST: &str = include_str!("../assets/api-method-manifest-v1.json");
const EXPECTED_MANIFEST_ID: &str = "api-method-manifest-v1";
const EXPECTED_SCHEMA_VERSION: &str = "1.0.0";
const EXPECTED_STATUS: &str = "frozen";
const EXPECTED_OPENAPI_URL: &str = "https://docs.deribit.com/specifications/deribit_openapi.json";
const EXPECTED_OPENAPI_SHA256: &str =
    "9375d9a18543be88b33b751e600911d529ce331cf0a421b28b0673121f8e4f8d";
const EXPECTED_INDEX_URL: &str = "https://docs.deribit.com/llms.txt";
const EXPECTED_INDEX_SHA256: &str =
    "6d70e8cbaeb34e698b559e55eeb10182a0c790d9d10442f014a9f479c06e626c";
const EXPECTED_RETRIEVED_ON: &str = "2026-08-28";
const EXPECTED_API_VERSION: &str = "2.1.1";
const EXPECTED_PARAMETER_AUTHORITY: &str = "official method page and frozen OpenAPI operation";
const TABLE_PROFILE_IDS: [&str; 10] = [
    "supporting",
    "announcement",
    "instruments",
    "order_book",
    "index",
    "ticker",
    "volatility",
    "funding",
    "combo",
    "options",
];

/// An immutable registry built exclusively from the compile-time manifest.
#[derive(Clone, Debug)]
pub struct ManifestRegistry {
    document: ManifestDocument,
}

impl ManifestRegistry {
    /// Parses and validates the frozen manifest compiled into this binary.
    pub fn embedded() -> Result<Self, CoreError> {
        let document = parse_embedded_manifest()?;
        validate_manifest_document(&document)?;
        Ok(Self { document })
    }

    /// Returns the manifest identifier.
    pub fn manifest_id(&self) -> &str {
        &self.document.manifest_id
    }

    /// Returns the frozen snapshot identifier.
    pub fn snapshot_id(&self) -> &str {
        &self.document.snapshot.snapshot_id
    }

    /// Returns the canonical manifest hash pinned by S0.
    pub const fn manifest_sha256(&self) -> &'static str {
        EMBEDDED_MANIFEST_SHA256
    }

    /// Returns the frozen snapshot hash pinned by S0.
    pub const fn snapshot_sha256(&self) -> &'static str {
        EMBEDDED_SNAPSHOT_SHA256
    }

    /// Returns validated operations that cannot be constructed from user JSON.
    pub fn operations(&self) -> impl Iterator<Item = ValidatedOperation<'_>> {
        self.document
            .methods
            .iter()
            .map(|entry| ValidatedOperation { entry })
    }

    /// Returns the fixed number of v1 direct public methods.
    pub fn method_count(&self) -> usize {
        self.document.methods.len()
    }

    /// Resolves a stable operation ID only through the frozen registry.
    pub fn operation(&self, operation_id: &str) -> Result<ValidatedOperation<'_>, CoreError> {
        self.document
            .methods
            .iter()
            .find(|entry| entry.operation_id == operation_id)
            .map(|entry| ValidatedOperation { entry })
            .ok_or_else(|| CoreError::UnknownOperation(operation_id.into()))
    }

    /// Resolves a canonical command only through the frozen registry.
    pub fn canonical_command(&self, command: &str) -> Result<ValidatedOperation<'_>, CoreError> {
        self.document
            .methods
            .iter()
            .find(|entry| entry.canonical_command == command)
            .map(|entry| ValidatedOperation { entry })
            .ok_or_else(|| CoreError::UnknownCommand(command.into()))
    }
}

/// An operation proved to be part of the compiled frozen registry.
///
/// Its private field prevents callers from constructing a replacement method
/// or bypassing registry lookup with arbitrary JSON.
#[derive(Clone, Copy, Debug)]
pub struct ValidatedOperation<'a> {
    entry: &'a MethodEntry,
}

impl<'a> ValidatedOperation<'a> {
    /// Returns the upstream public method identifier.
    pub fn method(self) -> &'a str {
        &self.entry.method
    }

    /// Returns the stable operation identifier consumed by later stages.
    pub fn operation_id(self) -> &'a str {
        &self.entry.operation_id
    }

    /// Returns the frozen two-token CLI command spelling.
    pub fn canonical_command(self) -> &'a str {
        &self.entry.canonical_command
    }

    /// Returns the frozen command tokens.
    pub fn command_tokens(self) -> &'a [String] {
        &self.entry.command_tokens
    }

    /// Returns safe parameter descriptors in manifest order.
    pub fn parameters(self) -> impl Iterator<Item = ParameterDescriptor<'a>> {
        self.entry
            .parameters
            .iter()
            .map(|parameter| ParameterDescriptor { parameter })
    }

    /// Returns the number of frozen parameters.
    pub fn parameter_count(self) -> usize {
        self.entry.parameters.len()
    }

    /// Returns the implementation request path.
    pub fn request_path(self) -> &'a str {
        &self.entry.transport.path
    }

    /// Returns the fixed implementation HTTP verb.
    pub fn implementation_http_method(self) -> &'a str {
        &self.entry.transport.implementation_http_method
    }

    /// Returns the JSON-RPC version.
    pub fn jsonrpc_version(self) -> &'a str {
        &self.entry.transport.jsonrpc
    }

    /// Returns the fixed JSON-RPC named-parameters request shape.
    pub fn request_shape(self) -> &'a str {
        &self.entry.transport.request_shape
    }

    /// Returns the frozen finite request-response interaction label.
    pub fn interaction(self) -> &'a str {
        &self.entry.interaction
    }

    /// Returns whether pagination is explicitly requested by parameters.
    pub fn explicit_pagination(self) -> bool {
        matches!(self.entry.pagination.mode, PaginationMode::Explicit)
    }

    /// Returns frozen pagination parameter names.
    pub fn pagination_parameters(self) -> &'a [String] {
        &self.entry.pagination.parameters
    }

    /// Returns the named table profile, or None for the mandatory generic path.
    pub fn table_profile_id(self) -> Option<&'a str> {
        self.entry.presentation.table_profile_id()
    }

    /// All operations exposed by this type are public and read-only.
    pub fn is_public_read_only(self) -> bool {
        self.entry.is_public_read_only()
    }

    /// Returns non-contract-changing provenance notes retained from the snapshot.
    pub fn evidence_notes(self) -> &'a [String] {
        &self.entry.evidence_notes
    }
}

/// A safe read-only view of a validated manifest parameter.
#[derive(Clone, Copy, Debug)]
pub struct ParameterDescriptor<'a> {
    parameter: &'a ParameterSpec,
}

impl<'a> ParameterDescriptor<'a> {
    /// Returns the upstream parameter name.
    pub fn name(self) -> &'a str {
        &self.parameter.name
    }

    /// Returns the frozen direct flag spelling.
    pub fn flag(self) -> &'a str {
        &self.parameter.flag
    }

    /// Returns whether the parameter is required.
    pub fn required(self) -> bool {
        self.parameter.required
    }

    /// Returns the frozen scalar type.
    pub fn value_type(self) -> ParameterType {
        self.parameter.value_type
    }

    /// Returns allowed enum values.
    pub fn enum_values(self) -> &'a [Value] {
        &self.parameter.enum_values
    }

    /// Returns the declared default kind.
    pub fn default_kind(self) -> DefaultKind {
        self.parameter.default.kind
    }

    /// Returns a literal default when one is declared.
    pub fn literal_default(self) -> Option<&'a Value> {
        self.parameter.default.value.as_ref()
    }

    /// Returns the direct-flag omission behavior.
    pub fn cli_behavior(self) -> CliBehavior {
        self.parameter.cli_behavior
    }

    /// Returns whether the parameter is nullable. v1 validates this as false.
    pub fn nullable(self) -> bool {
        self.parameter.nullable
    }

    /// Returns the frozen unit label.
    pub fn unit(self) -> &'a str {
        &self.parameter.unit
    }

    /// Returns an optional inclusive minimum.
    pub fn minimum(self) -> Option<&'a Number> {
        self.parameter.constraints.minimum.as_ref()
    }

    /// Returns an optional inclusive maximum.
    pub fn maximum(self) -> Option<&'a Number> {
        self.parameter.constraints.maximum.as_ref()
    }

    /// Returns the description for a dynamic default, if declared.
    pub fn dynamic_default_description(self) -> Option<&'a str> {
        self.parameter.default.description.as_deref()
    }

    /// Returns the unit for a dynamic default, if declared.
    pub fn dynamic_default_unit(self) -> Option<&'a str> {
        self.parameter.default.unit.as_deref()
    }
}

/// One allowed API operation and its immutable source-derived policy.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MethodEntry {
    method: String,
    operation_id: String,
    canonical_command: String,
    command_tokens: Vec<String>,
    transport: MethodTransport,
    interaction: String,
    auth_required: bool,
    read_only: bool,
    streaming: bool,
    side_effect: String,
    pagination: PaginationSpec,
    parameters: Vec<ParameterSpec>,
    presentation: PresentationSpec,
    source: MethodSource,
    evidence_notes: Vec<String>,
}

impl MethodEntry {
    /// All S2 entries are verified public, unauthenticated, read-only entries.
    fn is_public_read_only(&self) -> bool {
        !self.auth_required && self.read_only && !self.streaming && self.side_effect == "none"
    }
}

/// The fixed JSON-RPC-over-HTTP properties for one method.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MethodTransport {
    official_http_method: String,
    implementation_http_method: String,
    path: String,
    jsonrpc: String,
    request_shape: String,
}

/// Explicit pagination policy from the frozen manifest.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PaginationSpec {
    mode: PaginationMode,
    implicit: bool,
    parameters: Vec<String>,
}

/// Pagination is either absent or explicitly represented in the command input.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum PaginationMode {
    None,
    Explicit,
}

/// A manifest parameter that later CLI code may bind without creating new names.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParameterSpec {
    name: String,
    flag: String,
    required: bool,
    #[serde(rename = "type")]
    value_type: ParameterType,
    #[serde(rename = "enum")]
    enum_values: Vec<Value>,
    default: ParameterDefault,
    cli_behavior: CliBehavior,
    nullable: bool,
    unit: String,
    constraints: ParameterConstraints,
    source: ParameterSource,
}

impl ParameterSpec {
    /// Returns the upstream parameter name.
    fn name(&self) -> &str {
        &self.name
    }

    /// Returns the frozen direct flag spelling.
    fn flag(&self) -> &str {
        &self.flag
    }
}

/// Scalar parameter type from the manifest.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ParameterType {
    String,
    Integer,
    Number,
    Boolean,
}

/// Direct-flag absence semantics from the manifest.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CliBehavior {
    MustSupply,
    OmitWhenAbsent,
}

/// Default semantics retained without inventing a local default.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParameterDefault {
    kind: DefaultKind,
    #[serde(default)]
    value: Option<Value>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    unit: Option<String>,
}

impl ParameterDefault {
    /// Returns a literal default only when the manifest declares one.
    fn value(&self) -> Option<&Value> {
        self.value.as_ref()
    }
}

/// Default value declaration status.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DefaultKind {
    NotDeclared,
    Literal,
    Dynamic,
}

/// Optional minimum and maximum constraints for a direct parameter.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParameterConstraints {
    #[serde(default)]
    minimum: Option<Number>,
    #[serde(default)]
    maximum: Option<Number>,
}

/// Source evidence for one parameter.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParameterSource {
    official_method_page: String,
    location: String,
    authority: String,
}

/// Table capability flags for one method.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PresentationSpec {
    table_profile_id: Option<String>,
    json_only: bool,
}

impl PresentationSpec {
    /// Returns the named profile or None for the required generic renderer.
    fn table_profile_id(&self) -> Option<&str> {
        self.table_profile_id.as_deref()
    }
}

/// Provenance for one upstream method.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MethodSource {
    official_method_page: String,
    openapi_pointer: String,
    snapshot_section: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestDocument {
    manifest_id: String,
    schema_version: String,
    status: String,
    snapshot: Snapshot,
    policy: ManifestPolicy,
    methods: Vec<MethodEntry>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    snapshot_id: String,
    source_file: String,
    source_sha256: String,
    official_openapi_url: String,
    official_openapi_sha256: String,
    official_index_url: String,
    official_index_sha256: String,
    retrieved_on: String,
    api_version: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestPolicy {
    namespace: String,
    auth_required: bool,
    read_only: bool,
    websocket_only: bool,
    streaming: bool,
    side_effect: String,
    official_transport: String,
    implementation_transport: String,
    implicit_retry: bool,
    implicit_redirect: bool,
    implicit_pagination: bool,
    one_request_per_operation: bool,
}

fn parse_embedded_manifest() -> Result<ManifestDocument, CoreError> {
    serde_json::from_str(EMBEDDED_MANIFEST)
        .map_err(|error| CoreError::EmbeddedManifest(error.to_string()))
}

fn validate_manifest_document(document: &ManifestDocument) -> Result<(), CoreError> {
    validate_equal("manifest_id", &document.manifest_id, EXPECTED_MANIFEST_ID)?;
    validate_equal(
        "schema_version",
        &document.schema_version,
        EXPECTED_SCHEMA_VERSION,
    )?;
    validate_equal("status", &document.status, EXPECTED_STATUS)?;
    validate_snapshot(&document.snapshot)?;
    validate_policy(&document.policy)?;

    if document.methods.len() != 38 {
        return Err(CoreError::ManifestValidation(format!(
            "expected 38 methods, found {}",
            document.methods.len()
        )));
    }

    let mut methods = HashSet::new();
    let mut operations = HashSet::new();
    let mut commands = HashSet::new();
    for entry in &document.methods {
        validate_method_entry(entry)?;
        if !methods.insert(entry.method.as_str()) {
            return Err(CoreError::ManifestValidation(format!(
                "duplicate method {}",
                entry.method
            )));
        }
        if !operations.insert(entry.operation_id.as_str()) {
            return Err(CoreError::ManifestValidation(format!(
                "duplicate operation_id {}",
                entry.operation_id
            )));
        }
        if !commands.insert(entry.canonical_command.as_str()) {
            return Err(CoreError::ManifestValidation(format!(
                "duplicate canonical command {}",
                entry.canonical_command
            )));
        }
    }
    Ok(())
}

fn validate_snapshot(snapshot: &Snapshot) -> Result<(), CoreError> {
    validate_equal("snapshot_id", &snapshot.snapshot_id, EMBEDDED_SNAPSHOT_ID)?;
    validate_equal(
        "snapshot source_file",
        &snapshot.source_file,
        "docs/api-snapshot-v1.md",
    )?;
    validate_equal(
        "snapshot source_sha256",
        &snapshot.source_sha256,
        EMBEDDED_SNAPSHOT_SHA256,
    )?;
    validate_equal(
        "official_openapi_url",
        &snapshot.official_openapi_url,
        EXPECTED_OPENAPI_URL,
    )?;
    validate_equal(
        "official_openapi_sha256",
        &snapshot.official_openapi_sha256,
        EXPECTED_OPENAPI_SHA256,
    )?;
    validate_equal(
        "official_index_url",
        &snapshot.official_index_url,
        EXPECTED_INDEX_URL,
    )?;
    validate_equal(
        "official_index_sha256",
        &snapshot.official_index_sha256,
        EXPECTED_INDEX_SHA256,
    )?;
    validate_equal(
        "retrieved_on",
        &snapshot.retrieved_on,
        EXPECTED_RETRIEVED_ON,
    )?;
    validate_equal("api_version", &snapshot.api_version, EXPECTED_API_VERSION)
}

fn validate_policy(policy: &ManifestPolicy) -> Result<(), CoreError> {
    let valid = policy.namespace == "public"
        && !policy.auth_required
        && policy.read_only
        && !policy.websocket_only
        && !policy.streaming
        && policy.side_effect == "none"
        && policy.official_transport == "HTTP GET with JSON-RPC 2.0"
        && policy.implementation_transport == "HTTP POST with JSON-RPC 2.0 named params"
        && !policy.implicit_retry
        && !policy.implicit_redirect
        && !policy.implicit_pagination
        && policy.one_request_per_operation;
    if valid {
        Ok(())
    } else {
        Err(CoreError::ManifestValidation(
            "manifest policy is not the frozen public read-only one-request policy".into(),
        ))
    }
}

fn validate_method_entry(entry: &MethodEntry) -> Result<(), CoreError> {
    let Some(suffix) = entry.method.strip_prefix("public/") else {
        return Err(CoreError::ManifestValidation(format!(
            "method {} is outside public namespace",
            entry.method
        )));
    };
    if !is_underscore_identifier(suffix) {
        return Err(CoreError::ManifestValidation(format!(
            "method {} has an invalid identifier",
            entry.method
        )));
    }
    validate_equal(
        "operation_id",
        &entry.operation_id,
        &format!("public_{suffix}"),
    )?;
    let command_suffix = suffix.replace('_', "-");
    validate_equal(
        "canonical_command",
        &entry.canonical_command,
        &format!("public {command_suffix}"),
    )?;
    if entry.command_tokens.as_slice() != ["public", command_suffix.as_str()] {
        return Err(CoreError::ManifestValidation(format!(
            "command tokens do not match {}",
            entry.canonical_command
        )));
    }
    if !entry.is_public_read_only() || entry.side_effect != "none" {
        return Err(CoreError::ManifestValidation(format!(
            "{} violates the public read-only boundary",
            entry.method
        )));
    }
    validate_equal("interaction", &entry.interaction, "finite_request_response")?;
    validate_transport(entry, suffix)?;
    validate_pagination(entry)?;
    validate_presentation(entry)?;
    validate_method_source(entry, suffix)?;
    validate_parameters(entry)?;
    Ok(())
}

fn validate_transport(entry: &MethodEntry, _suffix: &str) -> Result<(), CoreError> {
    let transport = &entry.transport;
    if transport.official_http_method != "GET"
        || transport.implementation_http_method != "POST"
        || transport.jsonrpc != "2.0"
        || transport.request_shape != "named_params"
        || transport.path != "/api/v2"
    {
        return Err(CoreError::ManifestValidation(format!(
            "{} has an invalid JSON-RPC transport contract",
            entry.method
        )));
    }
    Ok(())
}

fn validate_pagination(entry: &MethodEntry) -> Result<(), CoreError> {
    let pagination = &entry.pagination;
    if pagination.implicit {
        return Err(CoreError::ManifestValidation(format!(
            "{} enables implicit pagination",
            entry.method
        )));
    }
    if matches!(pagination.mode, PaginationMode::None) && !pagination.parameters.is_empty() {
        return Err(CoreError::ManifestValidation(format!(
            "{} has pagination parameters without explicit pagination",
            entry.method
        )));
    }
    let names: HashSet<&str> = entry
        .parameters
        .iter()
        .map(|parameter| parameter.name())
        .collect();
    if pagination
        .parameters
        .iter()
        .any(|parameter| !names.contains(parameter.as_str()))
    {
        return Err(CoreError::ManifestValidation(format!(
            "{} references an unknown pagination parameter",
            entry.method
        )));
    }
    Ok(())
}

fn validate_presentation(entry: &MethodEntry) -> Result<(), CoreError> {
    if entry.presentation.json_only {
        return Err(CoreError::ManifestValidation(format!(
            "{} incorrectly disables table output",
            entry.method
        )));
    }
    if let Some(profile) = entry.presentation.table_profile_id() {
        if !TABLE_PROFILE_IDS.contains(&profile) {
            return Err(CoreError::ManifestValidation(format!(
                "{} references unknown table profile {profile}",
                entry.method
            )));
        }
    }
    Ok(())
}

fn validate_method_source(entry: &MethodEntry, suffix: &str) -> Result<(), CoreError> {
    validate_https_url(
        "method source official_method_page",
        &entry.source.official_method_page,
    )?;
    validate_equal(
        "method source openapi_pointer",
        &entry.source.openapi_pointer,
        &format!("paths./public/{suffix}.get"),
    )?;
    if !entry.source.snapshot_section.starts_with("4.") {
        return Err(CoreError::ManifestValidation(format!(
            "{} has invalid snapshot section",
            entry.method
        )));
    }
    Ok(())
}

fn validate_parameters(entry: &MethodEntry) -> Result<(), CoreError> {
    let mut names = HashSet::new();
    let mut flags = HashSet::new();
    for parameter in &entry.parameters {
        if !is_underscore_identifier(parameter.name()) {
            return Err(CoreError::ManifestValidation(format!(
                "{} has invalid parameter name {}",
                entry.method,
                parameter.name()
            )));
        }
        validate_equal(
            "parameter flag",
            parameter.flag(),
            &format!("--{}", parameter.name().replace('_', "-")),
        )?;
        if !names.insert(parameter.name()) || !flags.insert(parameter.flag()) {
            return Err(CoreError::ManifestValidation(format!(
                "{} has duplicate parameter name or flag",
                entry.method
            )));
        }
        if parameter.nullable {
            return Err(CoreError::ManifestValidation(format!(
                "{} parameter {} is unexpectedly nullable",
                entry.method,
                parameter.name()
            )));
        }
        let expected_behavior = if parameter.required {
            CliBehavior::MustSupply
        } else {
            CliBehavior::OmitWhenAbsent
        };
        if parameter.cli_behavior != expected_behavior {
            return Err(CoreError::ManifestValidation(format!(
                "{} parameter {} has inconsistent CLI behavior",
                entry.method,
                parameter.name()
            )));
        }
        validate_parameter_default(entry, parameter)?;
        validate_parameter_values(entry, parameter)?;
        validate_parameter_constraints(entry, parameter)?;
        validate_parameter_source(entry, parameter)?;
    }
    Ok(())
}

fn validate_parameter_default(
    entry: &MethodEntry,
    parameter: &ParameterSpec,
) -> Result<(), CoreError> {
    let default = &parameter.default;
    match default.kind {
        DefaultKind::NotDeclared => {
            if default.value.is_some() || default.description.is_some() || default.unit.is_some() {
                return Err(CoreError::ManifestValidation(format!(
                    "{} parameter {} has an invalid not_declared default",
                    entry.method,
                    parameter.name()
                )));
            }
        }
        DefaultKind::Literal => {
            let Some(value) = default.value() else {
                return Err(CoreError::ManifestValidation(format!(
                    "{} parameter {} is missing literal default value",
                    entry.method,
                    parameter.name()
                )));
            };
            if !value_matches_type(value, parameter.value_type) {
                return Err(CoreError::ManifestValidation(format!(
                    "{} parameter {} has a literal default with the wrong type",
                    entry.method,
                    parameter.name()
                )));
            }
        }
        DefaultKind::Dynamic => {
            if default.description.as_deref().is_none_or(str::is_empty) {
                return Err(CoreError::ManifestValidation(format!(
                    "{} parameter {} is missing dynamic default description",
                    entry.method,
                    parameter.name()
                )));
            }
        }
    }
    Ok(())
}

fn validate_parameter_values(
    entry: &MethodEntry,
    parameter: &ParameterSpec,
) -> Result<(), CoreError> {
    if parameter
        .enum_values
        .iter()
        .any(|value| !value_matches_type(value, parameter.value_type))
    {
        return Err(CoreError::ManifestValidation(format!(
            "{} parameter {} has enum value with wrong type",
            entry.method,
            parameter.name()
        )));
    }
    if let Some(default) = parameter.default.value() {
        if !parameter.enum_values.is_empty() && !parameter.enum_values.contains(default) {
            return Err(CoreError::ManifestValidation(format!(
                "{} parameter {} has default outside enum",
                entry.method,
                parameter.name()
            )));
        }
    }
    Ok(())
}

fn validate_parameter_constraints(
    entry: &MethodEntry,
    parameter: &ParameterSpec,
) -> Result<(), CoreError> {
    let constraints = &parameter.constraints;
    if let (Some(minimum), Some(maximum)) = (&constraints.minimum, &constraints.maximum) {
        let minimum = minimum.as_f64().ok_or_else(|| {
            CoreError::ManifestValidation(format!(
                "{} parameter {} has non-finite minimum",
                entry.method,
                parameter.name()
            ))
        })?;
        let maximum = maximum.as_f64().ok_or_else(|| {
            CoreError::ManifestValidation(format!(
                "{} parameter {} has non-finite maximum",
                entry.method,
                parameter.name()
            ))
        })?;
        if minimum > maximum {
            return Err(CoreError::ManifestValidation(format!(
                "{} parameter {} has inverted constraints",
                entry.method,
                parameter.name()
            )));
        }
    }
    Ok(())
}

fn validate_parameter_source(
    entry: &MethodEntry,
    parameter: &ParameterSpec,
) -> Result<(), CoreError> {
    validate_https_url(
        "parameter source official_method_page",
        &parameter.source.official_method_page,
    )?;
    if parameter.source.location.is_empty()
        || parameter.source.authority != EXPECTED_PARAMETER_AUTHORITY
    {
        return Err(CoreError::ManifestValidation(format!(
            "{} parameter {} has invalid source evidence",
            entry.method,
            parameter.name()
        )));
    }
    Ok(())
}

fn value_matches_type(value: &Value, parameter_type: ParameterType) -> bool {
    match parameter_type {
        ParameterType::String => value.is_string(),
        ParameterType::Integer => value.as_i64().is_some() || value.as_u64().is_some(),
        ParameterType::Number => value.is_number(),
        ParameterType::Boolean => value.is_boolean(),
    }
}

fn validate_https_url(label: &str, value: &str) -> Result<(), CoreError> {
    let parsed = Url::parse(value).map_err(|error| {
        CoreError::ManifestValidation(format!("{label} is not a valid URL: {error}"))
    })?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return Err(CoreError::ManifestValidation(format!(
            "{label} must be an absolute HTTPS URL"
        )));
    }
    Ok(())
}

fn validate_equal(label: &str, actual: &str, expected: &str) -> Result<(), CoreError> {
    if actual == expected {
        Ok(())
    } else {
        Err(CoreError::ManifestValidation(format!(
            "{label} expected {expected:?}, found {actual:?}"
        )))
    }
}

fn is_underscore_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

#[cfg(test)]
mod tests {
    use super::{
        EMBEDDED_MANIFEST_SHA256, EMBEDDED_SNAPSHOT_ID, EMBEDDED_SNAPSHOT_SHA256, ManifestRegistry,
        parse_embedded_manifest, validate_manifest_document,
    };
    use crate::errors::CoreError;

    #[test]
    fn embedded_registry_is_complete_and_read_only() {
        let registry = ManifestRegistry::embedded().unwrap();

        assert_eq!(registry.method_count(), 38);
        assert_eq!(registry.snapshot_id(), EMBEDDED_SNAPSHOT_ID);
        assert_eq!(registry.manifest_sha256(), EMBEDDED_MANIFEST_SHA256);
        assert_eq!(registry.snapshot_sha256(), EMBEDDED_SNAPSHOT_SHA256);
        assert!(
            registry
                .operations()
                .all(|entry| entry.is_public_read_only())
        );
    }

    #[test]
    fn registry_refuses_unknown_operation_and_command() {
        let registry = ManifestRegistry::embedded().unwrap();

        assert!(matches!(
            registry.operation("private_buy"),
            Err(CoreError::UnknownOperation(_))
        ));
        assert!(matches!(
            registry.canonical_command("public no-such-method"),
            Err(CoreError::UnknownCommand(_))
        ));
    }

    #[test]
    fn validation_rejects_a_private_method_even_in_an_embedded_shape() {
        let mut document = parse_embedded_manifest().unwrap();
        document.methods[0].method = "private/buy".into();

        assert!(matches!(
            validate_manifest_document(&document),
            Err(CoreError::ManifestValidation(_))
        ));
    }

    #[test]
    fn validation_rejects_generic_transport_escape() {
        let mut document = parse_embedded_manifest().unwrap();
        document.methods[0].transport.path = "/api/v2/public/get_time".into();

        assert!(matches!(
            validate_manifest_document(&document),
            Err(CoreError::ManifestValidation(_))
        ));
    }

    #[test]
    fn strict_deserialization_rejects_unknown_manifest_fields() {
        let malformed = super::EMBEDDED_MANIFEST.replacen(
            "\"status\": \"frozen\"",
            "\"status\": \"frozen\", \"unexpected\": true",
            1,
        );

        let parsed: Result<super::ManifestDocument, _> = serde_json::from_str(&malformed);
        assert!(parsed.is_err());
    }

    #[test]
    fn validated_operations_expose_only_frozen_parameter_contracts() {
        let registry = ManifestRegistry::embedded().unwrap();
        let operation = registry.operation("public_get_announcements").unwrap();
        let parameter = operation.parameters().next().unwrap();

        assert_eq!(operation.implementation_http_method(), "POST");
        assert_eq!(operation.request_shape(), "named_params");
        assert_eq!(parameter.name(), "start_timestamp");
        assert!(!parameter.required());
        assert!(!parameter.nullable());
        assert_eq!(parameter.default_kind(), super::DefaultKind::Dynamic);
        assert_eq!(
            parameter.dynamic_default_description(),
            Some("server current time")
        );
    }
}
