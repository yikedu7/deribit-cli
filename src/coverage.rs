//! Machine-readable coverage metadata kept separate from API-native output.

use serde::Serialize;

use crate::manifest::ManifestRegistry;

/// Build-time snapshot and correspondence metadata for the local coverage command.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CoverageMetadata {
    pub tool_version: String,
    pub api_snapshot: String,
    pub snapshot_sha256: String,
    pub manifest_id: String,
    pub manifest_sha256: String,
    pub methods: Vec<CoverageMethod>,
}

impl CoverageMetadata {
    /// Builds metadata only from the immutable embedded registry.
    pub fn from_registry(registry: &ManifestRegistry) -> Self {
        let methods = registry
            .operations()
            .map(|entry| CoverageMethod {
                api_method: entry.method().to_owned(),
                operation_id: entry.operation_id().to_owned(),
                command: entry.canonical_command().to_owned(),
                auth: false,
                transport: "http-json-rpc".into(),
                interaction: entry.interaction().to_owned(),
                status: "frozen".into(),
            })
            .collect();

        Self {
            tool_version: env!("CARGO_PKG_VERSION").into(),
            api_snapshot: registry.snapshot_id().into(),
            snapshot_sha256: registry.snapshot_sha256().into(),
            manifest_id: registry.manifest_id().into(),
            manifest_sha256: registry.manifest_sha256().into(),
            methods,
        }
    }
}

/// One frozen API-method-to-command mapping.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CoverageMethod {
    pub api_method: String,
    pub operation_id: String,
    pub command: String,
    pub auth: bool,
    pub transport: String,
    pub interaction: String,
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::CoverageMetadata;
    use crate::manifest::{EMBEDDED_MANIFEST_SHA256, EMBEDDED_SNAPSHOT_ID, ManifestRegistry};

    #[test]
    fn coverage_metadata_has_all_frozen_mappings() {
        let registry = ManifestRegistry::embedded().unwrap();
        let coverage = CoverageMetadata::from_registry(&registry);

        assert_eq!(coverage.api_snapshot, EMBEDDED_SNAPSHOT_ID);
        assert_eq!(coverage.manifest_sha256, EMBEDDED_MANIFEST_SHA256);
        assert_eq!(coverage.methods.len(), 38);
        assert!(
            coverage
                .methods
                .iter()
                .all(|method| !method.auth && method.status == "frozen")
        );
    }
}
