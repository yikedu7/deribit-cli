//! Strict parameter decoding for direct flags and explicit raw JSON sources.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

use serde_json::{Map, Number, Value};
use thiserror::Error;

use crate::{ErrorClass, ParameterDescriptor, ParameterType, ValidatedOperation};

/// Maximum raw parameter payload size, measured before UTF-8 decoding.
pub const MAX_PARAMETER_BYTES: usize = 1_048_576;
/// Maximum JSON object/array nesting depth.
pub const MAX_PARAMETER_DEPTH: usize = 64;

/// The one selected parameter source for a public operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParameterInput {
    /// Direct method flags, keyed by the canonical API parameter name.
    Typed(HashMap<String, String>),
    /// One inline JSON object.
    Inline(String),
    /// One explicitly named local file.
    File(PathBuf),
    /// The explicitly selected non-terminal standard input stream.
    Stdin,
}

/// Errors produced before any transport can be constructed.
#[derive(Debug, Error)]
pub enum ParameterError {
    #[error("parameter usage rejected: {0}")]
    Usage(String),
    #[error("parameter input rejected: {0}")]
    Input(String),
    #[error("could not read parameter file {path}: {source}")]
    LocalIo { path: PathBuf, source: io::Error },
}

impl ParameterError {
    /// Returns the frozen process-boundary classification.
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Usage(_) => ErrorClass::Usage,
            Self::Input(_) => ErrorClass::Input,
            Self::LocalIo { .. } => ErrorClass::LocalIo,
        }
    }
}

/// Resolves one selected source and validates it against the operation schema.
pub fn resolve_parameters(
    operation: ValidatedOperation<'_>,
    input: ParameterInput,
    stdin: &mut dyn Read,
    stdin_is_terminal: bool,
) -> Result<Value, ParameterError> {
    match input {
        ParameterInput::Typed(values) => decode_typed(operation, &values),
        ParameterInput::Inline(text) => decode_raw(operation, text.as_bytes(), false),
        ParameterInput::File(path) => {
            let bytes = read_limited_file(&path)?;
            decode_raw(operation, &bytes, false)
        }
        ParameterInput::Stdin => {
            if stdin_is_terminal {
                return Err(ParameterError::Input(
                    "--params-stdin requires a non-terminal stdin".into(),
                ));
            }
            let bytes = read_limited(stdin)?;
            decode_raw(operation, &bytes, true)
        }
    }
}

fn read_limited_file(path: &Path) -> Result<Vec<u8>, ParameterError> {
    let mut file = File::open(path).map_err(|source| ParameterError::LocalIo {
        path: path.to_owned(),
        source,
    })?;
    read_limited(&mut file).map_err(|error| match error {
        ParameterError::Input(message) => ParameterError::Input(message),
        ParameterError::Usage(message) => ParameterError::Usage(message),
        ParameterError::LocalIo { source, .. } => ParameterError::LocalIo {
            path: path.to_owned(),
            source,
        },
    })
}

fn read_limited(reader: &mut dyn Read) -> Result<Vec<u8>, ParameterError> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_PARAMETER_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|source| ParameterError::LocalIo {
            path: PathBuf::from("<stdin>"),
            source,
        })?;
    if bytes.len() > MAX_PARAMETER_BYTES {
        return Err(ParameterError::Input(format!(
            "raw parameters exceed {MAX_PARAMETER_BYTES} bytes"
        )));
    }
    Ok(bytes)
}

fn decode_typed(
    operation: ValidatedOperation<'_>,
    values: &HashMap<String, String>,
) -> Result<Value, ParameterError> {
    let descriptors = operation
        .parameters()
        .map(|descriptor| (descriptor.name(), descriptor))
        .collect::<HashMap<_, _>>();
    let mut object = Map::new();
    for (name, token) in values {
        let descriptor = descriptors
            .get(name.as_str())
            .copied()
            .ok_or_else(|| ParameterError::Usage(format!("unknown parameter flag for {name}")))?;
        object.insert(name.clone(), parse_typed_value(descriptor, token)?);
    }
    validate_object(operation, &object, true)?;
    Ok(Value::Object(object))
}

fn parse_typed_value(
    descriptor: ParameterDescriptor<'_>,
    token: &str,
) -> Result<Value, ParameterError> {
    if descriptor.nullable() && token == "null" {
        return Ok(Value::Null);
    }
    let value = match descriptor.value_type() {
        ParameterType::String => Value::String(token.to_owned()),
        ParameterType::Boolean => match token {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => return Err(usage_type(descriptor, token)),
        },
        ParameterType::Integer => {
            if !is_canonical_integer(token) {
                return Err(usage_type(descriptor, token));
            }
            let integer = token
                .parse::<i64>()
                .map_err(|_| usage_type(descriptor, token))?;
            Value::Number(integer.into())
        }
        ParameterType::Number => {
            let number = token
                .parse::<Number>()
                .map_err(|_| usage_type(descriptor, token))?;
            Value::Number(number)
        }
    };
    validate_known_value(descriptor, &value, true)?;
    Ok(value)
}

