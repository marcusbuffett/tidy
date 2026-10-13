//! Module graph: parse, resolve, reachability and export demand propagation.

use crate::extract::{self, ExportKind, FileFacts, ImportKind, Members, Names};
use crate::patterns::is_source_ext;
use crate::project::Project;
use crate::resolve::{Resolved, Resolvers, Target};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::path::{Path, PathBuf};

pub struct Module {
    pub path: PathBuf,
    pub rel: String,
    pub ws: usize,
    pub facts: FileFacts,
    /// (import index, resolution). Glob imports expand to several entries.
    pub edges: Vec<(usize, Resolved)>,
    pub is_entry: bool,
    pub is_project: bool,
    pub is_ignored: bool,
    pub is_config: bool,
    pub dev_entry: bool,
    pub tool_entry: bool,
    /// Workspace setting: report exports of entry files too.
    pub include_entry_exports: bool,
    pub reachable: bool,
    /// Per export: demanded members (None = never imported).
    pub export_used: Vec<Option<Members>>,
}

pub struct Graph {
    pub modules: Vec<Module>,
    pub by_path: FxHashMap<PathBuf, usize>,
    /// (module, export name) → defining (module, export index), through `export *` chains.
    memo: FxHashMap<(usize, String), Option<(usize, usize)>>,
}

pub struct Timings {
    pub parse_ms: f64,
    pub resolve_ms: f64,
    pub analyze_ms: f64,
    pub parsed_files: usize,
    pub extra_files: usize,
}

fn is_code(path: &Path) -> bool {
    path.to_str().map(is_source_ext).unwrap_or(false) || extract::is_component_file(path)
}

