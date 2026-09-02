//! A minimal JSON value type, parser, and serializer — scoped to exactly
//! what Ollama-shaped API request/response bodies need (flat-ish objects,
//! strings, numbers, bools, nested objects/arrays for `options`). Not a
//! general-purpose JSON library: no streaming, no arbitrary-precision
//! numbers, no comments/trailing-comma leniency.
//!
//! Hand-rolled rather than pulling in `serde_json` for the same reason the
//! rest of this project hand-rolls things it could reach for a crate for —
//! JSON parsing here is generic plumbing, not "the LLM technology", but
//! keeping the whole workspace dependency-free (`gguf`/`tensor-core`/`qwen2`
//! all have zero external dependencies) was a deliberate, consistent choice
//! worth continuing rather than breaking for this one crate.

use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    /// Insertion order preserved (Ollama-shaped payloads are small — a
    /// handful of keys — so linear `get()` is simpler and fast enough;
    /// `BTreeMap` would silently reorder keys in serialized output).
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        self.as_f64().filter(|n| *n >= 0.0 && n.fract() == 0.0).map(|n| n as u64)
    }

    pub fn object(entries: Vec<(&str, Json)>) -> Json {
        Json::Object(entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn str(s: impl Into<String>) -> Json {
        Json::String(s.into())
    }

    pub fn num(n: impl Into<f64>) -> Json {
        Json::Number(n.into())
    }

    /// Compact serialization (no pretty-printing — this only ever goes over
    /// the wire, never shown to a human directly).
    pub fn to_json_string(&self) -> String {
        let mut out = String::new();
        self.write_to(&mut out);
        out
    }

    fn write_to(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Number(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    let _ = write!(out, "{}", *n as i64);
                } else {
                    let _ = write!(out, "{n}");
                }
            }
            Json::String(s) => write_json_string(out, s),
            Json::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write_to(out);
                }
                out.push(']');
            }
            Json::Object(entries) => {
                out.push('{');
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_json_string(out, k);
                    out.push(':');
                    v.write_to(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[derive(Debug)]
pub struct JsonParseError {
    pub message: String,
    pub position: usize,
}

impl std::fmt::Display for JsonParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSON parse error at byte {}: {}", self.position, self.message)
    }
}
impl std::error::Error for JsonParseError {}

