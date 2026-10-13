//! Workspace discovery and package.json parsing.

use anyhow::{Context, Result};
use rustc_hash::FxHashSet;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DepKind {
    Prod,
    Dev,
    Peer,
    Optional,
    OptionalPeer,
}

#[derive(Debug, Default)]
pub struct Manifest {
    pub path: PathBuf,
    pub name: Option<String>,
    pub deps: Vec<(String, DepKind, usize)>, // name, kind, byte offset in file
    pub scripts: Vec<(String, String)>,
    /// Command strings from hook/tool fields (lint-staged, husky, prisma.seed, wireit, ...).
    pub commands: Vec<String>,
    /// Every string value in package.json outside dependency maps and scripts, for heuristics.
    pub strings: Vec<String>,
    /// Entry file candidates from main/module/exports/bin/types (relative to package dir).
    pub entry_refs: Vec<String>,
    pub raw: Value,
    pub text: String,
}

impl Manifest {
    pub fn has_dep(&self, name: &str) -> bool {
        self.deps.iter().any(|(n, _, _)| n == name)
    }
    pub fn dep_names(&self) -> impl Iterator<Item = &str> {
        self.deps.iter().map(|(n, _, _)| n.as_str())
    }
}

pub fn read_manifest(path: &Path) -> Result<Manifest> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw: Value = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let mut m = Manifest { path: path.to_path_buf(), text, raw: raw.clone(), ..Default::default() };
    let obj = match raw.as_object() {
        Some(o) => o,
        None => return Ok(m),
    };
    m.name = obj.get("name").and_then(|v| v.as_str()).map(str::to_string);
    let dep_sections: &[(&str, DepKind)] =
        &[("dependencies", DepKind::Prod), ("devDependencies", DepKind::Dev), ("peerDependencies", DepKind::Peer), ("optionalDependencies", DepKind::Optional)];
    let peer_meta = obj.get("peerDependenciesMeta").and_then(|v| v.as_object());
    for (section, kind) in dep_sections {
        if let Some(Value::Object(deps)) = obj.get(*section) {
            for name in deps.keys() {
                let kind = if *kind == DepKind::Peer
                    && peer_meta.and_then(|pm| pm.get(name)).and_then(|v| v.get("optional")).and_then(|v| v.as_bool()).unwrap_or(false)
                {
                    DepKind::OptionalPeer
                } else {
                    *kind
                };
                let pos = find_key_offset(&m.text, section, name);
                m.deps.push((name.clone(), kind, pos));
            }
        }
    }
    if let Some(Value::Object(scripts)) = obj.get("scripts") {
        for (k, v) in scripts {
            if let Some(s) = v.as_str() {
                m.scripts.push((k.clone(), s.to_string()));
            }
        }
    }
    for key in ["lint-staged", "nano-staged", "pre-commit", "gitHooks", "simple-git-hooks", "betterScripts"] {
        if let Some(v) = obj.get(key) {
            collect_strings(v, &mut m.commands);
        }
    }
    for (outer, inner) in [("husky", "hooks"), ("prisma", "seed"), ("nodemonConfig", "exec")] {
        if let Some(v) = obj.get(outer).and_then(|o| o.get(inner)) {
            collect_strings(v, &mut m.commands);
        }
    }
    if let Some(Value::Object(w)) = obj.get("wireit") {
        for v in w.values() {
            if let Some(c) = v.get("command").and_then(|c| c.as_str()) {
                m.commands.push(c.to_string());
            }
        }
    }
    for key in ["main", "module", "browser", "types", "typings", "svelte", "source"] {
        if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
            m.entry_refs.push(s.to_string());
        }
    }
    if let Some(bin) = obj.get("bin") {
        match bin {
            Value::String(s) => m.entry_refs.push(s.clone()),
            Value::Object(o) => m.entry_refs.extend(o.values().filter_map(|v| v.as_str().map(str::to_string))),
            _ => {}
        }
    }
    if let Some(exports) = obj.get("exports") {
        collect_export_targets(exports, &mut m.entry_refs);
    }
    let skip: FxHashSet<&str> = [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
        "peerDependenciesMeta",
        "scripts",
        "name",
        "version",
        "description",
        "keywords",
        "author",
        "license",
        "repository",
        "homepage",
        "bugs",
        "packageManager",
        "engines",
        "overrides",
        "resolutions",
        "pnpm",
        "workspaces",
        "catalog",
        "catalogs",
        "patchedDependencies",
        "trustedDependencies",
        "files",
        "publishConfig",
        "funding",
        "contributors",
        "private",
        "type",
        "sideEffects",
    ]
    .into_iter()
    .collect();
    for (k, v) in obj {
        if !skip.contains(k.as_str()) {
            collect_strings(v, &mut m.strings);
        }
    }
    Ok(m)
}

fn collect_export_targets(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| collect_export_targets(x, out)),
        Value::Object(o) => o.values().for_each(|x| collect_export_targets(x, out)),
        _ => {}
    }
}

pub fn collect_strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| collect_strings(x, out)),
        Value::Object(o) => {
            for (k, x) in o {
                out.push(k.clone());
                collect_strings(x, out);
            }
        }
        _ => {}
    }
}

/// Byte offset of `"name"` inside the given section (best effort, for line/col reporting).
fn find_key_offset(text: &str, section: &str, name: &str) -> usize {
    let sec = format!("\"{section}\"");
    let Some(start) = text.find(&sec) else { return 0 };
    let key = format!("\"{name}\"");
    text[start..].find(&key).map(|i| start + i + 1).unwrap_or(0)
}

#[derive(Debug)]
pub struct Workspace {
    pub dir: PathBuf,
    /// Path relative to root, "." for the root.
    pub rel: String,
    pub manifest: Manifest,
    /// Index of the parent workspace (the nearest enclosing one).
    pub parent: Option<usize>,
}

