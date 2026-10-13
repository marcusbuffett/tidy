//! Walk the repository, assign files to workspaces, classify entry/project/config files.

use crate::config::{plugin_override, PluginOverride, RawConfig, RawWorkspaceConfig};
use crate::patterns::{self, PatternSet, Tool, TOOLS};
use crate::scripts;
use crate::workspace::Workspace;
use anyhow::Result;
use rustc_hash::{FxHashMap, FxHashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct ProjectFile {
    pub path: PathBuf,
    /// Root-relative posix path.
    pub rel: String,
    pub ws: usize,
    pub is_project: bool,
    pub is_entry: bool,
    pub is_config: bool,
    pub is_ignored: bool,
    /// Entry only through a dev tool (tests, stories, lint configs...).
    pub dev_entry: bool,
    /// Entry only because a tool convention says so (its exports are consumed by the tool).
    pub tool_entry: bool,
}

pub struct WorkspaceSetup {
    /// (include, exclude) for `.d.ts` files from tsconfig.
    pub dts: Option<(PatternSet, PatternSet)>,
    pub project: PatternSet,
    /// Entries from config / defaults (exports reported when includeEntryExports).
    pub entry: PatternSet,
    /// Entries from tool conventions (production tools).
    pub tool_entry: PatternSet,
    pub dev_entry: PatternSet,
    pub ignore: PatternSet,
    pub config: PatternSet,
    pub exact_entries: FxHashSet<String>,
    pub enabled_tools: Vec<usize>,
    /// Enabled tools that have a config file in this workspace (their enabler package is in use).
    pub tools_with_config: Vec<usize>,
    pub ignore_dependencies: Vec<crate::config::NameMatcher>,
    pub ignore_binaries: Vec<crate::config::NameMatcher>,
    pub ignore_unresolved: Vec<crate::config::NameMatcher>,
    pub include_entry_exports: bool,
    pub ignore_exports_used_in_file: ExportsUsedInFile,
    pub is_ignored_workspace: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ExportsUsedInFile {
    pub all: bool,
    pub kinds: Vec<String>,
}

impl ExportsUsedInFile {
    fn from_value(v: Option<&serde_json::Value>) -> ExportsUsedInFile {
        match v {
            Some(serde_json::Value::Bool(b)) => ExportsUsedInFile { all: *b, kinds: vec![] },
            Some(serde_json::Value::Object(o)) => {
                ExportsUsedInFile { all: false, kinds: o.iter().filter(|(_, v)| v.as_bool().unwrap_or(false)).map(|(k, _)| k.clone()).collect() }
            }
            _ => ExportsUsedInFile::default(),
        }
    }
    pub fn applies(&self, kind: &str) -> bool {
        self.all || self.kinds.iter().any(|k| k == kind)
    }
}

pub struct Project {
    pub root: PathBuf,
    pub files: Vec<ProjectFile>,
    pub setups: Vec<WorkspaceSetup>,
    /// All files (root-relative) for glob-import expansion.
    pub all_rel: Vec<String>,
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let out: Mutex<Vec<PathBuf>> = Mutex::new(Vec::with_capacity(4096));
    let mut builder = ignore::WalkBuilder::new(root);
    builder.hidden(false).git_ignore(true).git_global(false).git_exclude(true).require_git(false).follow_links(false).filter_entry(|e| {
        let name = e.file_name();
        name != "node_modules" && name != ".git" && name != ".hg" && name != ".svn" && name != ".yarn" && name != ".pnpm-store"
    });
    builder.build_parallel().run(|| {
        Box::new(|entry| {
            if let Ok(e) = entry {
                if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    out.lock().unwrap().push(e.into_path());
                }
            }
            ignore::WalkState::Continue
        })
    });
    let mut v = out.into_inner().unwrap();
    v.sort();
    v
}

fn rel_posix(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

fn ws_config<'c>(cfg: &'c RawConfig, ws: &Workspace) -> Option<&'c RawWorkspaceConfig> {
    if ws.rel == "." {
        return Some(cfg.workspaces.get(".").unwrap_or(&cfg.root));
    }
    if let Some(c) = cfg.workspaces.get(&ws.rel) {
        return Some(c);
    }
    // glob keys like `apps/workers/*`
    for (k, c) in &cfg.workspaces {
        if k.contains('*') {
            if let Ok(g) = patterns::build_glob(k) {
                if g.compile_matcher().is_match(&ws.rel) {
                    return Some(c);
                }
            }
        }
    }
    None
}

