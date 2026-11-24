use std::collections::HashSet;

use deribit_cli::{ManifestRegistry, product_command};

#[test]
fn product_tree_has_one_named_command_surface() {
    let registry = ManifestRegistry::embedded().unwrap();
    let command = product_command(&registry);
    let root_names = command
        .get_subcommands()
        .map(|subcommand| subcommand.get_name())
        .collect::<HashSet<_>>();
    assert_eq!(
        root_names,
        HashSet::from([
            "market",
            "orders",
            "portfolio",
            "account",
            "wallet",
            "stream",
            "rpc",
            "catalog",
            "completion",
        ])
    );

    let mut public_methods = HashSet::new();
    let mut named = Vec::new();
    for group_name in ["market", "orders", "portfolio", "account", "wallet"] {
        let group = command.find_subcommand(group_name).unwrap();
        for child in group.get_subcommands() {
            named.push(format!("{group_name} {}", child.get_name()));
            let Some(operation) = registry.operations().find(|operation| {
                let token = operation.command_tokens()[1].as_str();
                token == child.get_name() || token.strip_prefix("get-") == Some(child.get_name())
            }) else {
                continue;
            };
            public_methods.insert(operation.method().to_owned());
            let actual_flags = child
                .get_arguments()
                .filter_map(|argument| argument.get_long())
                .map(|long| format!("--{long}"))
                .collect::<HashSet<_>>();
            let expected_flags = operation
                .parameters()
                .map(|parameter| parameter.flag().to_owned())
                .collect::<HashSet<_>>();
            assert_eq!(actual_flags, expected_flags, "{}", operation.method());
        }
    }
    assert_eq!(named.len(), 39);
    assert_eq!(public_methods.len(), 35);
    for expected in [
        "market ticker",
        "orders list",
        "portfolio positions",
        "account summary",
        "wallet transfers",
    ] {
        assert!(named.iter().any(|name| name == expected), "{expected}");
    }
    assert!(command.find_subcommand("public").is_none());
    assert!(command.find_subcommand("coverage").is_none());
    assert!(command.find_subcommand("version").is_none());
}
