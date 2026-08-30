//! Deterministic human-readable table rendering from immutable native JSON.

use std::{collections::BTreeSet, env};

use serde_json::{Map, Number, Value};
use tabled::{
    builder::Builder,
    settings::{Alignment, Panel, Style, object::Columns},
};
use thiserror::Error;

use crate::{
    ColorPolicy, GlobalOptions, NativeResponse,
    table_profiles::{FieldKind, TableProfile},
};

const MIN_WIDTH: usize = 20;
const MAX_WIDTH: usize = 240;
const DEFAULT_WIDTH: usize = 120;

/// Fully explicit terminal context. Golden tests never inspect the real terminal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationContext {
    pub operation: String,
    pub profile: TableProfile,
    pub environment: Option<String>,
    pub width: usize,
    pub unicode: bool,
    pub color: bool,
}

impl PresentationContext {
    /// Creates deterministic renderer inputs for tests and non-process consumers.
    pub fn explicit(
        operation: impl Into<String>,
        profile: TableProfile,
        environment: Option<impl Into<String>>,
        width: usize,
        unicode: bool,
        color: bool,
    ) -> Self {
        Self {
            operation: operation.into(),
            profile,
            environment: environment.map(Into::into),
            width: width.clamp(MIN_WIDTH, MAX_WIDTH),
            unicode,
            color,
        }
    }

    /// Resolves the frozen process rules without changing any data field.
    pub fn from_process(
        operation: impl Into<String>,
        profile: TableProfile,
        environment: Option<impl Into<String>>,
        global: GlobalOptions,
        stdout_is_terminal: bool,
    ) -> Self {
        let term = env::var("TERM").unwrap_or_default();
        let utf8 = ["LC_ALL", "LC_CTYPE", "LANG"].iter().any(|name| {
            env::var(name).ok().is_some_and(|value| {
                value.to_ascii_uppercase().contains("UTF-8")
                    || value.to_ascii_uppercase().contains("UTF8")
            })
        });
        let capable = stdout_is_terminal && term != "dumb" && utf8;
        let no_color = env::var_os("NO_COLOR").is_some();
        let color = !no_color
            && match global.color {
                ColorPolicy::Always => true,
                ColorPolicy::Never => false,
                ColorPolicy::Auto => capable,
            };
        let width = global
            .table_width
            .map(usize::from)
            .or_else(|| env::var("COLUMNS").ok()?.parse::<usize>().ok())
            .unwrap_or(DEFAULT_WIDTH)
            .clamp(MIN_WIDTH, MAX_WIDTH);
        Self {
            operation: operation.into(),
            profile,
            environment: environment.map(Into::into),
            width,
            unicode: capable,
            color,
        }
    }
}

/// Rendered stdout and the exact count of cells whose display was shortened.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableOutput {
    text: String,
    truncated_cells: usize,
}

impl TableOutput {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub const fn truncated_cells(&self) -> usize {
        self.truncated_cells
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.text.into_bytes()
    }
}

#[derive(Debug, Error)]
pub enum PresentationError {
    #[error("API-native errors must remain JSON and cannot be rendered as a success table")]
    ApiError,
    #[error("table profile is not in the frozen registry: {0}")]
    UnknownProfile(String),
}

/// Renders a validated success response without mutating or consuming it.
pub fn render_native(
    native: &NativeResponse,
    context: &PresentationContext,
) -> Result<TableOutput, PresentationError> {
    if native.is_api_error() {
        return Err(PresentationError::ApiError);
    }
    let payload = native
        .value()
        .as_object()
        .and_then(|object| object.get("result"))
        .unwrap_or_else(|| native.value());
    Ok(render_value(payload, context))
}

/// Renders a local/tool-owned or already-unwrapped JSON value.
pub fn render_value(payload: &Value, context: &PresentationContext) -> TableOutput {
    let mut renderer = Renderer::new(context, unknown_unit_fields(payload, context.profile));
    let body = if context.profile == TableProfile::OrderBook {
        renderer.render_order_book(payload)
    } else {
        renderer.render_shape(payload)
    };
    let rows = payload.as_array().map(Vec::len);
    let mut title = context.operation.clone();
    let separator = if context.unicode { " · " } else { " - " };
    title.push_str(separator);
    title.push_str("profile=");
    title.push_str(context.profile.as_str());
    if let Some(environment) = &context.environment {
        title.push_str(separator);
        title.push_str("env=");
        title.push_str(environment);
    }
    if let Some(rows) = rows {
        title.push_str(separator);
        title.push_str(&format!("rows={rows}"));
    }
    title = truncate_tail(&title, context.width, context.unicode).0;

    let mut text = format!("{title}\n{body}");
    if let Some(status) = presentation_status(payload, &renderer.unknown_units) {
        text.push('\n');
        text.push_str(&status);
    }
    if renderer.truncated > 0 {
        text.push('\n');
        text.push_str(&format!(
            "note: {} cell(s) truncated; replace --output table with --output json to view complete API-native values.",
            renderer.truncated
        ));
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    if context.color {
        text = apply_color(text, context.unicode);
    }
    TableOutput {
        text,
        truncated_cells: renderer.truncated,
    }
}

struct Renderer<'a> {
    context: &'a PresentationContext,
    truncated: usize,
    unknown_units: BTreeSet<String>,
}