/// Collect quoted strings from non-code config files (json/yaml/toml/...).
fn strings_from_text(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'"' || c == b'\'' {
            let mut j = i + 1;
            while j < b.len() && b[j] != c && b[j] != b'\n' {
                if b[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            if j < b.len() && b[j] == c {
                if let Ok(s) = std::str::from_utf8(&b[i + 1..j]) {
                    if !s.is_empty() && s.len() < 200 {
                        out.push(s.to_string());
                    }
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    // YAML/TOML bare values: `extends: foo`, `- foo`, `key = foo`
    for line in text.lines() {
        let l = line.trim();
        let v = l.strip_prefix("- ").or_else(|| l.split_once(':').map(|(_, v)| v)).or_else(|| l.split_once('=').map(|(_, v)| v));
        if let Some(v) = v {
            let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
            if !v.is_empty() && !v.contains(' ') && v.len() < 120 {
                out.push(v.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

pub fn build(project: &Project, resolvers: &Resolvers, workspaces_dirs: &[PathBuf]) -> (Graph, Timings) {
    let t0 = std::time::Instant::now();
    let root = &project.root;
    // Parse all project/entry/config files in parallel.
    let parsed: Vec<(usize, FileFacts)> = project
        .files
        .par_iter()
        .enumerate()
        .map(|(i, f)| {
            let t = std::time::Instant::now();
            let mut facts = if is_code(&f.path) {
                match std::fs::read_to_string(&f.path) {
                    Ok(src) => extract::extract(&f.path, &src, f.is_config),
                    Err(_) => FileFacts::default(),
                }
            } else if f.is_config {
                let strings = std::fs::read_to_string(&f.path).map(|t| strings_from_text(&t)).unwrap_or_default();
                FileFacts { strings, ..Default::default() }
            } else {
                FileFacts::default()
            };
            if f.is_config {
                add_config_refs(&mut facts);
            }
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            if ms > 3.0 && std::env::var_os("TIDY_DEBUG_PARSE").is_some() {
                eprintln!("slow parse {ms:.1}ms {} ({} bytes)", f.rel, std::fs::metadata(&f.path).map(|m| m.len()).unwrap_or(0));
            }
            (i, facts)
        })
        .collect();
    let mut modules: Vec<Module> = Vec::with_capacity(parsed.len());
    let mut by_path: FxHashMap<PathBuf, usize> = FxHashMap::default();
    for (i, facts) in parsed {
        let f = &project.files[i];
        let n = facts.exports.len();
        by_path.insert(f.path.clone(), modules.len());
        modules.push(Module {
            path: f.path.clone(),
            rel: f.rel.clone(),
            ws: f.ws,
            facts,
            edges: vec![],
            is_entry: f.is_entry,
            is_project: f.is_project,
            is_ignored: f.is_ignored,
            is_config: f.is_config,
            dev_entry: f.dev_entry,
            tool_entry: f.tool_entry,
            include_entry_exports: project.setups[f.ws].include_entry_exports,
            reachable: false,
            export_used: vec![None; n],
        });
    }
    let parsed_files = modules.len();
    let parse_ms = t0.elapsed().as_secs_f64() * 1000.0;
    crate::perf("parse:project", t0);

    // Resolve imports; newly discovered internal files are parsed and resolved in further rounds.
    let t1 = std::time::Instant::now();
    let mut resolve_ms = 0.0;
    let mut parse_extra_ms = 0.0;
    let all_rel_set: FxHashSet<&str> = project.all_rel.iter().map(|s| s.as_str()).collect();
    let mut pending: Vec<usize> = (0..modules.len()).collect();
    let mut extra_files = 0;
    while !pending.is_empty() {
        let tr = std::time::Instant::now();
        let results: Vec<(usize, Vec<(usize, Resolved)>)> = pending
            .par_iter()
            .map(|&mi| {
                let m = &modules[mi];
                let dir = m.path.parent().unwrap_or(root);
                let mut edges = Vec::with_capacity(m.facts.imports.len());
                for (ii, imp) in m.facts.imports.iter().enumerate() {
                    if imp.kind == ImportKind::Glob {
                        for p in expand_glob(root, dir, &imp.specifier, &project.all_rel, &all_rel_set) {
                            edges.push((ii, Resolved { package: None, target: Target::Internal(p) }));
                        }
                        continue;
                    }
                    edges.push((ii, resolvers.resolve(dir, &imp.specifier)));
                }
                (mi, edges)
            })
            .collect();
        resolve_ms += tr.elapsed().as_secs_f64() * 1000.0;
        let mut new_paths: Vec<PathBuf> = Vec::new();
        for (mi, edges) in results {
            for (_, r) in &edges {
                if let Target::Internal(p) = &r.target {
                    if !by_path.contains_key(p) {
                        by_path.insert(p.clone(), usize::MAX);
                        new_paths.push(p.clone());
                    }
                }
            }
            modules[mi].edges = edges;
        }
        let tp = std::time::Instant::now();
        let parsed: Vec<(PathBuf, FileFacts)> = new_paths
            .into_par_iter()
            .map(|p| {
                let t = std::time::Instant::now();
                let facts =
                    if is_code(&p) { std::fs::read_to_string(&p).map(|s| extract::extract_light(&p, &s)).unwrap_or_default() } else { FileFacts::default() };
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                if ms > 3.0 && std::env::var_os("TIDY_DEBUG_PARSE").is_some() {
                    eprintln!("slow parse {ms:.1}ms {} ({} bytes)", p.display(), std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0));
                }
                (p, facts)
            })
            .collect();
        parse_extra_ms += tp.elapsed().as_secs_f64() * 1000.0;
        crate::perf("parse:extra-round", tp);
        pending.clear();
        for (p, facts) in parsed {
            let ws = workspace_for(&p, workspaces_dirs);
            let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            let n = facts.exports.len();
            let id = modules.len();
            by_path.insert(p.clone(), id);
            modules.push(Module {
                path: p,
                rel,
                ws,
                facts,
                edges: vec![],
                is_entry: false,
                is_project: false,
                is_ignored: true,
                is_config: false,
                dev_entry: false,
                tool_entry: false,
                include_entry_exports: false,
                reachable: false,
                export_used: vec![None; n],
            });
            extra_files += 1;
            pending.push(id);
        }
    }
    let _ = t1;

    // Files referenced from config files (setup files, reporters, plugins) are consumed by the tool.
    let mut config_targets: Vec<usize> = Vec::new();
    for m in &modules {
        if !m.is_config {
            continue;
        }
        for (ii, r) in &m.edges {
            if matches!(m.facts.imports[*ii].kind, ImportKind::ConfigRef | ImportKind::Url | ImportKind::Glob | ImportKind::Dynamic) {
                if let Target::Internal(p) = &r.target {
                    if let Some(&t) = by_path.get(p) {
                        config_targets.push(t);
                    }
                }
            }
        }
    }
    for t in config_targets {
        let m = &mut modules[t];
        m.is_entry = true;
        m.tool_entry = true;
    }
    let t2 = std::time::Instant::now();
    let mut graph = Graph { modules, by_path, memo: FxHashMap::default() };
    graph.reachability();
    graph.propagate_exports();
    let analyze_ms = t2.elapsed().as_secs_f64() * 1000.0;
    (graph, Timings { parse_ms: parse_ms + parse_extra_ms, resolve_ms, analyze_ms, parsed_files, extra_files })
}

/// Relative path strings in config files (`setupFiles: ['./setup.ts']`, reporters, globalSetup...)
/// become edges so the referenced files count as used.
fn add_config_refs(facts: &mut FileFacts) {
    let existing: FxHashSet<String> = facts.imports.iter().map(|i| i.specifier.clone()).collect();
    let strings = facts.strings.clone();
    for s in strings {
        if s.contains(' ') || s.len() > 200 || s.is_empty() || existing.contains(&s) {
            continue;
        }
        if s.contains('*') {
            // build/test entry globs only under entry-like keys (`entry: ['src/*.ts']`, rollup `input`, storybook `stories`)
            const ENTRY_KEYS: &[&str] =
                &["entry", "entries", "input", "entryPoints", "stories", "testMatch", "setupFiles", "globalSetup", "import", "require", "schema", "paths"];
            let keyed = facts.keyed_strings.iter().any(|(k, v)| v == &s && ENTRY_KEYS.contains(&k.as_str()));
            if !keyed || s.starts_with('!') {
                continue;
            }
            facts.imports.push(extract::Import {
                specifier: s,
                kind: ImportKind::Glob,
                is_type: false,
                names: Names::Named(vec![]),
                start: 0,
                line: 0,
                col: 0,
            });
            continue;
        }
        // vite `root: "./app"` → the html entry under that root
        if facts.keyed_strings.iter().any(|(k, v)| k == "root" && v == &s) && !s.contains('*') {
            let spec = format!("./{}/index.html", s.trim_start_matches("./").trim_end_matches('/'));
            if !existing.contains(&spec) {
                facts.imports.push(extract::Import {
                    specifier: spec,
                    kind: ImportKind::ConfigRef,
                    is_type: false,
                    names: Names::Named(vec![]),
                    start: 0,
                    line: 0,
                    col: 0,
                });
            }
        }
        let relative = s.starts_with("./") || s.starts_with("../");
        // bare file names like `test-setup.ts` or `src/setup.ts` are resolved relative to the config file
        let looks_like_file = !relative
            && !s.starts_with('/')
            && !s.starts_with('@')
            && !s.starts_with('#')
            && !s.contains(':')
            && is_source_ext(&s)
            && !s.starts_with("node_modules");
        if !relative && !looks_like_file {
            continue;
        }
        let spec = if relative { s } else { format!("./{s}") };
        facts.imports.push(extract::Import {
            specifier: spec,
            kind: ImportKind::ConfigRef,
            is_type: false,
            names: Names::Named(vec![]),
            start: 0,
            line: 0,
            col: 0,
        });
    }
}

fn workspace_for(p: &Path, dirs: &[PathBuf]) -> usize {
    let mut best = 0;
    let mut best_len = 0;
    for (i, d) in dirs.iter().enumerate() {
        if p.starts_with(d) {
            let n = d.components().count();
            if n > best_len {
                best_len = n;
                best = i;
            }
        }
    }
    best
}

fn expand_glob(root: &Path, dir: &Path, pattern: &str, all_rel: &[String], _set: &FxHashSet<&str>) -> Vec<PathBuf> {
    let negated = pattern.starts_with('!');
    if negated {
        return vec![];
    }
    let base = if pattern.starts_with('/') { root.to_path_buf() } else { dir.to_path_buf() };
    let joined = base.join(pattern.trim_start_matches('/'));
    // normalise `..` segments
    let mut norm = PathBuf::new();
    for c in joined.components() {
        match c {
            std::path::Component::ParentDir => {
                norm.pop();
            }
            std::path::Component::CurDir => {}
            other => norm.push(other),
        }
    }
    let Ok(rel) = norm.strip_prefix(root) else { return vec![] };
    let rel = rel.to_string_lossy().replace('\\', "/");
    let Ok(g) = crate::patterns::build_glob(&rel) else { return vec![] };
    let m = g.compile_matcher();
    all_rel.iter().filter(|r| m.is_match(r.as_str())).map(|r| root.join(r)).collect()
}

impl Graph {
    fn reachability(&mut self) {
        let mut stack: Vec<usize> = self.modules.iter().enumerate().filter(|(_, m)| m.is_entry).map(|(i, _)| i).collect();
        for &i in &stack {
            self.modules[i].reachable = true;
        }
        while let Some(i) = stack.pop() {
            let targets: Vec<usize> = self.modules[i]
                .edges
                .iter()
                .filter_map(|(_, r)| match &r.target {
                    Target::Internal(p) => self.by_path.get(p).copied(),
                    _ => None,
                })
                .collect();
            for t in targets {
                if !self.modules[t].reachable {
                    self.modules[t].reachable = true;
                    stack.push(t);
                }
            }
        }
    }

    fn targets_of_import(&self, m: usize, import: usize) -> Vec<usize> {
        self.modules[m]
            .edges
            .iter()
            .filter(|(ii, _)| *ii == import)
            .filter_map(|(_, r)| match &r.target {
                Target::Internal(p) => self.by_path.get(p).copied(),
                _ => None,
            })
            .collect()
    }

    /// Where `name` exported from module `t` is actually defined, following `export * from` chains.
    fn find_export(
        &self,
        t: usize,
        name: &str,
        memo: &mut FxHashMap<(usize, String), Option<(usize, usize)>>,
        visiting: &mut FxHashSet<usize>,
    ) -> Option<(usize, usize)> {
        if let Some(i) = self.modules[t].facts.exports.iter().position(|e| e.name == name) {
            return Some((t, i));
        }
        let key = (t, name.to_string());
        if let Some(v) = memo.get(&key) {
            return *v;
        }
        if !visiting.insert(t) {
            return None;
        }
        let mut found = None;
        let alls: Vec<usize> = self.modules[t].facts.export_alls.iter().map(|ea| ea.import).collect();
        'outer: for imp in alls {
            for t2 in self.targets_of_import(t, imp) {
                if let Some(r) = self.find_export(t2, name, memo, visiting) {
                    found = Some(r);
                    break 'outer;
                }
            }
        }
        visiting.remove(&t);
        memo.insert(key, found);
        found
    }

    fn propagate_exports(&mut self) {
        let mut all_marked: Vec<bool> = vec![false; self.modules.len()];
        let mut visiting: FxHashMap<(usize, String), Option<(usize, usize)>> = std::mem::take(&mut self.memo);
        // `export * from './x'` in an entry file publishes x's exports (knip: reExportingEntryFile).
        for m in 0..self.modules.len() {
            if !self.modules[m].is_entry || self.modules[m].include_entry_exports {
                continue;
            }
            let alls: Vec<usize> = self.modules[m].facts.export_alls.iter().map(|ea| ea.import).collect();
            for imp in alls {
                for t in self.targets_of_import(m, imp) {
                    self.mark_all_inner(t, &mut all_marked, &mut visiting);
                }
            }
        }
        for m in 0..self.modules.len() {
            let edges: Vec<(usize, Option<usize>)> = self.modules[m]
                .edges
                .iter()
                .map(|(ii, r)| {
                    (
                        *ii,
                        match &r.target {
                            Target::Internal(p) => self.by_path.get(p).copied(),
                            _ => None,
                        },
                    )
                })
                .collect();
            for (ii, target) in edges {
                let Some(t) = target else { continue };
                let names = self.modules[m].facts.imports[ii].names.clone();
                match names {
                    Names::All => self.mark_all_inner(t, &mut all_marked, &mut visiting),
                    Names::Named(list) => {
                        for n in list {
                            if n.name == "*" {
                                match n.members {
                                    Members::All => self.mark_all_inner(t, &mut all_marked, &mut visiting),
                                    Members::Named(ms) => {
                                        for mm in ms {
                                            self.demand(t, &mm, &Members::All, &mut all_marked, &mut visiting);
                                        }
                                    }
                                }
                            } else {
                                self.demand(t, &n.name, &n.members, &mut all_marked, &mut visiting);
                            }
                        }
                    }
                }
            }
        }
        self.memo = visiting;
    }

    fn mark_all_inner(&mut self, t: usize, all_marked: &mut Vec<bool>, memo: &mut FxHashMap<(usize, String), Option<(usize, usize)>>) {
        if all_marked[t] {
            return;
        }
        all_marked[t] = true;
        let n = self.modules[t].facts.exports.len();
        let mut forwards: Vec<(usize, String)> = Vec::new();
        for i in 0..n {
            self.modules[t].export_used[i] = Some(Members::All);
            if let Some((imp, name)) = &self.modules[t].facts.exports[i].reexport {
                forwards.push((*imp, name.clone()));
            }
        }
        for ea in self.modules[t].facts.export_alls.clone() {
            forwards.push((ea.import, "*".into()));
        }
        for (imp, name) in forwards {
            for t2 in self.targets_of_import(t, imp) {
                if name == "*" {
                    self.mark_all_inner(t2, all_marked, memo);
                } else {
                    self.demand(t2, &name, &Members::All, all_marked, memo);
                }
            }
        }
    }

    fn demand(
        &mut self,
        t: usize,
        name: &str,
        members: &Members,
        all_marked: &mut Vec<bool>,
        visiting: &mut FxHashMap<(usize, String), Option<(usize, usize)>>,
    ) {
        let mut guard = FxHashSet::default();
        let Some((t, i)) = self.find_export(t, name, visiting, &mut guard) else { return };
        {
            let changed = match &mut self.modules[t].export_used[i] {
                Some(existing) => existing.merge(members),
                slot @ None => {
                    *slot = Some(members.clone());
                    true
                }
            };
            if !changed {
                return;
            }
            if let Some((imp, imported)) = self.modules[t].facts.exports[i].reexport.clone() {
                for t2 in self.targets_of_import(t, imp) {
                    if imported == "*" {
                        match members {
                            Members::All => self.mark_all_inner(t2, all_marked, visiting),
                            Members::Named(ms) => {
                                for mm in ms.clone() {
                                    self.demand(t2, &mm, &Members::All, all_marked, visiting);
                                }
                            }
                        }
                    } else {
                        self.demand(t2, &imported, members, all_marked, visiting);
                    }
                }
            }
        }
    }

    #[allow(dead_code)]
    pub fn module_kind(&self, i: usize) -> ExportKind {
        let _ = i;
        ExportKind::Unknown
    }
}
