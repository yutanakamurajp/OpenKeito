//! Multi-error JSON loading for models: scans the raw JSON for positions and
//! duplicate keys, parses into `serde_json::Value`, then deserializes each
//! section and item separately so every E001 error in the document is returned
//! together with both its line/column and its JSON path.

use std::collections::{HashMap, HashSet};

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::components::Component;
use crate::controls::Control;
use crate::diagnostic::{self, unknown_name, Diagnostic, DocumentKind};
use crate::model::{
    version_at_least, FrequencyScan, Initialization, LinearSolver, Method, Model, Output,
    Partitioning, Simulation, Switching, INTERPOLATED_BY_DEFAULT,
};

#[derive(Debug, Clone, Copy)]
struct Loc {
    key: Option<(usize, usize)>,
    start: (usize, usize),
    end: (usize, usize),
}

#[derive(Debug, Clone)]
struct DuplicateKey {
    json_path: String,
    key: String,
    line: usize,
    column: usize,
}

struct Scan {
    locs: HashMap<String, Loc>,
    duplicates: Vec<DuplicateKey>,
}

struct Scanner<'a> {
    src: &'a str,
    bytes: &'a [u8],
    idx: usize,
    line: usize,
    col: usize,
    locs: HashMap<String, Loc>,
    duplicates: Vec<DuplicateKey>,
}

impl<'a> Scanner<'a> {
    fn new(src: &'a str) -> Self {
        Scanner {
            src,
            bytes: src.as_bytes(),
            idx: 0,
            line: 1,
            col: 1,
            locs: HashMap::new(),
            duplicates: Vec::new(),
        }
    }

    #[inline]
    fn pos(&self) -> (usize, usize) {
        (self.line, self.col)
    }

    #[inline]
    fn bump(&mut self) -> Option<u8> {
        let b = *self.bytes.get(self.idx)?;
        self.idx += 1;
        if b == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(b)
    }

    fn skip_ws(&mut self) {
        while let Some(&b) = self.bytes.get(self.idx) {
            if matches!(b, b' ' | b'\t' | b'\r' | b'\n') {
                self.bump();
            } else {
                break;
            }
        }
    }

    fn read_string(&mut self) -> Option<String> {
        let start = self.idx;
        if self.bump()? != b'"' {
            return None;
        }
        while let Some(b) = self.bump() {
            if b == b'\\' {
                self.bump();
            } else if b == b'"' {
                return serde_json::from_str::<String>(&self.src[start..self.idx]).ok();
            }
        }
        None
    }

    fn scan_value(&mut self, path: &str, key_pos: Option<(usize, usize)>) {
        self.skip_ws();
        let start = self.pos();
        match self.bytes.get(self.idx).copied() {
            Some(b'{') => {
                self.bump();
                self.skip_ws();
                let mut seen: HashSet<String> = HashSet::new();
                if self.bytes.get(self.idx) != Some(&b'}') {
                    loop {
                        self.skip_ws();
                        let kp = self.pos();
                        let Some(k) = self.read_string() else {
                            break;
                        };
                        let child_path = if path.is_empty() {
                            k.clone()
                        } else {
                            format!("{path}.{k}")
                        };
                        if !seen.insert(k.clone()) {
                            self.duplicates.push(DuplicateKey {
                                json_path: child_path.clone(),
                                key: k,
                                line: kp.0,
                                column: kp.1,
                            });
                        }
                        self.skip_ws();
                        if self.bytes.get(self.idx) == Some(&b':') {
                            self.bump();
                        }
                        self.scan_value(&child_path, Some(kp));
                        self.skip_ws();
                        if self.bytes.get(self.idx) == Some(&b',') {
                            self.bump();
                        } else {
                            break;
                        }
                    }
                }
                self.skip_ws();
                let end = self.pos();
                if self.bytes.get(self.idx) == Some(&b'}') {
                    self.bump();
                }
                self.locs.insert(
                    path.to_string(),
                    Loc {
                        key: key_pos,
                        start,
                        end,
                    },
                );
            }
            Some(b'[') => {
                self.bump();
                self.skip_ws();
                let mut i = 0usize;
                if self.bytes.get(self.idx) != Some(&b']') {
                    loop {
                        let child_path = format!("{path}[{i}]");
                        self.scan_value(&child_path, None);
                        i += 1;
                        self.skip_ws();
                        if self.bytes.get(self.idx) == Some(&b',') {
                            self.bump();
                        } else {
                            break;
                        }
                    }
                }
                self.skip_ws();
                let end = self.pos();
                if self.bytes.get(self.idx) == Some(&b']') {
                    self.bump();
                }
                self.locs.insert(
                    path.to_string(),
                    Loc {
                        key: key_pos,
                        start,
                        end,
                    },
                );
            }
            Some(b'"') => {
                let _ = self.read_string();
                let end = self.pos();
                self.locs.insert(
                    path.to_string(),
                    Loc {
                        key: key_pos,
                        start,
                        end,
                    },
                );
            }
            _ => {
                while let Some(&b) = self.bytes.get(self.idx) {
                    if matches!(b, b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n') {
                        break;
                    }
                    self.bump();
                }
                let end = self.pos();
                self.locs.insert(
                    path.to_string(),
                    Loc {
                        key: key_pos,
                        start,
                        end,
                    },
                );
            }
        }
    }
}

