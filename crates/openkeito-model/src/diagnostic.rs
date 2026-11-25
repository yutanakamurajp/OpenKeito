use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

/// A problem found in a model or during a run, written so that a person or an
/// AI agent can fix it without reading the source: where it is, what is wrong
/// and what to change.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable identifier such as "E130".
    pub code: &'static str,
    /// Location in the model, such as "components[2].resistance_ohm" (or
    /// "line 14 column 4" for E001).
    pub path: String,
    /// Model JSON path (such as "components[2].resistance") when `path`
    /// holds "line X column Y" on a read error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_path: Option<String>,
    pub message: String,
    pub hint: String,
    /// The components and nodes the diagnostic is about, where its path does
    /// not say (the members of a loop of ideal branches, nodes that are not
    /// connected to gnd). Not part of the document: `topology` uses them to
    /// say where a diagram marks the diagnostic.
    #[serde(skip)]
    pub about: Vec<Subject>,
}

/// What a diagnostic is about beyond its path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// The name of a component.
    Component(String),
    /// The name of a node.
    Node(String),
}

impl Diagnostic {
    /// The components the diagnostic is about, for `topology`.
    pub fn about_components<S: Into<String>>(mut self, names: impl IntoIterator<Item = S>) -> Self {
        let names = names.into_iter().map(|n| Subject::Component(n.into()));
        self.about.extend(names);
        self
    }

    /// The nodes the diagnostic is about, for `topology`.
    pub fn about_nodes<S: Into<String>>(mut self, names: impl IntoIterator<Item = S>) -> Self {
        let names = names.into_iter().map(|n| Subject::Node(n.into()));
        self.about.extend(names);
        self
    }

    pub fn error(
        code: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Diagnostic {
            severity: Severity::Error,
            code,
            path: path.into(),
            json_path: None,
            message: message.into(),
            hint: hint.into(),
            about: Vec::new(),
        }
    }

    pub fn warning(
        code: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Diagnostic {
            severity: Severity::Warning,
            code,
            path: path.into(),
            json_path: None,
            message: message.into(),
            hint: hint.into(),
            about: Vec::new(),
        }
    }

    pub fn with_json_path(mut self, json_path: impl Into<String>) -> Self {
        self.json_path = Some(json_path.into());
        self
    }
}

/// The kinds of JSON documents, for the wording of E001.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentKind {
    Model,
    Batch,
    Schedule,
}

impl DocumentKind {
    /// What the document is called, and the command that prints its schema.
    fn words(self) -> (&'static str, &'static str) {
        match self {
            DocumentKind::Model => ("model", "openkeito schema"),
            DocumentKind::Batch => ("batch file", "openkeito schema --batch"),
            DocumentKind::Schedule => ("schedule", "openkeito schema --schedule"),
        }
    }
}

pub(crate) fn from_parse_error(
    text: &str,
    e: &serde_json::Error,
    kind: DocumentKind,
) -> Diagnostic {
    let full = e.to_string();
    let said = match full.rfind(" at line ") {
        Some(k) => &full[..k],
        None => full.as_str(),
    };
    let (mut line, mut column) = (e.line(), e.column());
    if let Some((name, _)) = unknown_name(said, "unknown field") {
        if let Some(at) = last_string_before(text, name, line, column) {
            (line, column) = at;
        }
    } else if let Some((name, _)) = unknown_name(said, "unknown variant") {
        if let Some(at) = last_string_before(text, name, line, column) {
            (line, column) = at;
        }
    }
    from_said(text, said, line, column, None, kind)
}

