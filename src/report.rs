//! Reporters: human-readable text and knip-compatible JSON.

use crate::issues::{Issue, IssueKind};
use rustc_hash::FxHashMap;
use serde_json::{json, Map, Value};
use std::fmt::Write as _;

pub fn text(issues: &[Issue]) -> String {
    let mut out = String::new();
    for kind in IssueKind::ALL {
        let mut list: Vec<&Issue> = issues.iter().filter(|i| i.kind == *kind).collect();
        if list.is_empty() {
            continue;
        }
        list.sort_by(|a, b| (&a.file, a.line, a.col, &a.name).cmp(&(&b.file, b.line, b.col, &b.name)));
        let _ = writeln!(out, "{} ({})", kind.title(), list.len());
        let width = list.iter().map(|i| display_name(i).len()).max().unwrap_or(0).min(60);
        for i in list {
            match kind {
                IssueKind::Files => {
                    let _ = writeln!(out, "{}", i.file);
                }
                IssueKind::Dependencies | IssueKind::DevDependencies | IssueKind::OptionalPeerDependencies | IssueKind::Binaries => {
                    let _ = writeln!(out, "{:<width$}  {}", display_name(i), i.file, width = width);
                }
                _ => {
                    let loc = if i.line > 0 { format!("{}:{}:{}", i.file, i.line, i.col) } else { i.file.clone() };
                    let _ = writeln!(out, "{:<width$}  {}", display_name(i), loc, width = width);
                }
            }
        }
    }
    out
}

fn display_name(i: &Issue) -> String {
    match &i.parent {
        Some(p) => format!("{}.{}", p, i.name),
        None => i.name.clone(),
    }
}

/// knip-compatible JSON: `{ files: [...], issues: [{ file, dependencies: [...], ... }] }`
pub fn json(issues: &[Issue]) -> String {
    let mut files: Vec<&str> = issues.iter().filter(|i| i.kind == IssueKind::Files).map(|i| i.file.as_str()).collect();
    files.sort();
    let mut per_file: FxHashMap<&str, Map<String, Value>> = FxHashMap::default();
    let mut order: Vec<&str> = Vec::new();
    for i in issues {
        if i.kind == IssueKind::Files {
            continue;
        }
        let entry = per_file.entry(i.file.as_str()).or_insert_with(|| {
            order.push(i.file.as_str());
            let mut m = Map::new();
            m.insert("file".into(), Value::String(i.file.clone()));
            for k in ["dependencies", "devDependencies", "optionalPeerDependencies", "unlisted", "binaries", "unresolved", "exports", "types"] {
                m.insert(k.into(), Value::Array(vec![]));
            }
            m.insert("enumMembers".into(), Value::Object(Map::new()));
            m.insert("classMembers".into(), Value::Object(Map::new()));
            m.insert("duplicates".into(), Value::Array(vec![]));
            m
        });
        let item = json!({ "name": i.name, "line": i.line, "col": i.col, "pos": i.pos });
        match i.kind {
            IssueKind::EnumMembers | IssueKind::ClassMembers => {
                let parent = i.parent.clone().unwrap_or_default();
                let obj = entry.get_mut(i.kind.key()).unwrap().as_object_mut().unwrap();
                obj.entry(parent).or_insert_with(|| Value::Array(vec![])).as_array_mut().unwrap().push(item);
            }
            _ => {
                entry.get_mut(i.kind.key()).unwrap().as_array_mut().unwrap().push(item);
            }
        }
    }
    order.sort();
    let issues_json: Vec<Value> = order.iter().map(|f| Value::Object(per_file.remove(f).unwrap())).collect();
    serde_json::to_string(&json!({ "files": files, "issues": issues_json })).unwrap()
}

/// Compact machine-friendly JSON: a flat list of issues.
pub fn json_flat(issues: &[Issue]) -> String {
    let list: Vec<Value> =
        issues.iter().map(|i| json!({ "type": i.kind.key(), "file": i.file, "name": display_name(i), "line": i.line, "col": i.col })).collect();
    serde_json::to_string(&list).unwrap()
}