impl<'a> Renderer<'a> {
    fn new(context: &'a PresentationContext, unknown_units: BTreeSet<String>) -> Self {
        Self {
            context,
            truncated: 0,
            unknown_units,
        }
    }

    fn render_shape(&mut self, payload: &Value) -> String {
        match payload {
            Value::Object(object) => self.render_field_value(object),
            Value::Array(values) if values.is_empty() => self.render_empty(),
            Value::Array(values) if self.is_time_series(values) => self.render_time_series(values),
            Value::Array(values) if values.iter().all(Value::is_object) => {
                self.render_object_list(values)
            }
            Value::Array(values) if values.iter().all(|value| !value.is_object()) => {
                self.render_scalar_list(values)
            }
            Value::Array(values) => self.render_heterogeneous(values),
            value => self.render_scalar(value),
        }
    }

    fn render_field_value(&mut self, object: &Map<String, Value>) -> String {
        let flattened = flatten_object(object);
        let columns = ordered_columns(
            flattened.iter().map(|(path, _)| path.as_str()),
            self.context.profile,
        );
        let mut rows = Vec::with_capacity(columns.len());
        let natural_field_width = columns
            .iter()
            .map(|path| display_width(&header(path, self.context.profile.field_kind(path))))
            .max()
            .unwrap_or(5);
        let (field_width, value_width) = two_column_widths(natural_field_width, self.context.width);
        for path in &columns {
            let value = flattened
                .iter()
                .find_map(|(candidate, value)| (candidate == path).then_some(value))
                .expect("column originates from flattened object");
            let kind = self.context.profile.field_kind(path);
            let raw = format_value(value, kind);
            let (value, cut) =
                truncate_for_kind(&raw, value_width.max(4), kind, self.context.unicode);
            self.truncated += usize::from(cut);
            let (field, cut) =
                truncate_middle(&header(path, kind), field_width, self.context.unicode);
            self.truncated += usize::from(cut);
            rows.push(vec![field, value]);
        }
        if rows.is_empty() {
            rows.push(vec!["value".into(), "{}".into()]);
        }
        self.grid(vec!["field".into(), "value".into()], rows, &[])
    }

    fn render_object_list(&mut self, values: &[Value]) -> String {
        let flattened = values
            .iter()
            .map(|value| flatten_object(value.as_object().expect("shape checked")))
            .collect::<Vec<_>>();
        let columns = ordered_columns(
            flattened
                .iter()
                .flat_map(|object| object.iter().map(|(path, _)| path.as_str())),
            self.context.profile,
        );
        if self.should_stack(&columns, &flattened) {
            return self.render_stacked(&flattened, &columns);
        }
        let limits = allocate_grid_widths(&columns, &flattened, self.context);
        let mut rows = Vec::with_capacity(values.len());
        for object in &flattened {
            let mut row = Vec::with_capacity(columns.len());
            for (index, path) in columns.iter().enumerate() {
                let kind = self.context.profile.field_kind(path);
                let value = object
                    .iter()
                    .find_map(|(candidate, value)| (candidate == path).then_some(value))
                    .map(|value| format_value(value, kind))
                    .unwrap_or_else(|| "missing".into());
                let (value, cut) =
                    truncate_for_kind(&value, limits[index], kind, self.context.unicode);
                self.truncated += usize::from(cut);
                row.push(value);
            }
            rows.push(row);
        }
        let headers = columns
            .iter()
            .map(|path| header(path, self.context.profile.field_kind(path)))
            .collect();
        let right = columns
            .iter()
            .enumerate()
            .filter_map(|(index, path)| {
                is_numeric_kind(self.context.profile.field_kind(path)).then_some(index)
            })
            .collect::<Vec<_>>();
        self.grid(headers, rows, &right)
    }