impl Workspace {
    pub fn pkg_name(&self) -> Option<&str> {
        self.manifest.name.as_deref()
    }
}

/// Discover workspaces: root + any package dirs matched by workspace globs (npm/yarn/bun/pnpm).
/// Workspace globs that are negated (`!examples/vue/2*`): files under them are ignored entirely.
pub fn negated_globs(root: &Path) -> Vec<String> {
    let mut globs: Vec<String> = Vec::new();
    if let Ok(m) = read_manifest(&root.join("package.json")) {
        match m.raw.get("workspaces") {
            Some(Value::Array(a)) => globs.extend(a.iter().filter_map(|v| v.as_str().map(str::to_string))),
            Some(Value::Object(o)) => {
                if let Some(Value::Array(a)) = o.get("packages") {
                    globs.extend(a.iter().filter_map(|v| v.as_str().map(str::to_string)));
                }
            }
            _ => {}
        }
    }
    if let Ok(text) = std::fs::read_to_string(root.join("pnpm-workspace.yaml")) {
        if let Ok(Value::Object(o)) = serde_yaml::from_str::<Value>(&text) {
            if let Some(Value::Array(a)) = o.get("packages") {
                globs.extend(a.iter().filter_map(|v| v.as_str().map(str::to_string)));
            }
        }
    }
    globs.into_iter().filter(|g| g.starts_with('!')).map(|g| format!("{}/**", g[1..].trim_end_matches('/'))).collect()
}

pub fn discover(root: &Path) -> Result<Vec<Workspace>> {
    let root_manifest = read_manifest(&root.join("package.json"))?;
    let mut globs: Vec<String> = Vec::new();
    match root_manifest.raw.get("workspaces") {
        Some(Value::Array(a)) => globs.extend(a.iter().filter_map(|v| v.as_str().map(str::to_string))),
        Some(Value::Object(o)) => {
            if let Some(Value::Array(a)) = o.get("packages") {
                globs.extend(a.iter().filter_map(|v| v.as_str().map(str::to_string)));
            }
        }
        _ => {}
    }
    let pnpm = root.join("pnpm-workspace.yaml");
    if let Ok(text) = std::fs::read_to_string(&pnpm) {
        if let Ok(Value::Object(o)) = serde_yaml::from_str::<Value>(&text) {
            if let Some(Value::Array(a)) = o.get("packages") {
                globs.extend(a.iter().filter_map(|v| v.as_str().map(str::to_string)));
            }
        }
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    let (pos, neg): (Vec<&String>, Vec<&String>) = globs.iter().partition(|g| !g.starts_with('!'));
    let neg_set = crate::patterns::build_set(&neg.iter().map(|g| g[1..].to_string()).collect::<Vec<_>>())?;
    for g in pos {
        for d in expand_workspace_glob(root, g)? {
            let rel = d.strip_prefix(root).unwrap_or(&d).to_string_lossy().replace('\\', "/");
            if neg_set.is_match(&rel) {
                continue;
            }
            if d.join("package.json").is_file() && d != root {
                dirs.push(d);
            }
        }
    }
    dirs.sort();
    dirs.dedup();
    let mut workspaces = vec![Workspace { dir: root.to_path_buf(), rel: ".".into(), manifest: root_manifest, parent: None }];
    for d in dirs {
        let manifest = match read_manifest(&d.join("package.json")) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("warning: {e:#}");
                continue;
            }
        };
        let rel = d.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        workspaces.push(Workspace { dir: d, rel, manifest, parent: None });
    }
    // parents: nearest enclosing workspace
    let dirs: Vec<PathBuf> = workspaces.iter().map(|w| w.dir.clone()).collect();
    for i in 1..workspaces.len() {
        let mut best: Option<usize> = Some(0);
        for (j, d) in dirs.iter().enumerate() {
            if j != i && workspaces[i].dir.starts_with(d) && d != root && best.map(|b| d.components().count() > dirs[b].components().count()).unwrap_or(true) {
                best = Some(j);
            }
        }
        workspaces[i].parent = best;
    }
    Ok(workspaces)
}

/// Expand a workspace glob like `apps/*` or `packages/**` into directories.
fn expand_workspace_glob(root: &Path, glob: &str) -> Result<Vec<PathBuf>> {
    let glob = glob.trim_end_matches('/');
    let mut results = Vec::new();
    let parts: Vec<&str> = glob.split('/').collect();
    fn walk(dir: &Path, parts: &[&str], out: &mut Vec<PathBuf>) {
        if parts.is_empty() {
            out.push(dir.to_path_buf());
            return;
        }
        let part = parts[0];
        if part == "**" {
            // match zero or more dirs
            walk(dir, &parts[1..], out);
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() && !is_skipped_dir(&p) {
                        walk(&p, parts, out);
                    }
                }
            }
        } else if part.contains('*') || part.contains('?') || part.contains('[') {
            let Ok(g) = globset::Glob::new(part) else { return };
            let m = g.compile_matcher();
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() && !is_skipped_dir(&p) && m.is_match(e.file_name().to_string_lossy().as_ref()) {
                        walk(&p, &parts[1..], out);
                    }
                }
            }
        } else {
            let p = dir.join(part);
            if p.is_dir() {
                walk(&p, &parts[1..], out);
            }
        }
    }
    walk(root, &parts, &mut results);
    Ok(results)
}

fn is_skipped_dir(p: &Path) -> bool {
    matches!(p.file_name().and_then(|n| n.to_str()), Some("node_modules" | ".git"))
}

#[allow(dead_code)]
pub fn manifest_object(m: &Manifest) -> Option<&Map<String, Value>> {
    m.raw.as_object()
}
