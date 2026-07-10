//! `apply-ops`: replay chosen logical ops onto a file, going through each
//! format's own writer so the edit is format-preserving where the format
//! allows it (`toml_edit` keeps comments and layout; keyvalue is edited
//! line-by-line; json/yaml/plist re-serialize). The model decides *which*
//! ops to keep — this module is only the mechanical half.

use std::fs;
use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::diff::{Op, parse_address};
use crate::store;
use crate::value::{self, Format};

/// Apply ops onto a file in place, returning the format that was used
/// (detected from the file unless overridden).
pub fn apply_to_file(path: &Path, format_override: Option<Format>, ops: &[Op]) -> Result<Format> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let format = format_override.unwrap_or_else(|| value::detect(path, &bytes));
    let output = render_applied(format, path, &bytes, ops)?;
    store::write_creating_parents(path, &output)?;
    Ok(format)
}

fn render_applied(format: Format, path: &Path, bytes: &[u8], ops: &[Op]) -> Result<Vec<u8>> {
    match format {
        Format::Json => {
            let text = std::str::from_utf8(bytes).context("file is not UTF-8")?;
            let mut root: Value = serde_json::from_str(text)
                .with_context(|| format!("parsing {} as JSON", path.display()))?;
            apply_structured(&mut root, format, ops)?;
            let mut out = serde_json::to_string_pretty(&root).context("serializing JSON")?;
            out.push('\n');
            Ok(out.into_bytes())
        }
        Format::Yaml => {
            let text = std::str::from_utf8(bytes).context("file is not UTF-8")?;
            let yaml: serde_norway::Value = serde_norway::from_str(text)
                .with_context(|| format!("parsing {} as YAML", path.display()))?;
            let mut root = serde_json::to_value(yaml).context("normalizing YAML")?;
            apply_structured(&mut root, format, ops)?;
            let out = serde_norway::to_string(&root).context("serializing YAML")?;
            Ok(out.into_bytes())
        }
        Format::Toml => {
            let text = std::str::from_utf8(bytes).context("file is not UTF-8")?;
            let out = apply_toml(text, ops)
                .with_context(|| format!("applying ops to {}", path.display()))?;
            Ok(out.into_bytes())
        }
        Format::Plist => {
            let plist_value = plist::Value::from_reader(Cursor::new(bytes))
                .with_context(|| format!("parsing {} as plist", path.display()))?;
            let mut root = value::plist_to_json(plist_value);
            apply_structured(&mut root, format, ops)?;
            let back = json_to_plist(&root)?;
            let mut out = Vec::new();
            if bytes.starts_with(b"bplist") {
                back.to_writer_binary(&mut out)
                    .context("writing binary plist")?;
            } else {
                back.to_writer_xml(&mut out).context("writing XML plist")?;
                out.push(b'\n');
            }
            Ok(out)
        }
        Format::Keyvalue => {
            let text = std::str::from_utf8(bytes).context("file is not UTF-8")?;
            let out = apply_keyvalue(text, ops)
                .with_context(|| format!("applying ops to {}", path.display()))?;
            Ok(out.into_bytes())
        }
        Format::Text => bail!(
            "{} is tracked as text: it has no addressed ops — edit the file directly",
            path.display()
        ),
    }
}

// --- structured (json/yaml/plist, via serde_json::Value) ---

pub fn apply_structured(root: &mut Value, format: Format, ops: &[Op]) -> Result<()> {
    for op in ops {
        match op {
            Op::Add { path, value } => set_value(root, format, path, value.clone())?,
            Op::Replace { path, to, .. } => set_value(root, format, path, to.clone())?,
            Op::Remove { path, .. } => remove_value(root, format, path)?,
            Op::Text { .. } | Op::Binary => {
                bail!("unaddressed op cannot be applied; resolve text diffs by hand")
            }
        }
    }
    Ok(())
}

fn set_value(root: &mut Value, format: Format, path: &str, new: Value) -> Result<()> {
    let segments = parse_address(format, path);
    let Some((last, parents)) = segments.split_last() else {
        *root = new;
        return Ok(());
    };
    match navigate(root, parents, path)? {
        Value::Object(map) => {
            map.insert(last.clone(), new);
        }
        Value::Array(items) => {
            let index: usize = last
                .parse()
                .with_context(|| format!("path {path}: {last} is not an array index"))?;
            if index == items.len() {
                items.push(new);
            } else {
                *items
                    .get_mut(index)
                    .with_context(|| format!("path {path}: index {index} out of range"))? = new;
            }
        }
        _ => bail!("path {path}: parent is not a container"),
    }
    Ok(())
}