fn tool_enabled(tool: &Tool, ws_idx: usize, workspaces: &[Workspace]) -> bool {
    let mut cur = Some(ws_idx);
    while let Some(i) = cur {
        let m = &workspaces[i].manifest;
        for e in tool.enablers {
            if let Some(prefix) = e.strip_suffix('/') {
                if m.dep_names().any(|n| n.starts_with(prefix) && n.len() > prefix.len() && n.as_bytes()[prefix.len()] == b'/') {
                    return true;
                }
            } else if m.has_dep(e) {
                return true;
            }
        }
        cur = workspaces[i].parent;
    }
    false
}

/// Map a package.json entry ref (`dist/index.js`) to an existing source file if possible.
fn map_entry_ref(ws_dir: &Path, r: &str) -> Option<String> {
    let r = r.trim_start_matches("./");
    if r.is_empty() || r.contains('*') || r.starts_with("http") {
        return None;
    }
    if ws_dir.join(r).is_file() {
        return Some(r.to_string());
    }
    // dist/foo.js → src/foo.ts etc.
    crate::resolve::map_dist_to_src(ws_dir, r).and_then(|p| p.strip_prefix(ws_dir).ok().map(|x| x.to_string_lossy().replace('\\', "/")))
}

/// tsconfig `include`/`files`/`exclude` as pattern sets (for declaration-file entries).
fn tsconfig_dts_patterns(dir: &Path) -> Option<(PatternSet, PatternSet)> {
    let text = std::fs::read_to_string(dir.join("tsconfig.json")).ok()?;
    let v: serde_json::Value = json5::from_str(&text).ok()?;
    let list =
        |key: &str| -> Option<Vec<String>> { v.get(key).and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()) };
    let mut include: Vec<String> = list("include").unwrap_or_else(|| vec!["**/*".into()]);
    include.extend(list("files").unwrap_or_default());
    let include: Vec<String> = include
        .into_iter()
        .map(|p| {
            let p = p.trim_start_matches("./").trim_end_matches('/').to_string();
            if p == "." || p.is_empty() {
                "**/*".to_string()
            } else if !p.contains('*') && !p.rsplit('/').next().unwrap_or("").contains('.') {
                format!("{p}/**/*")
            } else {
                p
            }
        })
        .collect();
    let mut exclude: Vec<String> = list("exclude").unwrap_or_else(|| vec!["node_modules".into()]);
    exclude = exclude
        .into_iter()
        .map(|p| {
            let p = p.trim_start_matches("./").trim_end_matches('/').to_string();
            if !p.contains('*') && !p.rsplit('/').next().unwrap_or("").contains('.') {
                format!("{p}/**")
            } else {
                p
            }
        })
        .collect();
    Some((PatternSet::new(&include).ok()?, PatternSet::new(&exclude).ok()?))
}