    fn render_stacked(&mut self, objects: &[Vec<(String, Value)>], columns: &[String]) -> String {
        let mut blocks = Vec::with_capacity(objects.len());
        for (record, object) in objects.iter().enumerate() {
            let natural_field_width = columns
                .iter()
                .map(|path| display_width(&header(path, self.context.profile.field_kind(path))))
                .max()
                .unwrap_or(5);
            let (field_width, value_width) =
                two_column_widths(natural_field_width, self.context.width);
            let mut rows = Vec::with_capacity(columns.len());
            for path in columns {
                let kind = self.context.profile.field_kind(path);
                let raw = object
                    .iter()
                    .find_map(|(candidate, value)| (candidate == path).then_some(value))
                    .map(|value| format_value(value, kind))
                    .unwrap_or_else(|| "missing".into());
                let (value, cut) = truncate_for_kind(&raw, value_width, kind, self.context.unicode);
                self.truncated += usize::from(cut);
                let (field, cut) =
                    truncate_middle(&header(path, kind), field_width, self.context.unicode);
                self.truncated += usize::from(cut);
                rows.push(vec![field, value]);
            }
            blocks.push(self.grid_with_panel(
                vec!["field".into(), "value".into()],
                rows,
                &[],
                format!("record={record}"),
            ));
        }
        blocks.join("\n\n")
    }