fn remove_value(root: &mut Value, format: Format, path: &str) -> Result<()> {
    let segments = parse_address(format, path);
    let Some((last, parents)) = segments.split_last() else {
        bail!("cannot remove the document root");
    };
    match navigate(root, parents, path)? {
        Value::Object(map) => {
            map.shift_remove(last)
                .with_context(|| format!("path {path}: key {last} not present"))?;
        }
        Value::Array(items) => {
            let index: usize = last
                .parse()
                .with_context(|| format!("path {path}: {last} is not an array index"))?;
            if index >= items.len() {
                bail!("path {path}: index {index} out of range");
            }
            items.remove(index);
        }
        _ => bail!("path {path}: parent is not a container"),
    }
    Ok(())
}

fn navigate<'root>(
    root: &'root mut Value,
    segments: &[String],
    full: &str,
) -> Result<&'root mut Value> {
    let mut current = root;
    for segment in segments {
        current = match current {
            Value::Object(map) => map.get_mut(segment),
            Value::Array(items) => segment
                .parse::<usize>()
                .ok()
                .and_then(|index| items.get_mut(index)),
            _ => None,
        }
        .with_context(|| format!("path {full}: segment {segment} not found"))?;
    }
    Ok(current)
}

// --- toml (format-preserving via toml_edit) ---

fn apply_toml(text: &str, ops: &[Op]) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = text.parse().context("parsing TOML")?;
    for op in ops {
        match op {
            Op::Add { path, value }
            | Op::Replace {
                path, to: value, ..
            } => {
                let segments = parse_address(Format::Toml, path);
                set_toml(doc.as_item_mut(), &segments, value)
                    .with_context(|| format!("setting {path}"))?;
            }
            Op::Remove { path, .. } => {
                let segments = parse_address(Format::Toml, path);
                remove_toml(doc.as_item_mut(), &segments)
                    .with_context(|| format!("removing {path}"))?;
            }
            Op::Text { .. } | Op::Binary => {
                bail!("unaddressed op cannot be applied; resolve text diffs by hand")
            }
        }
    }
    Ok(doc.to_string())
}

fn set_toml(item: &mut toml_edit::Item, segments: &[String], value: &Value) -> Result<()> {
    let Some((head, rest)) = segments.split_first() else {
        *item = toml_edit::Item::Value(json_to_toml(value)?);
        return Ok(());
    };
    if rest.is_empty()
        && let Ok(index) = head.parse::<usize>()
    {
        return set_toml_index(item, index, value);
    }
    let child = toml_child(item, head).with_context(|| format!("segment {head} not reachable"))?;
    set_toml(child, rest, value)
}

/// Final numeric segment: replace an element in place or append at `len`.
fn set_toml_index(item: &mut toml_edit::Item, index: usize, value: &Value) -> Result<()> {
    if let Some(tables) = item.as_array_of_tables_mut() {
        let table = json_to_toml_table(value)?;
        if index == tables.len() {
            tables.push(table);
        } else if let Some(slot) = tables.get_mut(index) {
            *slot = table;
        } else {
            bail!("index {index} out of range");
        }
        return Ok(());
    }
    let array = item.as_array_mut().context("not an array")?;
    let new = json_to_toml(value)?;
    if index == array.len() {
        array.push(new);
    } else if let Some(slot) = array.get_mut(index) {
        *slot = new;
    } else {
        bail!("index {index} out of range");
    }
    Ok(())
}

fn remove_toml(item: &mut toml_edit::Item, segments: &[String]) -> Result<()> {
    let Some((last, parents)) = segments.split_last() else {
        bail!("cannot remove the document root");
    };
    let mut current = item;
    for segment in parents {
        // Probe immutably first: `get_mut` with a string key auto-creates
        // missing entries, which a remove must not do.
        let exists = current.get(segment.as_str()).is_some()
            || segment
                .parse::<usize>()
                .ok()
                .is_some_and(|index| current.get(index).is_some());
        if !exists {
            bail!("segment {segment} not found");
        }
        current = toml_child(current, segment)
            .with_context(|| format!("segment {segment} not reachable"))?;
    }
    if let Ok(index) = last.parse::<usize>() {
        if let Some(tables) = current.as_array_of_tables_mut() {
            if index >= tables.len() {
                bail!("index {index} out of range");
            }
            tables.remove(index);
            return Ok(());
        }
        if let Some(array) = current.as_array_mut() {
            if index >= array.len() {
                bail!("index {index} out of range");
            }
            array.remove(index);
            return Ok(());
        }
    }
    let table = current
        .as_table_like_mut()
        .with_context(|| format!("parent of {last} is not a table"))?;
    table
        .remove(last)
        .with_context(|| format!("key {last} not present"))?;
    Ok(())
}