pub fn build(root: &Path, cfg: &RawConfig, workspaces: &[Workspace], production: bool) -> Result<Project> {
    let t = std::time::Instant::now();
    let all = walk(root);
    crate::perf("walk", t);
    let t = std::time::Instant::now();
    let all_rel: Vec<String> = all.iter().map(|p| rel_posix(root, p)).collect();

    // Assign files to the deepest workspace: walk up parent directories with a dir → workspace map,
    // memoising per directory.
    let ws_dirs: FxHashMap<&Path, usize> = workspaces.iter().enumerate().map(|(i, w)| (w.dir.as_path(), i)).collect();
    let mut dir_ws: FxHashMap<PathBuf, usize> = FxHashMap::default();
    let mut file_ws: Vec<usize> = Vec::with_capacity(all.len());
    for p in &all {
        let dir = p.parent().unwrap_or(root);
        if let Some(&w) = dir_ws.get(dir) {
            file_ws.push(w);
            continue;
        }
        let mut cur = Some(dir);
        let mut found = 0;
        let mut visited: Vec<&Path> = Vec::new();
        while let Some(d) = cur {
            if let Some(&w) = dir_ws.get(d) {
                found = w;
                break;
            }
            visited.push(d);
            if let Some(&w) = ws_dirs.get(d) {
                found = w;
                break;
            }
            if d == root {
                break;
            }
            cur = d.parent();
        }
        for d in visited {
            dir_ws.insert(d.to_path_buf(), found);
        }
        file_ws.push(found);
    }

    // Root-level ignore applies to all workspaces (root-relative).
    let root_ignore = PatternSet::new(&cfg.root.ignore.as_ref().map(|s| s.to_vec()).unwrap_or_default())?;
    let negated_ws = PatternSet::new(&crate::workspace::negated_globs(root))?;
    let ignore_ws_set = patterns::build_set(&cfg.ignore_workspaces)?;

    // One combined globset over every tool's config patterns: a single match per file tells which tools.
    let mut combined = globset::GlobSetBuilder::new();
    let mut glob_tool: Vec<usize> = Vec::new();
    for (ti, t) in TOOLS.iter().enumerate() {
        for c in t.config {
            for e in patterns::expand_braces(&patterns::normalize_glob(c)) {
                combined.add(globset::GlobBuilder::new(&e).literal_separator(true).build()?);
                glob_tool.push(ti);
            }
        }
    }
    let combined = combined.build()?;
    // Pre-filter by basename: most config patterns end in a literal file name.
    let mut literal_names: FxHashSet<String> = FxHashSet::default();
    let mut has_wildcard_tail = false;
    for t in TOOLS {
        for c in t.config {
            for e in patterns::expand_braces(&patterns::normalize_glob(c)) {
                let tail = e.rsplit('/').next().unwrap_or(&e).to_string();
                if tail.contains(['*', '?', '[']) {
                    has_wildcard_tail = true;
                } else {
                    literal_names.insert(tail);
                }
            }
        }
    }

    // Pass A: which tools have config files present per workspace.
    let mut config_present: Vec<FxHashSet<usize>> = vec![FxHashSet::default(); workspaces.len()];
    let ws_rels: Vec<String> = all.iter().enumerate().map(|(i, p)| rel_posix(&workspaces[file_ws[i]].dir, p)).collect();
    for (i, ws_rel) in ws_rels.iter().enumerate() {
        let base = ws_rel.rsplit('/').next().unwrap_or(ws_rel);
        let candidate = literal_names.contains(base)
            || (has_wildcard_tail
                && (ws_rel.starts_with('.') || ws_rel.contains("/.") || ws_rel.ends_with(".prisma") || ws_rel.ends_with(".yml") || ws_rel.ends_with(".yaml")));
        if !candidate {
            continue;
        }
        for gi in combined.matches(ws_rel) {
            config_present[file_ws[i]].insert(glob_tool[gi]);
        }
    }

    crate::perf("assign+toolscan", t);
    let t = std::time::Instant::now();
    let default_entry: Vec<String> =
        ["{index,cli,main}.{js,mjs,cjs,jsx,ts,tsx,mts,cts}", "src/{index,cli,main}.{js,mjs,cjs,jsx,ts,tsx,mts,cts}"].iter().map(|s| s.to_string()).collect();
    let default_project: Vec<String> = vec!["**/*.{js,mjs,cjs,jsx,ts,tsx,mts,cts}".into()];

    let mut setups = Vec::with_capacity(workspaces.len());
    for (wi, ws) in workspaces.iter().enumerate() {
        let wcfg = ws_config(cfg, ws);
        let root_cfg = &cfg.root;
        let entry: Vec<String> = wcfg.and_then(|c| c.entry.as_ref()).map(|s| s.to_vec()).unwrap_or_else(|| default_entry.clone());
        let mut tool_entry: Vec<String> = Vec::new();
        let mut project: Vec<String> = wcfg.and_then(|c| c.project.as_ref()).map(|s| s.to_vec()).unwrap_or_else(|| default_project.clone());
        // single-file components when the framework is present (knip "compilers")
        for (dep, pat) in [("svelte", "**/*.svelte"), ("vue", "**/*.vue"), ("astro", "**/*.astro"), ("@mdx-js/mdx", "**/*.mdx")] {
            let mut cur = Some(wi);
            let mut present = false;
            while let Some(i) = cur {
                if workspaces[i].manifest.has_dep(dep) {
                    present = true;
                    break;
                }
                cur = workspaces[i].parent;
            }
            if present && wcfg.and_then(|c| c.project.as_ref()).is_none() {
                project.push(pat.to_string());
            }
        }
        let mut ignore: Vec<String> = match wcfg {
            Some(c) if !std::ptr::eq(c, root_cfg) => c.ignore.as_ref().map(|s| s.to_vec()).unwrap_or_default(),
            _ => vec![],
        };
        let mut dev_entry: Vec<String> = Vec::new();
        let mut config: Vec<String> = Vec::new();
        let mut enabled_tools = Vec::new();
        let overrides: FxHashMap<&str, PluginOverride> = {
            let mut m = FxHashMap::default();
            for src in [Some(root_cfg), wcfg].into_iter().flatten() {
                for (k, v) in &src.plugins {
                    if let Some(o) = plugin_override(v) {
                        m.insert(k.as_str(), o);
                    }
                }
            }
            m
        };
        for (ti, tool) in TOOLS.iter().enumerate() {
            let ov = overrides.get(tool.name);
            if ov.map(|o| o.disabled).unwrap_or(false) {
                continue;
            }
            let enabled = tool_enabled(tool, wi, workspaces)
                || (tool.enablers.is_empty() && config_present[wi].contains(&ti))
                || ov.map(|o| !o.disabled).unwrap_or(false);
            if !enabled {
                continue;
            }
            if production && tool.dev {
                continue;
            }
            enabled_tools.push(ti);
            let cfg_pats: Vec<String> = ov.and_then(|o| o.config.clone()).unwrap_or_else(|| tool.config.iter().map(|s| s.to_string()).collect());
            let entry_pats: Vec<String> = ov.and_then(|o| o.entry.clone()).unwrap_or_else(|| tool.entry.iter().map(|s| s.to_string()).collect());
            config.extend(cfg_pats);
            if tool.dev {
                dev_entry.extend(entry_pats);
            } else {
                tool_entry.extend(entry_pats);
            }
            if let Some(p) = ov.and_then(|o| o.project.clone()) {
                project.extend(p);
            }
        }
        // Declaration files included by tsconfig are entry files (they are part of the TS program).
        let dts = tsconfig_dts_patterns(&ws.dir);
        // package.json refs and script-referenced files
        let mut exact_entries: FxHashSet<String> = FxHashSet::default();
        for r in &ws.manifest.entry_refs {
            if let Some(m) = map_entry_ref(&ws.dir, r) {
                exact_entries.insert(m);
            }
        }
        for s in scripts::script_file_refs(&ws.manifest, &ws.dir) {
            exact_entries.insert(s);
        }
        for name in ["tidy.ts", "tidy.js", "knip.ts", "knip.js", "knip.mjs", "knip.config.ts", "knip.config.js"] {
            if ws.dir.join(name).is_file() {
                exact_entries.insert(name.to_string());
            }
        }
        // Scripts run from GitHub workflows (`node scripts/x.mjs`) are entry files.
        if ws.rel == "." {
            for f in &all {
                let rel = rel_posix(root, f);
                if rel.starts_with(".github/workflows/") && (rel.ends_with(".yml") || rel.ends_with(".yaml")) {
                    if let Ok(text) = std::fs::read_to_string(f) {
                        for (_, script) in crate::deps::workflow_runs(&text) {
                            for file in scripts::shell_text(&script, &ws.manifest, &ws.dir).files {
                                exact_entries.insert(file);
                            }
                        }
                    }
                }
            }
        }
        ignore.retain(|p| !p.is_empty());
        let mut ignore_dependencies = crate::config::matchers(&root_cfg.ignore_dependencies);
        let mut ignore_binaries = crate::config::matchers(&root_cfg.ignore_binaries);
        let mut ignore_unresolved = crate::config::matchers(&root_cfg.ignore_unresolved);
        if let Some(c) = wcfg {
            if ws.rel != "." {
                ignore_dependencies.extend(crate::config::matchers(&c.ignore_dependencies));
                ignore_binaries.extend(crate::config::matchers(&c.ignore_binaries));
                ignore_unresolved.extend(crate::config::matchers(&c.ignore_unresolved));
            }
        }
        let include_entry_exports = wcfg.and_then(|c| c.include_entry_exports).or(root_cfg.include_entry_exports).unwrap_or(false);
        let ieuf = ExportsUsedInFile::from_value(wcfg.and_then(|c| c.ignore_exports_used_in_file.as_ref()).or(root_cfg.ignore_exports_used_in_file.as_ref()));
        setups.push(WorkspaceSetup {
            dts,
            project: PatternSet::new(&project)?,
            entry: PatternSet::new(&entry)?,
            tool_entry: PatternSet::new(&tool_entry)?,
            dev_entry: PatternSet::new(&dev_entry)?,
            ignore: PatternSet::new(&ignore)?,
            config: PatternSet::new(&config)?,
            exact_entries,
            tools_with_config: enabled_tools.iter().copied().filter(|t| config_present[wi].contains(t)).collect(),
            enabled_tools,
            ignore_dependencies,
            ignore_binaries,
            ignore_unresolved,
            include_entry_exports,
            ignore_exports_used_in_file: ieuf,
            is_ignored_workspace: ws.rel != "." && ignore_ws_set.is_match(&ws.rel),
        });
    }

    crate::perf("setups", t);
    let t = std::time::Instant::now();
    // Pass B: classify.
    let mut files = Vec::with_capacity(all.len());
    for (i, p) in all.into_iter().enumerate() {
        let ws = file_ws[i];
        let rel = all_rel[i].clone();
        if negated_ws.is_match(&rel) {
            continue;
        }
        let ws_rel = ws_rels[i].clone();
        let s = &setups[ws];
        let is_ignored = root_ignore.is_match(&rel) || s.ignore.is_match(&ws_rel) || s.is_ignored_workspace;
        let is_config = s.config.is_match(&ws_rel);
        let is_dts = ws_rel.ends_with(".d.ts") || ws_rel.ends_with(".d.mts") || ws_rel.ends_with(".d.cts");
        let dts_entry = is_dts && s.dts.as_ref().map(|(inc, exc)| inc.is_match(&ws_rel) && !exc.is_match(&ws_rel)).unwrap_or(false);
        let user_entry = s.entry.is_match(&ws_rel) || s.exact_entries.contains(&ws_rel) || dts_entry;
        let tool = s.tool_entry.is_match(&ws_rel);
        let dev = s.dev_entry.is_match(&ws_rel);
        let is_entry = user_entry || tool || dev || is_config;
        let is_project = s.project.is_match(&ws_rel);
        if !is_entry && !is_project && !is_config {
            continue;
        }
        files.push(ProjectFile {
            path: p,
            rel,
            ws,
            is_project,
            is_entry,
            is_config,
            is_ignored,
            dev_entry: !user_entry && !tool && dev,
            tool_entry: !user_entry && (tool || dev || is_config),
        });
    }
    crate::perf("classify", t);
    Ok(Project { root: root.to_path_buf(), files, setups, all_rel })
}