    fn render_scalar_list(&mut self, values: &[Value]) -> String {
        let value_width = self.context.width.saturating_sub(14).max(4);
        let rows = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let raw = format_value(value, FieldKind::Text);
                let (value, cut) = truncate_tail(&raw, value_width, self.context.unicode);
                self.truncated += usize::from(cut);
                vec![index.to_string(), value]
            })
            .collect();
        self.grid(vec!["index".into(), "value".into()], rows, &[0])
    }

    fn render_heterogeneous(&mut self, values: &[Value]) -> String {
        let object_columns = ordered_columns(
            values
                .iter()
                .filter_map(Value::as_object)
                .flat_map(|object| {
                    flatten_object(object)
                        .into_iter()
                        .map(|(path, _)| path)
                        .collect::<Vec<_>>()
                }),
            self.context.profile,
        );
        let mut columns = vec!["index".into(), "kind".into(), "value".into()];
        columns.extend(object_columns.iter().cloned());
        let objects = values
            .iter()
            .map(|value| value.as_object().map(flatten_object).unwrap_or_default())
            .collect::<Vec<_>>();
        if self.should_stack(&columns, &objects) {
            let normalized = values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    let mut fields = vec![
                        ("index".into(), Value::from(index as u64)),
                        (
                            "kind".into(),
                            Value::String(
                                if value.is_object() {
                                    "object"
                                } else {
                                    "scalar"
                                }
                                .into(),
                            ),
                        ),
                    ];
                    if let Some(object) = value.as_object() {
                        fields.extend(flatten_object(object));
                    } else {
                        fields.push(("value".into(), value.clone()));
                    }
                    fields
                })
                .collect::<Vec<_>>();
            return self.render_stacked(&normalized, &columns);
        }
        let limits = [8, 8, 20];
        let rows = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let mut row = vec![
                    index.to_string(),
                    if value.is_object() {
                        "object"
                    } else {
                        "scalar"
                    }
                    .into(),
                    if value.is_object() {
                        "missing".into()
                    } else {
                        truncate_tail(&compact_json(value), limits[2], self.context.unicode).0
                    },
                ];
                let flattened = value.as_object().map(flatten_object).unwrap_or_default();
                for path in &object_columns {
                    row.push(
                        flattened
                            .iter()
                            .find_map(|(candidate, value)| {
                                (candidate == path).then(|| compact_json(value))
                            })
                            .unwrap_or_else(|| "missing".into()),
                    );
                }
                row
            })
            .collect();
        self.grid(columns, rows, &[0])
    }

    fn render_time_series(&mut self, values: &[Value]) -> String {
        let normalized = values
            .iter()
            .map(|value| {
                let pair = value.as_array().expect("time-series shape checked");
                let mut object = Map::new();
                object.insert("timestamp".into(), pair[0].clone());
                object.insert("value".into(), pair[1].clone());
                if let Some(resolution) = pair.get(2) {
                    object.insert("resolution".into(), resolution.clone());
                }
                Value::Object(object)
            })
            .collect::<Vec<_>>();
        self.render_object_list(&normalized)
    }

    fn render_empty(&mut self) -> String {
        let headers = if self.context.profile == TableProfile::Generic {
            vec!["value".into()]
        } else {
            self.context
                .profile
                .preferred_columns()
                .iter()
                .map(|path| header(path, self.context.profile.field_kind(path)))
                .collect()
        };
        let natural_width = headers
            .iter()
            .map(|value| display_width(value).max(9))
            .sum::<usize>()
            + 3 * headers.len()
            + 1;
        if natural_width > self.context.width {
            if headers
                .iter()
                .all(|value| display_width(value) + 4 <= self.context.width)
            {
                let mut groups = Vec::<Vec<String>>::new();
                for header in headers {
                    let candidate_width = groups
                        .last()
                        .into_iter()
                        .flatten()
                        .chain(std::iter::once(&header))
                        .map(|value| display_width(value).max(9))
                        .sum::<usize>()
                        + 3 * (groups.last().map(Vec::len).unwrap_or(0) + 1)
                        + 1;
                    if candidate_width > self.context.width {
                        groups.push(vec![header]);
                    } else if let Some(group) = groups.last_mut() {
                        group.push(header);
                    } else {
                        groups.push(vec![header]);
                    }
                }
                return groups
                    .into_iter()
                    .map(|headers| {
                        self.grid(
                            headers.clone(),
                            vec![vec!["<no rows>".into(); headers.len()]],
                            &[],
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
            }
            let natural_field_width = headers
                .iter()
                .map(|value| display_width(value))
                .max()
                .unwrap_or(5);
            let (field_width, value_width) =
                two_column_widths(natural_field_width, self.context.width);
            let rows = headers
                .into_iter()
                .map(|field| {
                    let (field, cut) = truncate_middle(&field, field_width, self.context.unicode);
                    self.truncated += usize::from(cut);
                    let (value, cut) =
                        truncate_tail("<no rows>", value_width, self.context.unicode);
                    self.truncated += usize::from(cut);
                    vec![field, value]
                })
                .collect();
            return self.grid(vec!["field".into(), "value".into()], rows, &[]);
        }
        self.grid(
            headers.clone(),
            vec![vec!["<no rows>".into(); headers.len()]],
            &[],
        )
    }

    fn render_scalar(&mut self, value: &Value) -> String {
        let kind = self.context.profile.field_kind("result");
        let raw = format_value(value, kind);
        let value_width = self.context.width.saturating_sub(14).max(4);
        let (value, cut) = truncate_for_kind(&raw, value_width, kind, self.context.unicode);
        self.truncated += usize::from(cut);
        self.grid(
            vec!["field".into(), "value".into()],
            vec![vec!["result".into(), value]],
            &[],
        )
    }

    fn render_order_book(&mut self, payload: &Value) -> String {
        let Some(object) = payload.as_object() else {
            return self.render_shape(payload);
        };
        let metadata = object
            .iter()
            .filter(|(key, _)| !matches!(key.as_str(), "asks" | "bids"))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Map<_, _>>();
        let mut sections = vec![self.render_field_value(&metadata)];
        for (name, label) in [("asks", "ASK"), ("bids", "BID")] {
            let mut rows = Vec::new();
            let mut objects = Vec::new();
            if let Some(levels) = object.get(name).and_then(Value::as_array) {
                for level in levels {
                    let values = level.as_array();
                    let price = values
                        .and_then(|values| values.first())
                        .map(|value| format_value(value, FieldKind::Price))
                        .unwrap_or_else(|| "missing".into());
                    let amount = values
                        .and_then(|values| values.get(1))
                        .map(|value| format_value(value, FieldKind::Number))
                        .unwrap_or_else(|| "missing".into());
                    let count = values
                        .and_then(|values| values.get(2))
                        .map(|value| format_value(value, FieldKind::Number))
                        .unwrap_or_else(|| "missing".into());
                    let raw = values
                        .filter(|values| values.len() > 3)
                        .map(|values| compact_json(&Value::Array(values[3..].to_vec())))
                        .unwrap_or_else(|| "missing".into());
                    rows.push(vec![label.into(), price, amount, count, raw]);
                    let mut object = vec![("side".into(), Value::String(label.into()))];
                    if let Some(value) = values.and_then(|values| values.first()) {
                        object.push(("price".into(), value.clone()));
                    }
                    if let Some(value) = values.and_then(|values| values.get(1)) {
                        object.push(("amount".into(), value.clone()));
                    }
                    if let Some(value) = values.and_then(|values| values.get(2)) {
                        object.push(("count".into(), value.clone()));
                    }
                    if let Some(values) = values.filter(|values| values.len() > 3) {
                        object.push(("raw".into(), Value::Array(values[3..].to_vec())));
                    }
                    objects.push(object);
                }
            }
            if rows.is_empty() {
                rows.push(vec!["<no rows>".into(); 5]);
            }
            let columns = ["side", "price", "amount", "count", "raw"]
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let table = if !objects.is_empty() && self.should_stack(&columns, &objects) {
                self.render_stacked(&objects, &columns)
            } else {
                self.grid(columns, rows, &[1, 2, 3])
            };
            sections.push(format!("{name}\n{table}"));
        }
        sections.join("\n\n")
    }

    fn is_time_series(&self, values: &[Value]) -> bool {
        matches!(
            self.context.profile,
            TableProfile::Volatility | TableProfile::Funding
        ) && !values.is_empty()
            && values.iter().all(|value| {
                value
                    .as_array()
                    .is_some_and(|pair| pair.len() >= 2 && pair[0].is_number())
            })
    }

    fn should_stack(&self, columns: &[String], objects: &[Vec<(String, Value)>]) -> bool {
        if columns.is_empty() {
            return false;
        }
        let natural = columns
            .iter()
            .map(|column| {
                let kind = self.context.profile.field_kind(column);
                objects
                    .iter()
                    .flat_map(|object| object.iter())
                    .filter(|(path, _)| path == column)
                    .map(|(_, value)| display_width(&format_value(value, kind)))
                    .max()
                    .unwrap_or(0)
                    .min(class_limit(kind))
                    .max(display_width(&header(column, kind)))
                    .max(4)
            })
            .sum::<usize>()
            + 3 * columns.len()
            + 1;
        natural > self.context.width
            || columns.iter().any(|column| {
                display_width(&header(column, self.context.profile.field_kind(column)))
                    > self.context.width.saturating_sub(11)
            })
    }

    fn grid(&self, headers: Vec<String>, rows: Vec<Vec<String>>, right: &[usize]) -> String {
        self.grid_inner(headers, rows, right, None)
    }

    fn grid_with_panel(
        &self,
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
        right: &[usize],
        panel: String,
    ) -> String {
        self.grid_inner(headers, rows, right, Some(panel))
    }

    fn grid_inner(
        &self,
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
        right: &[usize],
        panel: Option<String>,
    ) -> String {
        let mut builder = Builder::default();
        builder.push_record(headers);
        for row in rows {
            builder.push_record(row);
        }
        let mut table = builder.build();
        if self.context.unicode {
            table.with(Style::modern());
        } else {
            table.with(Style::ascii());
        }
        for index in right {
            table.modify(Columns::one(*index), Alignment::right());
        }
        if let Some(panel) = panel {
            table.with(Panel::header(panel));
        }
        table.to_string()
    }
}

fn flatten_object(object: &Map<String, Value>) -> Vec<(String, Value)> {
    let mut output = Vec::new();
    for (key, value) in object {
        if let Value::Object(nested) = value {
            if nested.is_empty() {
                output.push((key.clone(), value.clone()));
            } else {
                for (nested_key, nested_value) in nested {
                    output.push((format!("{key}.{nested_key}"), nested_value.clone()));
                }
            }
        } else {
            output.push((key.clone(), value.clone()));
        }
    }
    output
}

fn unknown_unit_fields(payload: &Value, profile: TableProfile) -> BTreeSet<String> {
    let mut fields = BTreeSet::new();
    collect_unknown_unit_fields(payload, profile, None, &mut fields);
    fields
}

fn collect_unknown_unit_fields(
    value: &Value,
    profile: TableProfile,
    path: Option<&str>,
    fields: &mut BTreeSet<String>,
) {
    match value {
        Value::Number(_) => {
            let path = path.unwrap_or("value");
            if profile.field_kind(path) == FieldKind::Text {
                fields.insert(path.to_owned());
            }
        }
        Value::Object(object) => {
            for (path, value) in flatten_object(object) {
                if ["complete", "truncated", "coverage", "warnings"]
                    .iter()
                    .any(|key| path == *key || path.starts_with(&format!("{key}.")))
                {
                    continue;
                }
                if profile == TableProfile::OrderBook && matches!(path.as_str(), "asks" | "bids") {
                    continue;
                }
                collect_unknown_unit_fields(&value, profile, Some(&path), fields);
            }
        }
        Value::Array(values)
            if matches!(profile, TableProfile::Volatility | TableProfile::Funding)
                && values.iter().all(|value| value.is_array()) => {}
        Value::Array(values) => {
            for value in values {
                collect_unknown_unit_fields(value, profile, path, fields);
            }
        }
        Value::Null | Value::Bool(_) | Value::String(_) => {}
    }
}

fn presentation_status(payload: &Value, unknown_units: &BTreeSet<String>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(object) = payload.as_object() {
        for key in ["complete", "truncated", "coverage", "warnings"] {
            if let Some(value) = object.get(key) {
                parts.push(format!("{key}={}", compact_json(value)));
            }
        }
    }
    if !unknown_units.is_empty() {
        parts.push(format!(
            "unit=unknown fields={}",
            unknown_units.iter().cloned().collect::<Vec<_>>().join(",")
        ));
    }
    if parts.is_empty() {
        None
    } else if parts.len() == 1 && parts[0].starts_with("unit=unknown") {
        Some(format!("warning: {}", parts[0]))
    } else {
        Some(format!("status: {}", parts.join("; ")))
    }
}

fn ordered_columns(
    fields: impl Iterator<Item = impl AsRef<str>>,
    profile: TableProfile,
) -> Vec<String> {
    let available = fields
        .map(|field| field.as_ref().to_owned())
        .collect::<BTreeSet<_>>();
    let mut columns = Vec::with_capacity(available.len());
    for preferred in profile.preferred_columns() {
        if available.contains(*preferred) {
            columns.push((*preferred).to_owned());
        }
    }
    let remaining = available
        .into_iter()
        .filter(|field| !columns.contains(field))
        .collect::<Vec<_>>();
    columns.extend(remaining);
    columns
}

fn two_column_widths(natural_field_width: usize, total_width: usize) -> (usize, usize) {
    let available = total_width.saturating_sub(7);
    let field_width = natural_field_width
        .clamp(5, 28)
        .min(available.saturating_sub(5));
    (field_width, available.saturating_sub(field_width).max(5))
}

fn allocate_grid_widths(
    columns: &[String],
    objects: &[Vec<(String, Value)>],
    context: &PresentationContext,
) -> Vec<usize> {
    let available = context
        .width
        .saturating_sub(3 * columns.len() + 1)
        .max(4 * columns.len());
    let mut widths = columns
        .iter()
        .map(|column| {
            let kind = context.profile.field_kind(column);
            objects
                .iter()
                .flat_map(|object| object.iter())
                .filter(|(path, _)| path == column)
                .map(|(_, value)| display_width(&format_value(value, kind)))
                .max()
                .unwrap_or(0)
                .min(class_limit(kind))
                .max(display_width(&header(column, kind)))
                .max(4)
        })
        .collect::<Vec<_>>();
    while widths.iter().sum::<usize>() > available {
        let Some((index, _)) = widths
            .iter()
            .enumerate()
            .filter(|(_, width)| **width > 4)
            .max_by_key(|(_, width)| **width)
        else {
            break;
        };
        widths[index] -= 1;
    }
    widths
}

fn class_limit(kind: FieldKind) -> usize {
    match kind {
        FieldKind::Number
        | FieldKind::Price
        | FieldKind::PercentValue
        | FieldKind::PercentFraction => 18,
        FieldKind::TimestampMillis => 24,
        FieldKind::Enum => 16,
        FieldKind::Identifier => 28,
        FieldKind::FreeText => 36,
        FieldKind::EmbeddedJson => 40,
        FieldKind::Text => 36,
    }
}

fn header(path: &str, kind: FieldKind) -> String {
    if kind == FieldKind::TimestampMillis {
        format!("{path} (UTC)")
    } else {
        path.to_owned()
    }
}

fn format_value(value: &Value, kind: FieldKind) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::String(value) if kind == FieldKind::Enum && value == "call" => "CALL".into(),
        Value::String(value) if kind == FieldKind::Enum && value == "put" => "PUT".into(),
        Value::String(value) if value.is_empty() => "\"\"".into(),
        Value::String(value) => value.clone(),
        Value::Number(number) => format_number(number, kind),
        Value::Array(values) if values.is_empty() => "[]".into(),
        Value::Object(object) if object.is_empty() => "{}".into(),
        Value::Array(_) | Value::Object(_) => compact_json(value),
    }
}