fn toml_child<'item>(
    item: &'item mut toml_edit::Item,
    segment: &str,
) -> Option<&'item mut toml_edit::Item> {
    if let Ok(index) = segment.parse::<usize>()
        && (item.is_array() || item.is_array_of_tables())
    {
        return item.get_mut(index);
    }
    item.get_mut(segment)
}

fn json_to_toml(value: &Value) -> Result<toml_edit::Value> {
    Ok(match value {
        Value::Null => bail!("TOML cannot represent null"),
        Value::Bool(flag) => (*flag).into(),
        Value::Number(number) => number
            .as_i64()
            .map(toml_edit::Value::from)
            .or_else(|| number.as_f64().map(toml_edit::Value::from))
            .context("number not representable in TOML")?,
        Value::String(text) => text.as_str().into(),
        Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(json_to_toml(item)?);
            }
            array.into()
        }
        Value::Object(map) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, item) in map {
                table.insert(key, json_to_toml(item)?);
            }
            table.into()
        }
    })
}

fn json_to_toml_table(value: &Value) -> Result<toml_edit::Table> {
    let Value::Object(map) = value else {
        bail!("array-of-tables element must be an object");
    };
    let mut table = toml_edit::Table::new();
    for (key, item) in map {
        table.insert(key, toml_edit::Item::Value(json_to_toml(item)?));
    }
    Ok(table)
}

// --- keyvalue (line-oriented, preserves comments and unknown lines) ---