fn scan_json(text: &str) -> Scan {
    let mut s = Scanner::new(text);
    s.scan_value("", None);
    Scan {
        locs: s.locs,
        duplicates: s.duplicates,
    }
}

fn key_pos(scan: &Scan, path: &str) -> (usize, usize) {
    scan.locs
        .get(path)
        .and_then(|l| l.key.or(Some(l.start)))
        .unwrap_or((1, 1))
}

fn start_pos(scan: &Scan, path: &str) -> (usize, usize) {
    scan.locs.get(path).map_or((1, 1), |l| l.start)
}

fn end_pos(scan: &Scan, path: &str) -> (usize, usize) {
    scan.locs.get(path).map_or((1, 1), |l| l.end)
}

fn push_said(
    errors: &mut Vec<Diagnostic>,
    said: &str,
    line: usize,
    column: usize,
    json_path: Option<String>,
) {
    errors.push(diagnostic::from_said(
        "",
        said,
        line,
        column,
        json_path,
        DocumentKind::Model,
    ));
}

fn unknown_field_said(name: &str, expected: &[&str]) -> String {
    if expected.is_empty() {
        format!("unknown field `{name}`, there are no fields")
    } else {
        let list: Vec<String> = expected.iter().map(|f| format!("`{f}`")).collect();
        format!(
            "unknown field `{name}`, expected one of {}",
            list.join(", ")
        )
    }
}

fn check_field<T: DeserializeOwned>(
    val: &Value,
    path: &str,
    scan: &Scan,
    errors: &mut Vec<Diagnostic>,
) -> Option<T> {
    match serde_json::from_value::<T>(val.clone()) {
        Ok(v) => Some(v),
        Err(e) => {
            let full = e.to_string();
            let said = full
                .rfind(" at line ")
                .map_or(full.as_str(), |k| &full[..k]);
            let (line, col) = start_pos(scan, path);
            push_said(errors, said, line, col, Some(path.to_string()));
            None
        }
    }
}

const MODEL_FIELDS: &[&str] = &[
    "format",
    "version",
    "name",
    "description",
    "simulation",
    "components",
    "controls",
    "outputs",
    "frequency_scan",
];

const SIMULATION_FIELDS: &[&str] = &[
    "time_step_s",
    "end_time_s",
    "method",
    "output_interval_steps",
    "initialization",
    "linear_solver",
    "switching",
    "partitioning",
    "threads",
];