fn format_number(number: &Number, kind: FieldKind) -> String {
    let text = number.to_string();
    if matches!(text.as_str(), "-0" | "-0.0") {
        return "0".into();
    }
    match kind {
        FieldKind::TimestampMillis => text
            .parse::<i64>()
            .ok()
            .map(format_timestamp_millis)
            .unwrap_or(text),
        FieldKind::PercentFraction => {
            let shifted = shift_decimal(&text, 2);
            let (rounded, approximate) = round_decimal(&shifted, 4);
            format!(
                "{}{}%",
                if approximate { "≈" } else { "" },
                pad_decimal(&rounded, 4)
            )
        }
        FieldKind::PercentValue => {
            let (rounded, approximate) = round_decimal(&text, 4);
            format!("{}{}%", if approximate { "≈" } else { "" }, rounded)
        }
        FieldKind::Price => {
            let (rounded, approximate) = round_decimal_preserving_scale(&text, 12);
            format!("{}{}", if approximate { "≈" } else { "" }, rounded)
        }
        _ if number.is_i64() || number.is_u64() => text,
        _ => {
            let (rounded, approximate) = round_decimal(&text, 8);
            format!("{}{}", if approximate { "≈" } else { "" }, rounded)
        }
    }
}

fn round_decimal_preserving_scale(input: &str, places: usize) -> (String, bool) {
    if input.contains(['e', 'E']) {
        return round_decimal(input, places);
    }
    let Some(dot) = input.find('.') else {
        return (input.to_owned(), false);
    };
    let decimals = input.len() - dot - 1;
    if decimals <= places {
        return (input.to_owned(), false);
    }
    let (rounded, approximate) = round_decimal(input, places);
    (pad_decimal(&rounded, places), approximate)
}