pub(crate) fn from_said(
    text: &str,
    said: &str,
    line: usize,
    column: usize,
    json_path: Option<String>,
    kind: DocumentKind,
) -> Diagnostic {
    let (what, schema) = kind.words();
    let general = format!(
        "The {what} must be one JSON document that follows the schema printed by `{schema}`. \
         Unknown fields and misspelled field names are errors."
    );
    let printed = format!("`{schema}` prints the schema of the {what}.");
    let exponent_mistake = if text.is_empty() {
        None
    } else {
        missing_exponent(text, line, column)
    };
    let hint = if let Some((mantissa, exponent)) = exponent_mistake {
        format!(
            "A number is written without the exponent letter. Write {mantissa}e{exponent}, not \
             {mantissa}{exponent}."
        )
    } else if let Some((name, expected)) = unknown_name(said, "unknown field") {
        match closest(name, &expected) {
            Some(c) => format!(
                "Write \"{c}\" in place of \"{name}\". The fields here are: {}. {printed}",
                expected.join(", ")
            ),
            None if expected.is_empty() => {
                format!("Remove \"{name}\": there are no fields here. {general}")
            }
            None => format!(
                "Remove \"{name}\", or use one of the fields here: {}. {printed}",
                expected.join(", ")
            ),
        }
    } else if let Some((name, expected)) = unknown_name(said, "unknown variant") {
        match closest(name, &expected) {
            Some(c) => format!(
                "Write \"{c}\" in place of \"{name}\". The values here are: {}. {printed}",
                expected.join(", ")
            ),
            None => format!(
                "Use one of the values here in place of \"{name}\": {}. {printed}",
                expected.join(", ")
            ),
        }
    } else if let Some(name) = said
        .strip_prefix("missing field `")
        .and_then(|rest| rest.strip_suffix('`'))
    {
        format!(
            "Add \"{name}\" to the object that ends at this position. `{schema}` lists the \
             fields of every object and which of them are required."
        )
    } else if let Some(name) = said
        .strip_prefix("duplicate field `")
        .and_then(|rest| rest.strip_suffix('`'))
    {
        format!(
            "Remove the duplicate \"{name}\" field so each key appears only once in the object."
        )
    } else if said == "trailing comma" {
        "Remove the trailing comma before '}' or ']': JSON does not allow trailing commas."
            .to_string()
    } else if said == "EOF while parsing a value" && text.trim().is_empty() {
        format!("The input is empty. Provide a JSON {what} that follows `{schema}`.")
    } else if said.starts_with("EOF while parsing ") {
        "The JSON input ended unexpectedly: check for an unclosed '{', '[' or '\"', or a missing \
         value."
            .to_string()
    } else if said == "number out of range" {
        "This number cannot be represented in IEEE 754 double precision (about +/-1.8e308). Check \
         the value and its unit."
            .to_string()
    } else if said.contains("a bus name string or an array of three node names") {
        "Write a bus name string (for example \"b1\", standing for b1.a, b1.b and b1.c) or an \
         array of three node names [\"a\", \"b\", \"c\"]."
            .to_string()
    } else if said.contains("a number or the name of a control signal") {
        "Write a JSON number without quotes (for a constant) or a quoted string naming a control \
         signal."
            .to_string()
    } else if let Some(hint) = length_hint(said) {
        hint
    } else if let Some(hint) = type_or_value_hint(said, &printed) {
        hint
    } else {
        general
    };
    let message = match &json_path {
        Some(jp) => format!("the {what} cannot be read at {jp}: {said}"),
        None => format!("the {what} cannot be read: {said}"),
    };
    let d = Diagnostic::error(
        "E001",
        format!("line {line} column {column}"),
        message,
        hint,
    );
    match json_path {
        Some(jp) => d.with_json_path(jp),
        None => d,
    }
}

fn length_hint(said: &str) -> Option<String> {
    let (_, expected) = said.strip_prefix("invalid length ")?.split_once(", ")?;
    if let Some(n) = expected
        .strip_prefix("expected an array of length ")
        .or_else(|| expected.strip_prefix("expected a tuple of size "))
    {
        return Some(format!("Write a JSON array with exactly {n} elements."));
    }
    if let Some(n) = expected
        .strip_prefix("expected ")
        .and_then(|rest| rest.split_whitespace().next())
    {
        if n.chars().all(|c| c.is_ascii_digit()) {
            return Some(format!("Write a JSON array with {n} elements."));
        }
    }
    Some(format!(
        "Check the number of elements in the JSON array ({expected})."
    ))
}

fn type_or_value_hint(said: &str, printed: &str) -> Option<String> {
    if (said.starts_with("invalid value: integer ") || said.starts_with("invalid type: integer "))
        && (said.contains("expected u32")
            || said.contains("expected u64")
            || said.contains("expected usize"))
    {
        return Some("Write a non-negative integer (>= 0).".to_string());
    }
    let rest = said.strip_prefix("invalid type: ")?;
    let (got, expected) = rest.split_once(", expected ")?;
    if expected == "f64" || expected == "a floating point number" {
        if let Some(quoted) = got.strip_prefix("string ") {
            let inner = quoted.trim_matches('"');
            if inner.trim().parse::<f64>().is_ok() {
                return Some(format!(
                    "Write {inner} as a JSON number without quotes, not {quoted}."
                ));
            }
        }
        return Some("Write a JSON number without quotes.".to_string());
    }
    if matches!(expected, "u32" | "u64" | "usize") {
        return Some("Write a non-negative integer (>= 0) without quotes.".to_string());
    }
    if expected == "a boolean" {
        return Some("Write true or false without quotes.".to_string());
    }
    if expected == "a string" {
        return Some("Write a JSON string in double quotes.".to_string());
    }
    if expected == "a sequence" {
        return Some("Write a JSON array [...].".to_string());
    }
    if expected == "a map" || expected.starts_with("struct ") {
        return Some(format!("Write a JSON object {{...}}. {printed}"));
    }
    None
}