fn read_simulation(val: &Value, scan: &Scan, errors: &mut Vec<Diagnostic>) -> Option<Simulation> {
    if let Ok(sim) = serde_json::from_value::<Simulation>(val.clone()) {
        return Some(sim);
    }
    let Some(map) = val.as_object() else {
        let (line, col) = start_pos(scan, "simulation");
        let err = serde_json::from_value::<Simulation>(val.clone()).unwrap_err();
        let full = err.to_string();
        let said = full
            .rfind(" at line ")
            .map_or(full.as_str(), |k| &full[..k]);
        push_said(errors, said, line, col, Some("simulation".to_string()));
        return None;
    };
    for key in map.keys() {
        if !SIMULATION_FIELDS.contains(&key.as_str()) {
            let jp = format!("simulation.{key}");
            let (line, col) = key_pos(scan, &jp);
            push_said(
                errors,
                &unknown_field_said(key, SIMULATION_FIELDS),
                line,
                col,
                Some(jp),
            );
        }
    }
    for req in ["time_step_s", "end_time_s"] {
        if !map.contains_key(req) {
            let (line, col) = end_pos(scan, "simulation");
            push_said(
                errors,
                &format!("missing field `{req}`"),
                line,
                col,
                Some(format!("simulation.{req}")),
            );
        }
    }
    if let Some(v) = map.get("time_step_s") {
        check_field::<f64>(v, "simulation.time_step_s", scan, errors);
    }
    if let Some(v) = map.get("end_time_s") {
        check_field::<f64>(v, "simulation.end_time_s", scan, errors);
    }
    if let Some(v) = map.get("method") {
        check_field::<Method>(v, "simulation.method", scan, errors);
    }
    if let Some(v) = map.get("output_interval_steps") {
        check_field::<Option<u64>>(v, "simulation.output_interval_steps", scan, errors);
    }
    if let Some(v) = map.get("initialization") {
        check_field::<Initialization>(v, "simulation.initialization", scan, errors);
    }
    if let Some(v) = map.get("linear_solver") {
        check_field::<LinearSolver>(v, "simulation.linear_solver", scan, errors);
    }
    if let Some(v) = map.get("switching") {
        check_field::<Option<Switching>>(v, "simulation.switching", scan, errors);
    }
    if let Some(v) = map.get("partitioning") {
        check_field::<Option<Partitioning>>(v, "simulation.partitioning", scan, errors);
    }
    if let Some(v) = map.get("threads") {
        check_field::<Option<u32>>(v, "simulation.threads", scan, errors);
    }
    None
}