fn apply_keyvalue(text: &str, ops: &[Op]) -> Result<String> {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    for op in ops {
        match op {
            Op::Add { path, value }
            | Op::Replace {
                path, to: value, ..
            } => {
                set_keyvalue(&mut lines, path, value)?;
            }
            Op::Remove { path, .. } => remove_keyvalue(&mut lines, path)?,
            Op::Text { .. } | Op::Binary => {
                bail!("unaddressed op cannot be applied; resolve text diffs by hand")
            }
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    Ok(out)
}

/// A keyvalue address resolved against the actual file: dotted paths are
/// ambiguous (`a.b` is section `a` key `b`, or root key `a.b`), so the
/// section part is only split off when a matching `[header]` exists.
struct KvAddress {
    section: Option<String>,
    key: String,
    /// Occurrence index for repeated keys (`keybind.1`).
    index: Option<usize>,
}

fn parse_kv_address(lines: &[String], path: &str) -> KvAddress {
    let segments: Vec<&str> = path.split('.').collect();
    let has_header = |name: &str| {
        lines
            .iter()
            .any(|line| value::header_name(line.trim()) == Some(name.to_owned()))
    };
    let (section, rest) = match segments.split_first() {
        Some((first, rest)) if !rest.is_empty() && has_header(first) => {
            (Some((*first).to_owned()), rest)
        }
        _ => (None, segments.as_slice()),
    };
    let (index, key_segments) = match rest.split_last() {
        Some((last, key_segments)) if !key_segments.is_empty() => last
            .parse::<usize>()
            .map_or((None, rest), |occurrence| (Some(occurrence), key_segments)),
        _ => (None, rest),
    };
    KvAddress {
        section,
        key: key_segments.join("."),
        index,
    }
}

/// Line range of a section's body (header excluded), or of the root section.
fn section_body(lines: &[String], section: Option<&str>) -> std::ops::Range<usize> {
    let is_header = |line: &String| value::header_name(line.trim()).is_some();
    section.map_or_else(
        || {
            let end = lines.iter().position(is_header).unwrap_or(lines.len());
            0..end
        },
        |name| {
            let start = lines
                .iter()
                .position(|line| value::header_name(line.trim()) == Some(name.to_owned()))
                .map_or(lines.len(), |header| header + 1);
            let end = lines[start..]
                .iter()
                .position(is_header)
                .map_or(lines.len(), |offset| start + offset);
            start..end
        },
    )
}

fn occurrences(lines: &[String], body: &std::ops::Range<usize>, key: &str) -> Vec<usize> {
    body.clone()
        .filter(|&line_number| {
            let trimmed = lines[line_number].trim();
            !trimmed.is_empty()
                && !trimmed.starts_with('#')
                && !trimmed.starts_with(';')
                && value::split_entry(trimmed).key == key
        })
        .collect()
}

/// Where to insert a new entry: after the last non-blank line of the body,
/// so blank separators before the next section stay put.
fn insertion_point(lines: &[String], body: &std::ops::Range<usize>) -> usize {
    lines[body.clone()]
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(body.start, |offset| body.start + offset + 1)
}

fn kv_line(key: &str, value: &Value) -> Result<String> {
    let rendered = match value {
        Value::String(text) => text.clone(),
        Value::Bool(_) | Value::Number(_) => value.to_string(),
        Value::Null | Value::Array(_) | Value::Object(_) => {
            bail!("value for {key} is not a keyvalue scalar")
        }
    };
    if rendered.is_empty() {
        Ok(key.to_owned())
    } else {
        Ok(format!("{key} = {rendered}"))
    }
}

fn set_keyvalue(lines: &mut Vec<String>, path: &str, value: &Value) -> Result<()> {
    let address = parse_kv_address(lines, path);
    // A one-segment object write is a whole new `[section]` block.
    if let (None, Value::Object(map)) = (&address.section, value)
        && address.index.is_none()
    {
        lines.push(format!("[{}]", address.key));
        for (key, item) in map {
            let line = kv_line(key, item)?;
            lines.push(line);
        }
        return Ok(());
    }
    if let Some(name) = &address.section
        && section_body(lines, Some(name)).start >= lines.len()
    {
        lines.push(format!("[{name}]"));
    }
    let body = section_body(lines, address.section.as_deref());
    let found = occurrences(lines, &body, &address.key);
    match (address.index, value) {
        (Some(occurrence), _) => {
            let line = kv_line(&address.key, value)?;
            if let Some(&line_number) = found.get(occurrence) {
                lines[line_number] = line;
            } else if occurrence == found.len() {
                let at = found
                    .last()
                    .map_or_else(|| insertion_point(lines, &body), |&last| last + 1);
                lines.insert(at, line);
            } else {
                bail!("occurrence {occurrence} of {} out of range", address.key);
            }
        }
        (None, Value::Array(items)) => {
            let at = found
                .first()
                .copied()
                .unwrap_or_else(|| insertion_point(lines, &body));
            for &line_number in found.iter().rev() {
                lines.remove(line_number);
            }
            for (offset, item) in items.iter().enumerate() {
                let line = kv_line(&address.key, item)?;
                lines.insert(at + offset, line);
            }
        }
        (None, _) => {
            let line = kv_line(&address.key, value)?;
            if let Some((&first, extra)) = found.split_first() {
                for &line_number in extra.iter().rev() {
                    lines.remove(line_number);
                }
                lines[first] = line;
            } else {
                lines.insert(insertion_point(lines, &body), line);
            }
        }
    }
    Ok(())
}

fn remove_keyvalue(lines: &mut Vec<String>, path: &str) -> Result<()> {
    let address = parse_kv_address(lines, path);
    // Removing a whole section: the path is just the header name.
    if address.key.is_empty() || (address.section.is_none() && address.index.is_none()) {
        let name = if address.key.is_empty() {
            address.section.clone()
        } else {
            Some(address.key.clone())
        };
        if let Some(name) = name
            && lines
                .iter()
                .any(|line| value::header_name(line.trim()) == Some(name.clone()))
        {
            let body = section_body(lines, Some(&name));
            let header = body.start.saturating_sub(1);
            lines.drain(header..body.end);
            return Ok(());
        }
    }
    let body = section_body(lines, address.section.as_deref());
    let found = occurrences(lines, &body, &address.key);
    if found.is_empty() {
        bail!("{} not present", address.key);
    }
    match address.index {
        Some(occurrence) => {
            let &line_number = found.get(occurrence).with_context(|| {
                format!("occurrence {occurrence} of {} out of range", address.key)
            })?;
            lines.remove(line_number);
        }
        None => {
            for &line_number in found.iter().rev() {
                lines.remove(line_number);
            }
        }
    }
    Ok(())
}

fn json_to_plist(value: &Value) -> Result<plist::Value> {
    Ok(match value {
        Value::Null => bail!("plist cannot represent null"),
        Value::Bool(flag) => plist::Value::Boolean(*flag),
        Value::Number(number) => number
            .as_i64()
            .map(|int| plist::Value::Integer(int.into()))
            .or_else(|| number.as_u64().map(|int| plist::Value::Integer(int.into())))
            .or_else(|| number.as_f64().map(plist::Value::Real))
            .context("number not representable in plist")?,
        Value::String(text) => text.strip_prefix("hex:").map_or_else(
            || plist::Value::String(text.clone()),
            |encoded| {
                unhex(encoded)
                    .map_or_else(|| plist::Value::String(text.clone()), plist::Value::Data)
            },
        ),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(json_to_plist(item)?);
            }
            plist::Value::Array(out)
        }
        Value::Object(map) => {
            let mut dict = plist::Dictionary::new();
            for (key, item) in map {
                dict.insert(key.clone(), json_to_plist(item)?);
            }
            plist::Value::Dictionary(dict)
        }
    })
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || text.is_empty() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::diff::diff_bytes;

    #[test]
    fn json_apply_round_trips_a_diff() {
        let base = br#"{"a": 1, "list": [1, 2], "drop": true}"#;
        let edited = br#"{"a": 2, "list": [1, 2, 3], "new": {"deep": "x"}}"#;
        let ops = diff_bytes(Format::Json, base, edited);
        let mut root: Value = serde_json::from_slice(base).expect("parse");
        apply_structured(&mut root, Format::Json, &ops).expect("apply");
        let expected: Value = serde_json::from_slice(edited).expect("parse");
        assert_eq!(root, expected);
    }

    #[test]
    fn toml_apply_preserves_comments() {
        let base = "# heading comment\ntitle = \"old\" # trailing\n\n[server]\nport = 80\n";
        let ops = vec![
            Op::Replace {
                path: "server.port".to_owned(),
                from: json!(80),
                to: json!(8080),
            },
            Op::Add {
                path: "server.host".to_owned(),
                value: json!("localhost"),
            },
        ];
        let out = apply_toml(base, &ops).expect("apply");
        assert!(out.contains("# heading comment"), "out: {out}");
        assert!(out.contains("# trailing"), "out: {out}");
        assert!(out.contains("port = 8080"), "out: {out}");
        assert!(out.contains("host = \"localhost\""), "out: {out}");
    }

    #[test]
    fn toml_remove_does_not_create_missing_paths() {
        let base = "a = 1\n";
        let err = apply_toml(
            base,
            &[Op::Remove {
                path: "missing.key".to_owned(),
                from: json!(1),
            }],
        )
        .expect_err("should fail");
        assert!(
            err.to_string().contains("removing missing.key"),
            "err: {err}"
        );
        let untouched = apply_toml(base, &[]).expect("no ops");
        assert_eq!(untouched, base);
    }

    #[test]
    fn keyvalue_apply_preserves_comments_and_repeats() {
        let base = "# ghostty\nfont-size = 13\nkeybind = a\nkeybind = b\n\n[sec]\nk = v\n";
        let ops = vec![
            Op::Replace {
                path: "font-size".to_owned(),
                from: json!("13"),
                to: json!("14"),
            },
            Op::Add {
                path: "keybind.2".to_owned(),
                value: json!("c"),
            },
            Op::Replace {
                path: "sec.k".to_owned(),
                from: json!("v"),
                to: json!("w"),
            },
        ];
        let mut lines: Vec<String> = base.lines().map(str::to_owned).collect();
        for op in &ops {
            match op {
                Op::Add { path, value }
                | Op::Replace {
                    path, to: value, ..
                } => {
                    set_keyvalue(&mut lines, path, value).expect("set");
                }
                _ => unreachable!(),
            }
        }
        let out = lines.join("\n");
        assert!(out.contains("# ghostty"), "out: {out}");
        assert!(out.contains("font-size = 14"), "out: {out}");
        assert!(out.contains("keybind = c"), "out: {out}");
        assert!(out.contains("k = w"), "out: {out}");
        // The new keybind lands right after the existing ones.
        let keybinds: Vec<&str> = out
            .lines()
            .filter(|line| line.starts_with("keybind"))
            .collect();
        assert_eq!(keybinds, vec!["keybind = a", "keybind = b", "keybind = c"]);
    }

    #[test]
    fn keyvalue_diff_round_trips_through_apply() {
        let base = "font-size = 13\n[sec]\nk = v\n";
        let edited = "font-size = 14\nnew-key = yes\n[sec]\nk = v\n";
        let ops = diff_bytes(Format::Keyvalue, base.as_bytes(), edited.as_bytes());
        let out = apply_keyvalue(base, &ops).expect("apply");
        assert!(
            diff_bytes(Format::Keyvalue, out.as_bytes(), edited.as_bytes()).is_empty(),
            "not logically equal after apply: {out}"
        );
    }

    #[test]
    fn plist_data_round_trips_through_hex() {
        let root = json!({"blob": "hex:00ff10", "name": "x"});
        let back = json_to_plist(&root).expect("convert");
        let dict = back.as_dictionary().expect("dict");
        assert_eq!(
            dict.get("blob"),
            Some(&plist::Value::Data(vec![0x00, 0xff, 0x10]))
        );
    }
}
