//! What a configuration loses on its way through IntelliJ's XML, kept in
//! `.idea/openidea/*.toml` next to it. Instead of listing fields per type, the
//! saver round-trips the config through the XML and stores the difference, so
//! whatever a type mapping can't hold is never lost.

use crate::RunConfig;
use ide_core::IdeResult;
use serde_json::{Map, Value as Json};
use std::collections::BTreeMap;
use std::path::Path;
use toml_edit::DocumentMut;

pub(crate) type Entries = BTreeMap<String, toml::Table>;

const UNSET: &str = "unset";
const PARAMS: &str = "params";
const PARAMS_UNSET: &str = "paramsUnset";

fn object(c: &RunConfig) -> Map<String, Json> {
    match serde_json::to_value(c) {
        Ok(Json::Object(mut m)) => {
            // hmm, `cwd: None` serializes as null; treat it like an absent key
            m.retain(|_, v| !v.is_null());
            m
        }
        _ => Map::new(),
    }
}

fn params_of(m: &Map<String, Json>) -> Map<String, Json> {
    m.get(PARAMS).and_then(Json::as_object).cloned().unwrap_or_default()
}

/// What `original` has that `lossy` (the XML round trip) doesn't; `None` when nothing.
pub(crate) fn diff(original: &RunConfig, lossy: &RunConfig) -> Option<toml::Table> {
    let o = object(original);
    let l = object(lossy);
    let mut out = Map::new();
    for (k, v) in &o {
        if k != PARAMS && k != "detected" && l.get(k) != Some(v) {
            out.insert(k.clone(), v.clone());
        }
    }
    let unset: Vec<Json> =
        l.keys().filter(|k| *k != PARAMS && !o.contains_key(*k)).map(|k| Json::String(k.clone())).collect();
    if !unset.is_empty() {
        out.insert(UNSET.into(), Json::Array(unset));
    }
    let (op, lp) = (params_of(&o), params_of(&l));
    let changed: Map<String, Json> =
        op.iter().filter(|(k, v)| lp.get(*k) != Some(v)).map(|(k, v)| (k.clone(), v.clone())).collect();
    if !changed.is_empty() {
        out.insert(PARAMS.into(), Json::Object(changed));
    }
    let removed: Vec<Json> = lp.keys().filter(|k| !op.contains_key(*k)).map(|k| Json::String(k.clone())).collect();
    if !removed.is_empty() {
        out.insert(PARAMS_UNSET.into(), Json::Array(removed));
    }
    if out.is_empty() {
        return None;
    }
    match toml::Value::try_from(Json::Object(out)) {
        Ok(toml::Value::Table(t)) => Some(t),
        _ => None,
    }
}

/// The id a sidecar entry pins, if any (it decides references before the rest applies).
pub(crate) fn id_of(entry: &toml::Table) -> Option<&str> {
    entry.get("id").and_then(toml::Value::as_str)
}

/// `base` with `entry` applied on top; an entry that no longer fits is ignored.
pub(crate) fn apply(base: RunConfig, entry: &toml::Table) -> RunConfig {
    let Ok(Json::Object(d)) = serde_json::to_value(entry) else { return base };
    let mut m = object(&base);
    m.insert("detected".into(), Json::Bool(false));
    let mut params = params_of(&m);
    for (k, v) in d {
        match k.as_str() {
            UNSET => {
                for key in v.as_array().into_iter().flatten().filter_map(Json::as_str) {
                    m.remove(key);
                }
            }
            PARAMS => params.extend(v.as_object().cloned().unwrap_or_default()),
            PARAMS_UNSET => {
                for key in v.as_array().into_iter().flatten().filter_map(Json::as_str) {
                    params.remove(key);
                }
            }
            _ => {
                m.insert(k, v);
            }
        }
    }
    m.insert(PARAMS.into(), Json::Object(params));
    match serde_json::from_value(Json::Object(m)) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("ignoring a stale run configuration sidecar entry for {}: {e}", base.name);
            base
        }
    }
}

/// The `[<table>."<key>"]` entries of a sidecar file.
pub(crate) fn read(file: &Path, table: &str) -> Entries {
    let Ok(text) = std::fs::read_to_string(file) else { return Entries::new() };
    match toml::from_str::<toml::Table>(&text) {
        Ok(mut doc) => match doc.remove(table) {
            Some(toml::Value::Table(t)) => t
                .into_iter()
                .filter_map(|(k, v)| match v {
                    toml::Value::Table(e) => Some((k, e)),
                    _ => None,
                })
                .collect(),
            _ => Entries::new(),
        },
        Err(e) => {
            tracing::warn!("{} is not valid TOML, ignoring it: {e}", file.display());
            Entries::new()
        }
    }
}

/// A string array at the top of a sidecar file.
pub(crate) fn read_list(file: &Path, key: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(file) else { return Vec::new() };
    let Ok(doc) = toml::from_str::<toml::Table>(&text) else { return Vec::new() };
    doc.get(key)
        .and_then(toml::Value::as_array)
        .map(|a| a.iter().filter_map(toml::Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default()
}

pub(crate) fn write_list(file: &Path, key: &str, values: &[String]) -> IdeResult<()> {
    edit(file, |doc| {
        if values.is_empty() {
            doc.remove(key);
        } else {
            doc.insert(key, toml_edit::value(values.iter().collect::<toml_edit::Array>()));
        }
        Ok(())
    })
}

/// Replaces `[<table>]` in the file, keeping its other tables. A file left
/// with nothing in it is removed; nothing is created for no entries.
pub(crate) fn write(file: &Path, table: &str, entries: &Entries) -> IdeResult<()> {
    edit(file, |doc| {
        if entries.is_empty() {
            doc.remove(table);
            return Ok(());
        }
        let wrapped = BTreeMap::from([(table, entries)]);
        let text = toml::to_string(&wrapped).map_err(|e| ide_core::IdeError::internal(e.to_string()))?;
        let fresh: DocumentMut = text.parse().map_err(|e| ide_core::IdeError::internal(format!("{e}")))?;
        if let Some(item) = fresh.get(table) {
            doc.insert(table, item.clone());
        }
        Ok(())
    })
}

fn edit(file: &Path, f: impl FnOnce(&mut DocumentMut) -> IdeResult<()>) -> IdeResult<()> {
    let existing = std::fs::read_to_string(file).ok();
    let mut doc = existing.as_deref().and_then(|t| t.parse::<DocumentMut>().ok()).unwrap_or_default();
    f(&mut doc)?;
    let text = doc.to_string();
    match existing {
        None if doc.is_empty() => Ok(()),
        Some(_) if doc.is_empty() => Ok(std::fs::remove_file(file)?),
        Some(old) if old == text => Ok(()),
        _ => ide_core::idea_xml::write_atomic(file, &text),
    }
}