fn find_key_path(val: &Value, prefix: &str, target: &str) -> Option<String> {
    match val {
        Value::Object(map) => {
            if map.contains_key(target) {
                return Some(format!("{prefix}.{target}"));
            }
            for (k, v) in map {
                let child = format!("{prefix}.{k}");
                if let Some(found) = find_key_path(v, &child, target) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                let child = format!("{prefix}[{i}]");
                if let Some(found) = find_key_path(v, &child, target) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn find_variant_path(val: &Value, prefix: &str, target: &str) -> Option<String> {
    match val {
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some(target) {
                return Some(format!("{prefix}.type"));
            }
            for (k, v) in map {
                if k == "type" {
                    continue;
                }
                let child = format!("{prefix}.{k}");
                if let Some(found) = find_variant_path(v, &child, target) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                let child = format!("{prefix}[{i}]");
                if let Some(found) = find_variant_path(v, &child, target) {
                    return Some(found);
                }
            }
            None
        }
        Value::String(s) if s == target => Some(prefix.to_string()),
        _ => None,
    }
}

fn find_missing_obj(val: &Value, prefix: &str, missing: &str) -> Option<String> {
    let Value::Object(map) = val else {
        return None;
    };
    for (k, v) in map {
        let child = format!("{prefix}.{k}");
        if let Some(found) = find_missing_obj(v, &child, missing) {
            return Some(found);
        }
    }
    if !map.contains_key(missing) {
        return Some(prefix.to_string());
    }
    None
}

fn find_value_mismatch(val: &Value, prefix: &str, said: &str) -> Option<String> {
    if let Some(rest) = said.strip_prefix("invalid type: ") {
        let (got, expected) = rest.split_once(", expected ")?;
        if let Some(quoted) = got.strip_prefix("string ") {
            if let Ok(target) = serde_json::from_str::<String>(quoted) {
                return find_matching_leaf(val, prefix, &|v| v.as_str() == Some(&target));
            }
        }
        if got == "boolean true" || got == "boolean false" {
            let b = got == "boolean true";
            return find_matching_leaf(val, prefix, &|v| v.as_bool() == Some(b));
        }
        if expected == "a sequence" {
            return find_matching_leaf(val, prefix, &|v| !v.is_array() && !v.is_object());
        }
    }
    if let Some(rest) = said.strip_prefix("invalid length ") {
        let len_str = rest.split(',').next()?.trim();
        if let Ok(len) = len_str.parse::<usize>() {
            return find_matching_leaf(val, prefix, &|v| {
                v.as_array().is_some_and(|a| a.len() == len)
            });
        }
    }
    if said.contains("a bus name string or an array of three node names") {
        return find_matching_leaf(val, prefix, &|v| {
            !v.is_string()
                && !v
                    .as_array()
                    .is_some_and(|a| a.len() == 3 && a.iter().all(Value::is_string))
                && (v.is_array() || v.is_number() || v.is_boolean() || v.is_null())
        });
    }
    if said.contains("a number or the name of a control signal") {
        return find_matching_leaf(val, prefix, &|v| v.is_boolean() || v.is_null());
    }
    if said.starts_with("invalid value: integer ") || said.starts_with("invalid type: integer ") {
        if let Some(num_str) = said
            .split("integer `")
            .nth(1)
            .and_then(|r| r.split('`').next())
        {
            if let Ok(n) = num_str.parse::<i64>() {
                return find_matching_leaf(val, prefix, &|v| v.as_i64() == Some(n));
            }
        }
    }
    None
}

fn find_matching_leaf(val: &Value, prefix: &str, pred: &dyn Fn(&Value) -> bool) -> Option<String> {
    if pred(val) {
        return Some(prefix.to_string());
    }
    match val {
        Value::Object(map) => {
            for (k, v) in map {
                let child = format!("{prefix}.{k}");
                if let Some(found) = find_matching_leaf(v, &child, pred) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                let child = format!("{prefix}[{i}]");
                if let Some(found) = find_matching_leaf(v, &child, pred) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn remove_key_recursive(val: &mut Value, target: &str) -> bool {
    match val {
        Value::Object(map) => {
            if map.remove(target).is_some() {
                return true;
            }
            for v in map.values_mut() {
                if remove_key_recursive(v, target) {
                    return true;
                }
            }
            false
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                if remove_key_recursive(v, target) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

fn record_value_error(
    val: &Value,
    base_path: &str,
    said: &str,
    scan: &Scan,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some((name, _)) = unknown_name(said, "unknown field") {
        let jp = find_key_path(val, base_path, name).unwrap_or_else(|| base_path.to_string());
        let (line, col) = key_pos(scan, &jp);
        push_said(errors, said, line, col, Some(jp));
    } else if let Some((name, _)) = unknown_name(said, "unknown variant") {
        let jp = find_variant_path(val, base_path, name).unwrap_or_else(|| base_path.to_string());
        let (line, col) = start_pos(scan, &jp);
        push_said(errors, said, line, col, Some(jp));
    } else if let Some(name) = said
        .strip_prefix("missing field `")
        .and_then(|r| r.strip_suffix('`'))
    {
        let obj_path =
            find_missing_obj(val, base_path, name).unwrap_or_else(|| base_path.to_string());
        let (line, col) = end_pos(scan, &obj_path);
        let jp = format!("{obj_path}.{name}");
        push_said(errors, said, line, col, Some(jp));
    } else if let Some(jp) = find_value_mismatch(val, base_path, said) {
        let (line, col) = start_pos(scan, &jp);
        push_said(errors, said, line, col, Some(jp));
    } else {
        let (line, col) = start_pos(scan, base_path);
        push_said(errors, said, line, col, Some(base_path.to_string()));
    }
}

fn check_item<T: DeserializeOwned>(
    val: &Value,
    base_path: &str,
    scan: &Scan,
    errors: &mut Vec<Diagnostic>,
) -> Option<T> {
    let mut current = val.clone();
    for _ in 0..16 {
        match serde_json::from_value::<T>(current.clone()) {
            Ok(v) => return Some(v),
            Err(e) => {
                let full = e.to_string();
                let said = full
                    .rfind(" at line ")
                    .map_or(full.as_str(), |k| &full[..k]);
                record_value_error(&current, base_path, said, scan, errors);
                if let Some((unknown, _)) = unknown_name(said, "unknown field") {
                    if remove_key_recursive(&mut current, unknown) {
                        continue;
                    }
                }
                return None;
            }
        }
    }
    None
}

/// Parses a model JSON string, collecting all E001 errors across the document.
pub(crate) fn read_model(text: &str) -> Result<Model, Vec<Diagnostic>> {
    let text = crate::strip_bom(text);
    let root: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            return Err(vec![diagnostic::from_parse_error(
                text,
                &e,
                DocumentKind::Model,
            )])
        }
    };
    let scan = scan_json(text);
    let mut errors = Vec::new();
    for dup in &scan.duplicates {
        let said = format!("duplicate field `{}`", dup.key);
        push_said(
            &mut errors,
            &said,
            dup.line,
            dup.column,
            Some(dup.json_path.clone()),
        );
    }
    let Some(map) = root.as_object() else {
        let err = serde_json::from_value::<Model>(root).unwrap_err();
        let full = err.to_string();
        let said = full
            .rfind(" at line ")
            .map_or(full.as_str(), |k| &full[..k]);
        push_said(&mut errors, said, 1, 1, None);
        return Err(errors);
    };

    for key in map.keys() {
        if !MODEL_FIELDS.contains(&key.as_str()) {
            let (line, col) = key_pos(&scan, key);
            push_said(
                &mut errors,
                &unknown_field_said(key, MODEL_FIELDS),
                line,
                col,
                Some(key.clone()),
            );
        }
    }
    for req in ["format", "version", "simulation", "components"] {
        if !map.contains_key(req) {
            let (line, col) = end_pos(&scan, "");
            push_said(
                &mut errors,
                &format!("missing field `{req}`"),
                line,
                col,
                Some(req.to_string()),
            );
        }
    }

    let format = map
        .get("format")
        .and_then(|v| check_field::<String>(v, "format", &scan, &mut errors));
    let version = map
        .get("version")
        .and_then(|v| check_field::<String>(v, "version", &scan, &mut errors));
    let name = match map.get("name") {
        Some(v) => check_field::<String>(v, "name", &scan, &mut errors),
        None => Some(String::new()),
    };
    let description = match map.get("description") {
        Some(v) => check_field::<String>(v, "description", &scan, &mut errors),
        None => Some(String::new()),
    };
    let simulation = map
        .get("simulation")
        .and_then(|v| read_simulation(v, &scan, &mut errors));

    let components = match map.get("components") {
        Some(Value::Array(arr)) => {
            let mut list = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                if let Some(c) =
                    check_item::<Component>(item, &format!("components[{i}]"), &scan, &mut errors)
                {
                    list.push(c);
                }
            }
            Some(list)
        }
        Some(other) => {
            check_field::<Vec<Component>>(other, "components", &scan, &mut errors);
            None
        }
        None => None,
    };

    let controls = match map.get("controls") {
        Some(Value::Array(arr)) => {
            let mut list = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                if let Some(c) =
                    check_item::<Control>(item, &format!("controls[{i}]"), &scan, &mut errors)
                {
                    list.push(c);
                }
            }
            Some(list)
        }
        Some(other) => {
            check_field::<Vec<Control>>(other, "controls", &scan, &mut errors);
            None
        }
        None => Some(Vec::new()),
    };

    let outputs = match map.get("outputs") {
        Some(Value::Null) | None => Some(None),
        Some(Value::Array(arr)) => {
            let mut list = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                if let Some(o) =
                    check_item::<Output>(item, &format!("outputs[{i}]"), &scan, &mut errors)
                {
                    list.push(o);
                }
            }
            Some(Some(list))
        }
        Some(other) => {
            check_field::<Vec<Output>>(other, "outputs", &scan, &mut errors);
            None
        }
    };

    let frequency_scan = match map.get("frequency_scan") {
        Some(Value::Null) | None => Some(None),
        Some(v) => check_item::<FrequencyScan>(v, "frequency_scan", &scan, &mut errors).map(Some),
    };

    if !errors.is_empty() {
        return Err(errors);
    }

    let version = version.expect("checked");
    let mut simulation = simulation.expect("checked");
    if simulation.switching.is_none() && version_at_least(&version, INTERPOLATED_BY_DEFAULT) {
        simulation.switching = Some(Switching::Interpolated);
    }
    Ok(Model {
        format: format.expect("checked"),
        version,
        name: name.expect("checked"),
        description: description.expect("checked"),
        simulation,
        components: components.expect("checked"),
        controls: controls.expect("checked"),
        outputs: outputs.expect("checked"),
        frequency_scan: frequency_scan.expect("checked"),
    })
}