fn pad_decimal(value: &str, places: usize) -> String {
    let Some((integer, fraction)) = value.split_once('.') else {
        return format!("{value}.{}", "0".repeat(places));
    };
    format!("{integer}.{fraction:0<places$}")
}

fn round_decimal(input: &str, places: usize) -> (String, bool) {
    if input.contains(['e', 'E']) {
        return input
            .parse::<f64>()
            .ok()
            .map(|value| (format!("{value:.5e}"), true))
            .unwrap_or_else(|| (input.to_owned(), false));
    }
    let Some(dot) = input.find('.') else {
        return (input.to_owned(), false);
    };
    let decimals = input.len() - dot - 1;
    if decimals <= places {
        return (trim_decimal(input), false);
    }
    let keep = dot + 1 + places;
    let mut prefix = input.as_bytes()[..keep].to_vec();
    let dropped = &input.as_bytes()[keep..];
    if dropped.iter().all(|byte| *byte == b'0') {
        return (trim_decimal(input), false);
    }
    let last_kept = prefix
        .iter()
        .rev()
        .find(|byte| byte.is_ascii_digit())
        .copied()
        .unwrap_or(b'0');
    let round_up = dropped[0] > b'5'
        || (dropped[0] == b'5'
            && (dropped[1..].iter().any(|byte| *byte != b'0') || (last_kept - b'0') % 2 == 1));
    if round_up {
        let mut cursor = prefix.len();
        loop {
            if cursor == 0 {
                prefix.insert(0, b'1');
                break;
            }
            cursor -= 1;
            if prefix[cursor] == b'.' || prefix[cursor] == b'-' {
                continue;
            }
            if prefix[cursor] == b'9' {
                prefix[cursor] = b'0';
            } else {
                prefix[cursor] += 1;
                break;
            }
        }
    }
    let rounded = String::from_utf8(prefix).expect("decimal prefix remains ASCII");
    (trim_decimal(&rounded), true)
}