fn decode_raw(
    operation: ValidatedOperation<'_>,
    bytes: &[u8],
    allow_initial_utf8_bom: bool,
) -> Result<Value, ParameterError> {
    if bytes.len() > MAX_PARAMETER_BYTES {
        return Err(ParameterError::Input(format!(
            "raw parameters exceed {MAX_PARAMETER_BYTES} bytes"
        )));
    }
    let bytes = normalize_bom(bytes, allow_initial_utf8_bom)?;
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ParameterError::Input("raw parameters are not valid UTF-8".into()))?;
    if text.trim().is_empty() {
        return Err(ParameterError::Input(
            "raw parameters are empty or whitespace-only".into(),
        ));
    }
    JsonAudit::new(text).audit()?;
    let value: Value = serde_json::from_str(text)
        .map_err(|error| ParameterError::Input(format!("invalid JSON: {error}")))?;
    let object = value.as_object().ok_or_else(|| {
        ParameterError::Input("raw parameters must be one top-level JSON object".into())
    })?;
    validate_object(operation, object, false)?;
    Ok(value)
}

fn normalize_bom(bytes: &[u8], allow_initial_utf8_bom: bool) -> Result<&[u8], ParameterError> {
    const UTF8_BOM: &[u8] = &[0xef, 0xbb, 0xbf];
    if bytes.starts_with(&[0xff, 0xfe])
        || bytes.starts_with(&[0xfe, 0xff])
        || bytes.starts_with(&[0x00, 0x00, 0xfe, 0xff])
        || bytes.starts_with(&[0xff, 0xfe, 0x00, 0x00])
    {
        return Err(ParameterError::Input(
            "UTF-16/UTF-32 BOM is not accepted".into(),
        ));
    }
    let normalized = if allow_initial_utf8_bom && bytes.starts_with(UTF8_BOM) {
        &bytes[UTF8_BOM.len()..]
    } else {
        bytes
    };
    if normalized
        .windows(UTF8_BOM.len())
        .any(|part| part == UTF8_BOM)
    {
        return Err(ParameterError::Input(
            "UTF-8 BOM is only allowed once at the start of stdin".into(),
        ));
    }
    Ok(normalized)
}

fn validate_object(
    operation: ValidatedOperation<'_>,
    object: &Map<String, Value>,
    direct_flags: bool,
) -> Result<(), ParameterError> {
    for descriptor in operation.parameters() {
        match object.get(descriptor.name()) {
            Some(value) => validate_known_value(descriptor, value, direct_flags)?,
            None if descriptor.required() => {
                return Err(ParameterError::Usage(format!(
                    "missing required parameter {} ({})",
                    descriptor.name(),
                    descriptor.flag()
                )));
            }
            None => {}
        }
    }
    Ok(())
}

fn validate_known_value(
    descriptor: ParameterDescriptor<'_>,
    value: &Value,
    direct_flag: bool,
) -> Result<(), ParameterError> {
    let invalid = || {
        let message = format!(
            "parameter {} has the wrong type or value: {}",
            descriptor.name(),
            value
        );
        if direct_flag {
            ParameterError::Usage(message)
        } else {
            ParameterError::Input(message)
        }
    };
    if value.is_null() {
        return if descriptor.nullable() {
            Ok(())
        } else {
            Err(invalid())
        };
    }
    let type_ok = match descriptor.value_type() {
        ParameterType::String => value.is_string(),
        ParameterType::Integer => value
            .as_number()
            .is_some_and(|number| is_json_integer_number(&number.to_string())),
        ParameterType::Number => value.is_number(),
        ParameterType::Boolean => value.is_boolean(),
    };
    if !type_ok
        || (!descriptor.enum_values().is_empty() && !descriptor.enum_values().contains(value))
    {
        return Err(invalid());
    }
    if descriptor.value_type() == ParameterType::Integer {
        let integer = value
            .as_number()
            .expect("integer type check above guarantees a number")
            .to_string();
        if descriptor.minimum().is_some_and(|minimum| {
            compare_integer_text(&integer, &minimum.to_string()) == Ordering::Less
        }) || descriptor.maximum().is_some_and(|maximum| {
            compare_integer_text(&integer, &maximum.to_string()) == Ordering::Greater
        }) {
            return Err(invalid());
        }
    }
    Ok(())
}

fn usage_type(descriptor: ParameterDescriptor<'_>, token: &str) -> ParameterError {
    ParameterError::Usage(format!(
        "{} rejects direct value {token:?}",
        descriptor.flag()
    ))
}

fn is_canonical_integer(value: &str) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'))
        && value != "-0"
}

