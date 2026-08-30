//! Consumer-facing registry binding validation.
//!
//! S3 can bind its frozen command syntax through this module, while S4 receives
//! a ValidatedOperation rather than a caller-provided method string. This
//! preserves the no-generic-call boundary from the core outward.

use std::collections::HashSet;

use crate::errors::CoreError;
use crate::manifest::{ManifestRegistry, ValidatedOperation};

/// A command-to-operation assertion supplied by a later frozen adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandCorrespondence {
    operation_id: String,
    canonical_command: String,
}

impl CommandCorrespondence {
    /// Creates a correspondence that must be checked against the immutable registry.
    pub fn new(operation_id: impl Into<String>, canonical_command: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.into(),
            canonical_command: canonical_command.into(),
        }
    }

    /// Returns the asserted operation ID.
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    /// Returns the asserted command spelling.
    pub fn canonical_command(&self) -> &str {
        &self.canonical_command
    }
}

/// Validates one already-parsed command binding before any request can be made.
pub fn validate_operation_binding<'a>(
    registry: &'a ManifestRegistry,
    operation_id: &str,
    command_tokens: &[String],
) -> Result<ValidatedOperation<'a>, CoreError> {
    let entry = registry.operation(operation_id)?;
    if entry.command_tokens() != command_tokens {
        return Err(CoreError::CommandCorrespondence(format!(
            "operation {operation_id} does not match frozen command tokens"
        )));
    }
    Ok(entry)
}

/// Validates exact set equality between future CLI bindings and the manifest.
///
/// The function deliberately validates both directions: no unknown operation,
/// duplicate operation, duplicate command, mismatch, or missing frozen method
/// is permitted.
pub fn validate_command_correspondence(
    registry: &ManifestRegistry,
    bindings: &[CommandCorrespondence],
) -> Result<(), CoreError> {
    if bindings.len() != registry.method_count() {
        return Err(CoreError::CommandCorrespondence(format!(
            "expected {} bindings, found {}",
            registry.method_count(),
            bindings.len()
        )));
    }

    let mut operation_ids = HashSet::new();
    let mut commands = HashSet::new();
    for binding in bindings {
        let entry = registry.operation(binding.operation_id())?;
        if entry.canonical_command() != binding.canonical_command() {
            return Err(CoreError::CommandCorrespondence(format!(
                "operation {} is bound to {}, expected {}",
                binding.operation_id(),
                binding.canonical_command(),
                entry.canonical_command()
            )));
        }
        if !operation_ids.insert(binding.operation_id()) {
            return Err(CoreError::CommandCorrespondence(format!(
                "duplicate operation binding {}",
                binding.operation_id()
            )));
        }
        if !commands.insert(binding.canonical_command()) {
            return Err(CoreError::CommandCorrespondence(format!(
                "duplicate command binding {}",
                binding.canonical_command()
            )));
        }
    }

    for entry in registry.operations() {
        if !operation_ids.contains(entry.operation_id()) {
            return Err(CoreError::CommandCorrespondence(format!(
                "missing binding for {}",
                entry.operation_id()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        CommandCorrespondence, validate_command_correspondence, validate_operation_binding,
    };
    use crate::errors::CoreError;
    use crate::manifest::ManifestRegistry;

    fn complete_bindings(registry: &ManifestRegistry) -> Vec<CommandCorrespondence> {
        registry
            .operations()
            .map(|entry| {
                CommandCorrespondence::new(entry.operation_id(), entry.canonical_command())
            })
            .collect()
    }

    #[test]
    fn complete_frozen_correspondence_is_accepted() {
        let registry = ManifestRegistry::embedded().unwrap();
        validate_command_correspondence(&registry, &complete_bindings(&registry)).unwrap();
    }

    #[test]
    fn missing_or_mismatched_binding_is_rejected() {
        let registry = ManifestRegistry::embedded().unwrap();
        let mut bindings = complete_bindings(&registry);
        bindings.pop();

        assert!(matches!(
            validate_command_correspondence(&registry, &bindings),
            Err(CoreError::CommandCorrespondence(_))
        ));

        let first = registry.operations().next().unwrap();
        let wrong_tokens = vec!["public".to_owned(), "not-frozen".to_owned()];
        assert!(matches!(
            validate_operation_binding(&registry, first.operation_id(), &wrong_tokens),
            Err(CoreError::CommandCorrespondence(_))
        ));
    }
}