fn trim_decimal(value: &str) -> String {
    if !value.contains('.') {
        return value.to_owned();
    }
    let trimmed = value.trim_end_matches('0').trim_end_matches('.');
    if matches!(trimmed, "-0" | "") {
        "0".into()
    } else {
        trimmed.into()
    }
}

fn shift_decimal(value: &str, places: usize) -> String {
    if value.contains(['e', 'E']) {
        return value
            .parse::<f64>()
            .map(|number| (number * 100_f64.powi(places as i32)).to_string())
            .unwrap_or_else(|_| value.to_owned());
    }
    let negative = value.starts_with('-');
    let value = value.strip_prefix('-').unwrap_or(value);
    let (integer, fraction) = value.split_once('.').unwrap_or((value, ""));
    let mut digits = format!("{integer}{fraction}");
    let decimal = integer.len() + places;
    while digits.len() < decimal {
        digits.push('0');
    }
    let shifted = if decimal >= digits.len() {
        digits
    } else {
        format!("{}.{}", &digits[..decimal], &digits[decimal..])
    };
    let shifted = if let Some((integer, fraction)) = shifted.split_once('.') {
        let integer = integer.trim_start_matches('0');
        format!(
            "{}.{}",
            if integer.is_empty() { "0" } else { integer },
            fraction
        )
    } else {
        shifted.trim_start_matches('0').to_owned()
    };
    let shifted = if shifted.starts_with('.') {
        format!("0{shifted}")
    } else if shifted.is_empty() {
        "0".to_owned()
    } else {
        shifted
    };
    format!(
        "{}{}",
        if negative && shifted != "0" { "-" } else { "" },
        shifted
    )
}