fn is_json_integer_number(value: &str) -> bool {
    !value.contains(['.', 'e', 'E'])
}

fn compare_integer_text(left: &str, right: &str) -> Ordering {
    let (left_negative, left_digits) = integer_parts(left);
    let (right_negative, right_digits) = integer_parts(right);
    match (left_negative, right_negative) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => compare_magnitude(left_digits, right_digits),
        (true, true) => compare_magnitude(right_digits, left_digits),
    }
}

fn integer_parts(value: &str) -> (bool, &str) {
    let negative = value.starts_with('-') && value != "-0";
    let digits = value.strip_prefix('-').unwrap_or(value);
    let normalized = digits.trim_start_matches('0');
    (
        negative,
        if normalized.is_empty() {
            "0"
        } else {
            normalized
        },
    )
}

fn compare_magnitude(left: &str, right: &str) -> Ordering {
    left.len()
        .cmp(&right.len())
        .then_with(|| left.as_bytes().cmp(right.as_bytes()))
}

struct JsonAudit<'a> {
    input: &'a str,
    cursor: usize,
}

impl<'a> JsonAudit<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, cursor: 0 }
    }

    fn audit(mut self) -> Result<(), ParameterError> {
        self.skip_whitespace();
        self.parse_value(0)?;
        self.skip_whitespace();
        if self.cursor != self.input.len() {
            return self.reject("trailing data after JSON document");
        }
        Ok(())
    }

    fn parse_value(&mut self, container_depth: usize) -> Result<(), ParameterError> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'{') => self.parse_object(container_depth),
            Some(b'[') => self.parse_array(container_depth),
            Some(b'"') => self.parse_string().map(|_| ()),
            Some(b't') => self.consume_literal("true"),
            Some(b'f') => self.consume_literal("false"),
            Some(b'n') => self.consume_literal("null"),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            _ => self.reject("invalid JSON value"),
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<(), ParameterError> {
        let depth = self.next_depth(depth)?;
        self.cursor += 1;
        self.skip_whitespace();
        if self.consume_if(b'}') {
            return Ok(());
        }
        let mut keys = HashSet::new();
        loop {
            self.skip_whitespace();
            let key = self.parse_string()?;
            if !keys.insert(key.clone()) {
                return self.reject(&format!("duplicate JSON object key {key:?}"));
            }
            self.skip_whitespace();
            self.expect(b':')?;
            self.parse_value(depth)?;
            self.skip_whitespace();
            if self.consume_if(b'}') {
                return Ok(());
            }
            self.expect(b',')?;
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<(), ParameterError> {
        let depth = self.next_depth(depth)?;
        self.cursor += 1;
        self.skip_whitespace();
        if self.consume_if(b']') {
            return Ok(());
        }
        loop {
            self.parse_value(depth)?;
            self.skip_whitespace();
            if self.consume_if(b']') {
                return Ok(());
            }
            self.expect(b',')?;
        }
    }

    fn parse_string(&mut self) -> Result<String, ParameterError> {
        let start = self.cursor;
        self.expect(b'"')?;
        let mut escaped = false;
        while let Some(byte) = self.peek() {
            self.cursor += 1;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                return serde_json::from_str(&self.input[start..self.cursor]).map_err(|error| {
                    ParameterError::Input(format!("invalid JSON string: {error}"))
                });
            }
        }
        self.reject("unterminated JSON string")
    }

    fn parse_number(&mut self) -> Result<(), ParameterError> {
        let start = self.cursor;
        while matches!(
            self.peek(),
            Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
        ) {
            self.cursor += 1;
        }
        serde_json::from_str::<Number>(&self.input[start..self.cursor])
            .map(|_| ())
            .map_err(|error| ParameterError::Input(format!("invalid JSON number: {error}")))
    }

    fn consume_literal(&mut self, literal: &str) -> Result<(), ParameterError> {
        if self.input[self.cursor..].starts_with(literal) {
            self.cursor += literal.len();
            Ok(())
        } else {
            self.reject("invalid JSON literal")
        }
    }

    fn next_depth(&self, depth: usize) -> Result<usize, ParameterError> {
        let next = depth + 1;
        if next > MAX_PARAMETER_DEPTH {
            Err(ParameterError::Input(format!(
                "JSON nesting exceeds {MAX_PARAMETER_DEPTH} containers"
            )))
        } else {
            Ok(next)
        }
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.cursor += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), ParameterError> {
        if self.consume_if(expected) {
            Ok(())
        } else {
            self.reject(&format!("expected JSON byte {:?}", expected as char))
        }
    }

    fn consume_if(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.as_bytes().get(self.cursor).copied()
    }

    fn reject<T>(&self, message: &str) -> Result<T, ParameterError> {
        Err(ParameterError::Input(message.to_owned()))
    }
}
