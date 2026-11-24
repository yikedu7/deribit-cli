use std::collections::BTreeSet;
use std::net::Ipv4Addr;

use serde_json::{Map, Value};

use crate::api_application::{Call, Failure};
use crate::api_catalog::{Catalog, Effect};

pub(crate) struct Scope<'a>(&'a str);

impl<'a> Scope<'a> {
    pub fn request(value: &'a str) -> Result<Self, Failure> {
        let mut areas = BTreeSet::new();
        let mut binding = false;
        for word in value.split_whitespace() {
            let (area, level) = word.split_once(':').unwrap_or((word, ""));
            let valid = match area {
                "connection" => level.is_empty(),
                "session" => {
                    (1..=64).contains(&level.len())
                        && level
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
                }
                "account" | "trade" | "wallet" | "custody" | "block_trade" | "block_rfq" => {
                    matches!(level, "none" | "read" | "read_write")
                }
                "expires" => level.parse::<u64>().is_ok_and(|value| value > 0),
                "ip" => valid_ip(level),
                _ => false,
            };
            if !valid || !areas.insert(area) {
                return Err(Failure::Input("invalid Mainnet scope specification"));
            }
            if matches!(area, "connection" | "session") {
                if binding {
                    return Err(Failure::Input("choose a single Mainnet token binding"));
                }
                binding = true;
            }
        }
        if !binding {
            return Err(Failure::Input(
                "Mainnet scope must select connection or session:<name>",
            ));
        }
        Ok(Self(value))
    }

    pub fn is_named(&self) -> bool {
        self.0
            .split_whitespace()
            .any(|word| word.starts_with("session:"))
    }

    pub fn prepare(&self, call: &mut Call, catalog: &Catalog) -> Result<(), Failure> {
        if call.method == "public/auth" {
            if call
                .params
                .get("scope")
                .is_some_and(|scope| scope.as_str() != Some(self.0))
            {
                return Err(Failure::Input("Mainnet session auth cannot change --scope"));
            }
            self.apply(&mut call.params);
        }
        if call.method == "public/exchange_token" && call.params.contains_key("scope") {
            return Err(Failure::Input(
                "Mainnet token exchange inherits the authenticated scope",
            ));
        }
        if call.method == "public/fork_token" && !self.is_named() {
            return Err(Failure::Input(
                "token forking requires a named session scope",
            ));
        }
        if !required_capabilities(call, catalog).all(|needed| permits(self.0, needed)) {
            return Err(Failure::InsufficientScope);
        }
        Ok(())
    }

    pub fn apply(&self, params: &mut Map<String, Value>) {
        let functional = [
            "account",
            "trade",
            "wallet",
            "custody",
            "block_trade",
            "block_rfq",
        ];
        let mut requested = self.0.to_owned();
        for area in functional {
            if !self
                .0
                .split_whitespace()
                .any(|word| word.split_once(':').is_some_and(|(name, _)| name == area))
            {
                requested.push(' ');
                requested.push_str(area);
                requested.push_str(":none");
            }
        }
        params.insert("scope".to_owned(), Value::String(requested));
    }

    pub fn check(
        &self,
        call: &Call,
        catalog: &Catalog,
        granted: Option<&str>,
    ) -> Result<(), Failure> {
        let actual = granted.ok_or(Failure::InsufficientScope)?;
        let row = catalog.method(&call.method).expect("prevalidated");
        let needs_main = matches!(
            call.method.as_str(),
            "private/move_positions" | "public/exchange_token"
        ) || call.params.contains_key("subaccount_id")
            || row
                .documented_scope
                .as_deref()
                .is_some_and(|text| text.ends_with(" and mainaccount"));
        let named_required = call.method == "public/fork_token";
        if (needs_main && !actual.split_whitespace().any(|word| word == "mainaccount"))
            || (named_required
                && !actual
                    .split_whitespace()
                    .any(|word| word.starts_with("session:")))
            || !required_capabilities(call, catalog).all(|needed| permits(actual, needed))
        {
            return Err(Failure::InsufficientScope);
        }
        Ok(())
    }
}

fn valid_ip(value: &str) -> bool {
    if let Some((address, bits)) = value.split_once('/') {
        address.parse::<Ipv4Addr>().is_ok() && bits.parse::<u8>().is_ok_and(|n| n <= 32)
    } else {
        value == "*" || value.parse::<Ipv4Addr>().is_ok()
    }
}

fn permits(scope: &str, required: &str) -> bool {
    let (area, needed) = required.split_once(':').expect("documented capability");
    let area = if area == "wallets" { "wallet" } else { area };
    let mut levels = scope.split_whitespace().filter_map(|word| {
        let (name, level) = word.split_once(':')?;
        (name == area).then_some(level)
    });
    matches!(
        (levels.next(), needed),
        (Some("read_write"), "read" | "read_write") | (Some("read"), "read")
    ) && levels.next().is_none()
}

fn required_capabilities<'a>(
    call: &'a Call,
    catalog: &'a Catalog,
) -> impl Iterator<Item = &'a str> {
    let method = catalog.method(&call.method).expect("prevalidated");
    let documented = method.documented_scope.as_deref().and_then(|scope| {
        // A public method can be called anonymously even when the docs list a scope.
        if !method.auth
            || (scope.contains("only for")
                && call
                    .params
                    .get("scope")
                    .is_none_or(|scope| scope == "connection"))
        {
            return None;
        }
        let alternative = usize::from(
            scope.contains("when")
                && call.params.get("block_rfq").and_then(Value::as_bool) == Some(true),
        );
        scope.split('`').skip(1).step_by(2).nth(alternative)
    });
    let channel_scopes = call
        .params
        .get("channels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|name| catalog.channel(name.as_str()?))
        .filter_map(|channel| channel.documented_scope.as_deref()?.split('`').nth(1));
    documented.into_iter().chain(channel_scopes)
}

pub(crate) fn is_mainnet_action(call: &Call, catalog: &Catalog) -> bool {
    let effect = catalog.method(&call.method).expect("prevalidated").effect;
    matches!(effect, Effect::Write | Effect::Special)
        || matches!(
            call.method.as_str(),
            "public/exchange_token" | "public/fork_token"
        )
        || (call.method == "private/logout"
            && call.params.get("invalidate_token") != Some(&Value::Bool(false)))
}
