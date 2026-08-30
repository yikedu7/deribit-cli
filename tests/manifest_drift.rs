use std::collections::HashSet;

use deribit_cli::{
    CommandCorrespondence, ManifestRegistry, build_cli, validate_command_correspondence,
};

#[test]
fn manifest_and_dynamic_cli_have_exact_correspondence() {
    let registry = ManifestRegistry::embedded().unwrap();
    let command = build_cli(&registry).unwrap();
    let public = command
        .get_subcommands()
        .find(|subcommand| subcommand.get_name() == "public")
        .unwrap();
    let cli_commands = public
        .get_subcommands()
        .map(|subcommand| format!("public {}", subcommand.get_name()))
        .collect::<HashSet<_>>();
    let manifest_commands = registry
        .operations()
        .map(|operation| operation.canonical_command().to_owned())
        .collect::<HashSet<_>>();
    assert_eq!(cli_commands.len(), 38);
    assert_eq!(cli_commands, manifest_commands);

    let bindings = registry
        .operations()
        .map(|operation| {
            let method_command = public
                .get_subcommands()
                .find(|subcommand| subcommand.get_name() == operation.command_tokens()[1])
                .unwrap();
            let actual_flags = method_command
                .get_arguments()
                .filter_map(|argument| argument.get_long())
                .map(|long| format!("--{long}"))
                .collect::<HashSet<_>>();
            let expected_flags = operation
                .parameters()
                .map(|parameter| parameter.flag().to_owned())
                .collect::<HashSet<_>>();
            assert_eq!(actual_flags, expected_flags, "{}", operation.method());
            CommandCorrespondence::new(operation.operation_id(), operation.canonical_command())
        })
        .collect::<Vec<_>>();
    validate_command_correspondence(&registry, &bindings).unwrap();

    let root_names = command
        .get_subcommands()
        .map(|subcommand| subcommand.get_name())
        .collect::<HashSet<_>>();
    assert_eq!(
        root_names,
        HashSet::from(["public", "coverage", "version", "completion"])
    );
    assert!(command.get_arguments().all(|argument| {
        !matches!(
            argument.get_long(),
            Some("method" | "endpoint" | "url" | "retry" | "all")
        )
    }));
}