pub fn parse(input: &str) -> Result<Json, JsonParseError> {
    let bytes = input.as_bytes();
    let mut p = Parser { bytes, pos: 0 };
    p.skip_ws();
    let value = p.parse_value()?;
    p.skip_ws();
    if p.pos != bytes.len() {
        return Err(p.err("trailing data after JSON value"));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, message: &str) -> JsonParseError {
        JsonParseError { message: message.to_string(), position: self.pos }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, b: u8) -> Result<(), JsonParseError> {
        if self.peek() == Some(b) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.err(&format!("expected '{}'", b as char)))
        }
    }

    fn parse_value(&mut self) -> Result<Json, JsonParseError> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(Json::String(self.parse_string()?)),
            Some(b't') => self.parse_literal("true", Json::Bool(true)),
            Some(b'f') => self.parse_literal("false", Json::Bool(false)),
            Some(b'n') => self.parse_literal("null", Json::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.parse_number(),
            _ => Err(self.err("unexpected character, expected a JSON value")),
        }
    }

    fn parse_literal(&mut self, lit: &str, value: Json) -> Result<Json, JsonParseError> {
        if self.bytes[self.pos..].starts_with(lit.as_bytes()) {
            self.pos += lit.len();
            Ok(value)
        } else {
            Err(self.err(&format!("expected literal {lit:?}")))
        }
    }

    fn parse_object(&mut self) -> Result<Json, JsonParseError> {
        self.expect(b'{')?;
        let mut entries = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Object(entries));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            let value = self.parse_value()?;
            entries.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(self.err("expected ',' or '}' in object")),
            }
        }
        Ok(Json::Object(entries))
    }

    fn parse_array(&mut self) -> Result<Json, JsonParseError> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Array(items));
        }
        loop {
            let value = self.parse_value()?;
            items.push(value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(self.err("expected ',' or ']' in array")),
            }
        }
        Ok(Json::Array(items))
    }

    fn parse_string(&mut self) -> Result<String, JsonParseError> {
        self.expect(b'"')?;
        let mut s = String::new();
        loop {
            match self.peek() {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    self.pos += 1;
                    break;
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some(b'"') => {
                            s.push('"');
                            self.pos += 1;
                        }
                        Some(b'\\') => {
                            s.push('\\');
                            self.pos += 1;
                        }
                        Some(b'/') => {
                            s.push('/');
                            self.pos += 1;
                        }
                        Some(b'b') => {
                            s.push('\u{8}');
                            self.pos += 1;
                        }
                        Some(b'f') => {
                            s.push('\u{c}');
                            self.pos += 1;
                        }
                        Some(b'n') => {
                            s.push('\n');
                            self.pos += 1;
                        }
                        Some(b'r') => {
                            s.push('\r');
                            self.pos += 1;
                        }
                        Some(b't') => {
                            s.push('\t');
                            self.pos += 1;
                        }
                        Some(b'u') => {
                            self.pos += 1;
                            let cp = self.parse_hex4()?;
                            // Minimal surrogate-pair handling for characters
                            // outside the BMP (e.g. some emoji) — a lone
                            // high surrogate followed by \uXXXX low surrogate.
                            if (0xD800..=0xDBFF).contains(&cp) {
                                if self.bytes[self.pos..].starts_with(b"\\u") {
                                    self.pos += 2;
                                    let low = self.parse_hex4()?;
                                    let c = 0x10000 + (((cp - 0xD800) as u32) << 10) + (low - 0xDC00) as u32;
                                    if let Some(ch) = char::from_u32(c) {
                                        s.push(ch);
                                    }
                                } else {
                                    return Err(self.err("unpaired UTF-16 surrogate in \\u escape"));
                                }
                            } else if let Some(ch) = char::from_u32(cp as u32) {
                                s.push(ch);
                            }
                        }
                        _ => return Err(self.err("invalid escape sequence")),
                    }
                }
                Some(_) => {
                    // Copy one UTF-8 character's worth of bytes verbatim.
                    let rest = std::str::from_utf8(&self.bytes[self.pos..]).map_err(|_| self.err("invalid UTF-8 in string"))?;
                    let ch = rest.chars().next().unwrap();
                    s.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
        Ok(s)
    }

    fn parse_hex4(&mut self) -> Result<u16, JsonParseError> {
        if self.pos + 4 > self.bytes.len() {
            return Err(self.err("truncated \\u escape"));
        }
        let hex = std::str::from_utf8(&self.bytes[self.pos..self.pos + 4]).map_err(|_| self.err("invalid \\u escape"))?;
        let v = u16::from_str_radix(hex, 16).map_err(|_| self.err("invalid hex digits in \\u escape"))?;
        self.pos += 4;
        Ok(v)
    }

    fn parse_number(&mut self) -> Result<Json, JsonParseError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap();
        text.parse::<f64>().map(Json::Number).map_err(|_| self.err("invalid number"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ollama_shaped_generate_request() {
        let input = r#"{"model":"qwen2.5:0.5b","prompt":"2 + 2 equals","raw":true,"stream":false,"options":{"temperature":0,"num_predict":10}}"#;
        let v = parse(input).unwrap();
        assert_eq!(v.get("model").and_then(Json::as_str), Some("qwen2.5:0.5b"));
        assert_eq!(v.get("prompt").and_then(Json::as_str), Some("2 + 2 equals"));
        assert_eq!(v.get("raw").and_then(Json::as_bool), Some(true));
        assert_eq!(v.get("stream").and_then(Json::as_bool), Some(false));
        let options = v.get("options").unwrap();
        assert_eq!(options.get("num_predict").and_then(Json::as_u64), Some(10));
    }

    #[test]
    fn round_trips_escapes() {
        let s = "line1\nline2\t\"quoted\"\\backslash";
        let j = Json::str(s);
        let serialized = j.to_json_string();
        let parsed = parse(&serialized).unwrap();
        assert_eq!(parsed.as_str(), Some(s));
    }

    #[test]
    fn parses_unicode_escape() {
        let v = parse(r#""café""#).unwrap();
        assert_eq!(v.as_str(), Some("café"));
    }

    #[test]
    fn serializes_object_preserving_order() {
        let j = Json::object(vec![("b", Json::num(1.0)), ("a", Json::num(2.0))]);
        assert_eq!(j.to_json_string(), r#"{"b":1,"a":2}"#);
    }

    #[test]
    fn integers_serialize_without_decimal_point() {
        assert_eq!(Json::num(42.0).to_json_string(), "42");
        assert_eq!(Json::num(0.5).to_json_string(), "0.5");
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(parse(r#"{"a":1} garbage"#).is_err());
    }

    #[test]
    fn parses_nested_arrays_and_objects() {
        let v = parse(r#"{"a":[1,2,{"b":true,"c":null}]}"#).unwrap();
        let arr = v.get("a").unwrap();
        match arr {
            Json::Array(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[2].get("b").and_then(Json::as_bool), Some(true));
                assert_eq!(items[2].get("c"), Some(&Json::Null));
            }
            _ => panic!("expected array"),
        }
    }
}