/// The name and the names expected in its place of a message of the parser
/// such as "unknown field `resistance`, expected one of `name`, `nodes`"
/// (`kind` is "unknown field" or "unknown variant").
pub(crate) fn unknown_name<'a>(said: &'a str, kind: &str) -> Option<(&'a str, Vec<&'a str>)> {
    let rest = said.strip_prefix(kind)?.strip_prefix(" `")?;
    let (name, rest) = rest.split_once('`')?;
    // "expected `a`", "expected one of `a`, `b`" or "there are no fields".
    let expected = rest
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|n| !n.is_empty())
        .collect();
    Some((name, expected))
}

/// The name among `expected` that `name` was most likely meant to be: one
/// that starts with it or that it starts with (a unit left out or added),
/// or else the one fewest changes of a letter away, when those are few.
fn closest<'a>(name: &str, expected: &[&'a str]) -> Option<&'a str> {
    let lower = name.to_ascii_lowercase();
    if lower.len() >= 2 {
        let related: Vec<&str> = expected
            .iter()
            .copied()
            .filter(|c| c.starts_with(&lower) || lower.starts_with(c))
            .collect();
        if let [one] = related[..] {
            return Some(one);
        }
    }
    let (distance, best) = expected
        .iter()
        .map(|c| (edit_distance(&lower, c), *c))
        .min_by_key(|(d, _)| *d)?;
    (distance <= (lower.len() / 3).max(2) && distance < lower.len()).then_some(best)
}

/// Number of letters to insert, remove or replace to turn `a` into `b`.
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let replaced = diagonal + usize::from(ca != cb);
            diagonal = row[j + 1];
            row[j + 1] = replaced.min(row[j] + 1).min(diagonal + 1);
        }
    }
    row[b.len()]
}

/// Line and column (from 1, in characters) of the last `"name"` in `text`
/// that starts before the position `line`, `column` of the parser.
fn last_string_before(
    text: &str,
    name: &str,
    line: usize,
    column: usize,
) -> Option<(usize, usize)> {
    let quoted = format!("\"{name}\"");
    let mut found = None;
    for (k, l) in text.lines().enumerate().take(line) {
        for (byte, _) in l.match_indices(&quoted) {
            let at = l[..byte].chars().count() + 1;
            if k + 1 < line || at < column {
                found = Some((k + 1, at));
            }
        }
    }
    found
}

/// The number at which the parser stopped when it is written without its
/// exponent letter, as in `1.1141-6`: what stands before the sign and the
/// sign with the digits after it.
fn missing_exponent(text: &str, line: usize, column: usize) -> Option<(&str, &str)> {
    let l = text.lines().nth(line.checked_sub(1)?)?;
    let b = l.as_bytes();
    // serde_json reports a 1-based column at or just after the offending byte.
    let sign = [column.wrapping_sub(1), column.wrapping_sub(2)]
        .into_iter()
        .find(|&i| {
            i >= 1
                && i < b.len()
                && (b[i] == b'-' || b[i] == b'+')
                && (b[i - 1].is_ascii_digit() || b[i - 1] == b'.')
        })?;
    let start = b[..sign]
        .iter()
        .rposition(|c| !(c.is_ascii_digit() || *c == b'.' || *c == b'-'))
        .map_or(0, |k| k + 1);
    let end = b[sign + 1..]
        .iter()
        .position(|c| !c.is_ascii_digit())
        .map_or(b.len(), |k| sign + 1 + k);
    (end > sign + 1).then(|| (&l[start..sign], &l[sign..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_closest_name_is_one_with_or_without_its_unit_or_a_few_letters_away() {
        let fields = ["name", "nodes", "resistance_ohm"];
        assert_eq!(closest("resistance", &fields), Some("resistance_ohm"));
        assert_eq!(closest("resistance_ohms", &fields), Some("resistance_ohm"));
        assert_eq!(closest("Nodes", &fields), Some("nodes"));
        assert_eq!(closest("node", &fields), Some("nodes"));
        assert_eq!(closest("nmae", &fields), Some("name"));
        assert_eq!(closest("capacitance_f", &fields), None);
        assert_eq!(closest("x", &fields), None);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(
            unknown_name(
                "unknown field `r`, expected one of `name`, `nodes`",
                "unknown field"
            ),
            Some(("r", vec!["name", "nodes"]))
        );
        assert_eq!(
            unknown_name("unknown field `r`, there are no fields", "unknown field"),
            Some(("r", vec![]))
        );
        assert_eq!(
            missing_exponent("  \"c\": 4.7-6,", 1, 11),
            Some(("4.7", "-6"))
        );
        assert_eq!(missing_exponent("  \"c\": 4.7,", 1, 11), None);
    }
}