fn format_timestamp_millis(milliseconds: i64) -> String {
    let seconds = milliseconds.div_euclid(1000);
    let millis = milliseconds.rem_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = second_of_day / 3_600;
    let minute = (second_of_day % 3_600) / 60;
    let second = second_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

fn compact_json(value: &Value) -> String {
    match value {
        Value::Object(object) => {
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(left, _)| *left);
            let body = entries
                .into_iter()
                .map(|(key, value)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("string serialization cannot fail"),
                        compact_json(value)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(compact_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => serde_json::to_string(value).expect("native JSON value serialization cannot fail"),
    }
}

fn truncate_for_kind(value: &str, width: usize, kind: FieldKind, unicode: bool) -> (String, bool) {
    if kind == FieldKind::Identifier {
        truncate_middle(value, width, unicode)
    } else {
        truncate_tail(value, width, unicode)
    }
}

fn truncate_tail(value: &str, width: usize, unicode: bool) -> (String, bool) {
    if display_width(value) <= width {
        return (value.to_owned(), false);
    }
    let marker = if unicode { "…" } else { "..." };
    let target = width.saturating_sub(display_width(marker));
    let prefix = take_display_prefix(value, target);
    (format!("{prefix}{marker}"), true)
}

fn truncate_middle(value: &str, width: usize, unicode: bool) -> (String, bool) {
    if display_width(value) <= width {
        return (value.to_owned(), false);
    }
    let marker = if unicode { "…" } else { "..." };
    let available = width.saturating_sub(display_width(marker));
    let left_width = available.div_ceil(2);
    let right_width = available / 2;
    let left = take_display_prefix(value, left_width);
    let right = take_display_suffix(value, right_width);
    (format!("{left}{marker}{right}"), true)
}

fn take_display_prefix(value: &str, target: usize) -> String {
    let mut width = 0;
    value
        .chars()
        .take_while(|character| {
            let next = width + char_width(*character);
            if next <= target {
                width = next;
                true
            } else {
                false
            }
        })
        .collect()
}

fn take_display_suffix(value: &str, target: usize) -> String {
    let mut width = 0;
    let mut characters = value
        .chars()
        .rev()
        .take_while(|character| {
            let next = width + char_width(*character);
            if next <= target {
                width = next;
                true
            } else {
                false
            }
        })
        .collect::<Vec<_>>();
    characters.reverse();
    characters.into_iter().collect()
}

/// Stable display width used by renderer and P01 line-width assertions.
pub fn display_width(value: &str) -> usize {
    value.chars().map(char_width).sum()
}

fn char_width(character: char) -> usize {
    let value = character as u32;
    if character.is_control()
        || (0x0300..=0x036f).contains(&value)
        || (0x1ab0..=0x1aff).contains(&value)
        || (0x1dc0..=0x1dff).contains(&value)
        || (0x20d0..=0x20ff).contains(&value)
        || (0xfe20..=0xfe2f).contains(&value)
    {
        0
    } else if (0x1100..=0x115f).contains(&value)
        || (0x2e80..=0xa4cf).contains(&value)
        || (0xac00..=0xd7a3).contains(&value)
        || (0xf900..=0xfaff).contains(&value)
        || (0xfe10..=0xfe6f).contains(&value)
        || (0xff00..=0xff60).contains(&value)
        || (0x1f300..=0x1faff).contains(&value)
        || (0x20000..=0x3fffd).contains(&value)
    {
        2
    } else {
        1
    }
}

fn is_numeric_kind(kind: FieldKind) -> bool {
    matches!(
        kind,
        FieldKind::Number | FieldKind::Price | FieldKind::PercentValue | FieldKind::PercentFraction
    )
}

fn apply_color(text: String, unicode: bool) -> String {
    let mut output = String::new();
    let mut in_table = false;
    let mut waiting_for_header = false;
    for (index, line) in text.lines().enumerate() {
        let border = !line.is_empty()
            && line.chars().all(|character| {
                character.is_whitespace()
                    || "+-|".contains(character)
                    || "┌┐└┘├┤┬┴┼─│".contains(character)
            });
        if index == 0 {
            output.push_str("\x1b[1m");
            output.push_str(line);
            output.push_str("\x1b[0m");
        } else if border {
            if !in_table || line.starts_with('┌') {
                in_table = true;
                waiting_for_header = true;
            }
            output.push_str("\x1b[2m");
            output.push_str(line);
            output.push_str("\x1b[0m");
            if line.starts_with('└') {
                in_table = false;
                waiting_for_header = false;
            }
        } else {
            let verticals = line
                .chars()
                .filter(|character| "|│".contains(*character))
                .count();
            if waiting_for_header && verticals >= 3 {
                output.push_str("\x1b[1m");
                output.push_str(line);
                output.push_str("\x1b[0m");
                waiting_for_header = false;
            } else if line.starts_with("warning:") {
                output.push_str("\x1b[33m");
                output.push_str(line);
                output.push_str("\x1b[0m");
            } else {
                output.push_str(&colorize_tokens(line));
            }
            if line.is_empty() || (verticals == 0 && !line.starts_with("note:")) {
                in_table = false;
                waiting_for_header = false;
            }
        }
        output.push('\n');
    }
    if !unicode {
        debug_assert!(!output.contains('┌'));
    }
    output
}

fn colorize_tokens(line: &str) -> String {
    line.split(' ')
        .map(|token| match token {
            "CALL" => "\x1b[34mCALL\x1b[0m".into(),
            "PUT" => "\x1b[35mPUT\x1b[0m".into(),
            "null" | "missing" => format!("\x1b[2m{token}\x1b[0m"),
            _ if is_negative_number_token(token) => format!("\x1b[31m{token}\x1b[0m"),
            _ => token.into(),
        })
        .collect::<Vec<String>>()
        .join(" ")
}

fn is_negative_number_token(token: &str) -> bool {
    let number = token.strip_prefix('-').unwrap_or("");
    !number.is_empty()
        && number
            .strip_suffix('%')
            .unwrap_or(number)
            .chars()
            .all(|character| character.is_ascii_digit() || ".eE+".contains(character))
}
