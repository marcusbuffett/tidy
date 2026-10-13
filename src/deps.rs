//! Dependency, unlisted and binary analysis per workspace.

use crate::graph::Graph;
use crate::issues::{Issue, IssueKind};
use crate::project::{Project, WorkspaceSetup};
use crate::resolve::Target;
use crate::scripts::{self, SYSTEM_BINARIES};
use crate::workspace::{DepKind, Workspace};
use rustc_hash::{FxHashMap, FxHashSet};
use std::path::Path;

const IGNORED_DEPENDENCIES: &[&str] = &["knip", "typescript", "tidy", "bun", "@types/bun", "bun-types"];

const PLUGIN_PREFIXES: &[&str] = &[
    "eslint-config-",
    "eslint-plugin-",
    "@babel/preset-",
    "@babel/plugin-",
    "babel-preset-",
    "babel-plugin-",
    "postcss-",
    "prettier-plugin-",
    "vite-plugin-",
    "rollup-plugin-",
    "stylelint-config-",
    "stylelint-plugin-",
    "remark-",
    "rehype-",
    "@eslint/",
    "@storybook/",
    "tailwindcss-",
    "expo-",
    "metro-",
    "@expo/",
    "@tailwindcss/",
    "@vitest/",
    "@commitlint/config-",
    "commitlint-config-",
    "semantic-release-",
    "@semantic-release/",
    "release-it-",
    "@release-it/",
    "lint-staged-",
    "gatsby-plugin-",
    "gatsby-theme-",
    "@nuxt/",
    "nuxt-",
    "@nuxtjs/",
    "@astrojs/",
    "@sveltejs/",
    "@docusaurus/",
    "docusaurus-",
    "@graphql-codegen/",
    "webpack-",
    "@rollup/plugin-",
    "@vitejs/plugin-",
    "vitest-",
    "jest-",
    "karma-",
    "mocha-",
    "@cspell/",
    "cspell-",
    "markdownlint-",
    "@react-native/",
    "react-native-",
    "@react-native-community/",
    "unplugin-",
    "@unocss/",
    "unocss-",
];

const KNOWN_BINARIES: &[(&str, &str)] = &[
    ("tsc", "typescript"),
    ("tsserver", "typescript"),
    ("playwright", "@playwright/test"),
    ("biome", "@biomejs/biome"),
    ("dotenv", "dotenv-cli"),
    ("sentry-cli", "@sentry/cli"),
    ("eas", "eas-cli"),
    ("changeset", "@changesets/cli"),
    ("commitlint", "@commitlint/cli"),
    ("run-s", "npm-run-all"),
    ("run-p", "npm-run-all"),
    ("graphql-codegen", "@graphql-codegen/cli"),
    ("gql-gen", "@graphql-codegen/cli"),
    ("svgr", "@svgr/cli"),
    ("postcss", "postcss-cli"),
    ("swc", "@swc/cli"),
    ("babel", "@babel/cli"),
    ("cap", "@capacitor/cli"),
    ("netlify", "netlify-cli"),
    ("firebase", "firebase-tools"),
    ("cdk", "aws-cdk"),
    ("ncc", "@vercel/ncc"),
    ("tauri", "@tauri-apps/cli"),
    ("ionic", "@ionic/cli"),
    ("ng", "@angular/cli"),
    ("nest", "@nestjs/cli"),
    ("remix", "@remix-run/dev"),
    ("react-router", "@react-router/dev"),
    ("svelte-kit", "@sveltejs/kit"),
    ("docusaurus", "@docusaurus/core"),
    ("eleventy", "@11ty/eleventy"),
    ("webpack", "webpack-cli"),
    ("storybook", "storybook"),
    ("sb", "storybook"),
    ("rnv", "rnv"),
    ("pod-install", "pod-install"),
    ("ts-node", "ts-node"),
    ("tsx", "tsx"),
    ("nuxi", "nuxi"),
    ("nuxt", "nuxt"),
    ("expo-cli", "expo-cli"),
    ("drizzle-kit", "drizzle-kit"),
    ("stryker", "@stryker-mutator/core"),
    ("c8", "c8"),
    ("nyc", "nyc"),
    ("tap", "tap"),
    ("size-limit", "size-limit"),
    ("madge", "madge"),
    ("typedoc", "typedoc"),
    ("vue-tsc", "vue-tsc"),
    ("svelte-check", "svelte-check"),
    ("rslib", "@rslib/core"),
    ("rsbuild", "@rsbuild/core"),
    ("rspack", "@rspack/cli"),
    ("oxlint", "oxlint"),
    ("oxfmt", "oxfmt"),
    ("relay-compiler", "relay-compiler"),
    ("wxt", "wxt"),
    ("prisma", "prisma"),
    ("vitest", "vitest"),
    ("jest", "jest"),
    ("eslint", "eslint"),
    ("prettier", "prettier"),
    ("next", "next"),
    ("expo", "expo"),
    ("turbo", "turbo"),
    ("vite", "vite"),
    ("tsup", "tsup"),
    ("tsdown", "tsdown"),
    ("rollup", "rollup"),
    ("cypress", "cypress"),
    ("knip", "knip"),
    ("husky", "husky"),
    ("lint-staged", "lint-staged"),
    ("cross-env", "cross-env"),
    ("rimraf", "rimraf"),
    ("shx", "shx"),
    ("nx", "nx"),
    ("lerna", "lerna"),
    ("concurrently", "concurrently"),
    ("wrangler", "wrangler"),
    ("nodemon", "nodemon"),
    ("esbuild", "esbuild"),
    ("sass", "sass"),
    ("tailwindcss", "tailwindcss"),
    ("mocha", "mocha"),
    ("ava", "ava"),
    ("wait-on", "wait-on"),
    ("serve", "serve"),
    ("http-server", "http-server"),
    ("msw", "msw"),
    ("patch-package", "patch-package"),
    ("release-it", "release-it"),
    ("semantic-release", "semantic-release"),
    ("vercel", "vercel"),
    ("supabase", "supabase"),
    ("convex", "convex"),
    ("sst", "sst"),
    ("serverless", "serverless"),
    ("zx", "zx"),
    ("electron", "electron"),
    ("electron-builder", "electron-builder"),
    ("electron-vite", "electron-vite"),
    ("astro", "astro"),
    ("gatsby", "gatsby"),
    ("hardhat", "hardhat"),
    ("react-native", "react-native"),
    ("npm-run-all", "npm-run-all"),
    ("npm-run-all2", "npm-run-all2"),
    ("tsc-alias", "tsc-alias"),
    ("tsc-watch", "tsc-watch"),
    ("vitepress", "vitepress"),
    ("unbuild", "unbuild"),
    ("changelogen", "changelogen"),
    ("bumpp", "bumpp"),
    ("syncpack", "syncpack"),
    ("depcheck", "depcheck"),
    ("cspell", "cspell"),
    ("markdownlint", "markdownlint-cli"),
    ("markdownlint-cli2", "markdownlint-cli2"),
    ("stylelint", "stylelint"),
    ("sort-package-json", "sort-package-json"),
    ("standard-version", "standard-version"),
    ("i18next", "i18next-parser"),
    ("plop", "plop"),
    ("json-server", "json-server"),
    ("gh-pages", "gh-pages"),
    ("open-cli", "open-cli"),
    ("wsrun", "wsrun"),
    ("ultra", "ultra-runner"),
    ("ts-patch", "ts-patch"),
    ("tsp", "ts-patch"),
    ("tsd", "tsd"),
    ("expect-type", "expect-type"),
    ("ncu", "npm-check-updates"),
    ("npm-check-updates", "npm-check-updates"),
    ("taze", "taze"),
    ("lefthook", "lefthook"),
    ("simple-git-hooks", "simple-git-hooks"),
    ("codegen", "@graphql-codegen/cli"),
    ("graphql-inspector", "@graphql-inspector/cli"),
    ("oclif", "oclif"),
    ("pkgroll", "pkgroll"),
    ("bunchee", "bunchee"),
    ("dts-bundle-generator", "dts-bundle-generator"),
    ("api-extractor", "@microsoft/api-extractor"),
    ("sort-imports", "sort-imports"),
    ("organize-imports-cli", "organize-imports-cli"),
    ("ts-unused-exports", "ts-unused-exports"),
    ("ts-prune", "ts-prune"),
];

/// Minimal view of an installed package's manifest.
#[derive(Default)]
struct Installed {
    peers: Vec<String>,
    has_types: bool,
    bins: Vec<String>,
}

struct InstalledCache {
    root: std::path::PathBuf,
    map: std::sync::Mutex<FxHashMap<std::path::PathBuf, std::sync::Arc<Installed>>>,
    /// (from_dir, name) → result, so repeated lookups never touch the file system.
    by_query: std::sync::Mutex<FxHashMap<(std::path::PathBuf, String), Option<std::sync::Arc<Installed>>>>,
}

impl InstalledCache {
    fn get(&self, from_dir: &Path, name: &str) -> Option<std::sync::Arc<Installed>> {
        let key = (from_dir.to_path_buf(), name.to_string());
        if let Some(v) = self.by_query.lock().unwrap().get(&key) {
            return v.clone();
        }
        let v = self.get_uncached(from_dir, name);
        self.by_query.lock().unwrap().insert(key, v.clone());
        v
    }

    fn get_uncached(&self, from_dir: &Path, name: &str) -> Option<std::sync::Arc<Installed>> {
        let mut cur = Some(from_dir);
        while let Some(d) = cur {
            let pj = d.join("node_modules").join(name).join("package.json");
            if pj.is_file() {
                // symlinked installs (pnpm, workspaces) share one real file
                let pj = std::fs::canonicalize(&pj).unwrap_or(pj);
                if let Some(v) = self.map.lock().unwrap().get(&pj) {
                    return Some(v.clone());
                }
                let text = std::fs::read_to_string(&pj).ok()?;
                let json: serde_json::Value = serde_json::from_str(&text).ok()?;
                let peers = json.get("peerDependencies").and_then(|p| p.as_object()).map(|o| o.keys().cloned().collect()).unwrap_or_default();
                let has_types = json.get("types").is_some() || json.get("typings").is_some();
                let bins = match json.get("bin") {
                    Some(serde_json::Value::String(_)) => vec![name.rsplit('/').next().unwrap_or(name).to_string()],
                    Some(serde_json::Value::Object(o)) => o.keys().cloned().collect(),
                    _ => vec![],
                };
                let v = std::sync::Arc::new(Installed { peers, has_types, bins });
                self.map.lock().unwrap().insert(pj, v.clone());
                return Some(v);
            }
            if d == self.root {
                break;
            }
            cur = d.parent();
        }
        None
    }
}

fn types_name(pkg: &str) -> String {
    if let Some(rest) = pkg.strip_prefix('@') {
        format!("@types/{}", rest.replace('/', "__"))
    } else {
        format!("@types/{pkg}")
    }
}

fn chain(ws: usize, workspaces: &[Workspace]) -> Vec<usize> {
    let mut out = vec![ws];
    let mut cur = workspaces[ws].parent;
    while let Some(p) = cur {
        out.push(p);
        cur = workspaces[p].parent;
    }
    out
}

/// Binary name → package name from `node_modules/.bin` symlinks in dir and ancestors.
fn bin_providers(dir: &Path, root: &Path) -> FxHashMap<String, String> {
    use std::sync::{Mutex, OnceLock};
    static DIR_CACHE: OnceLock<Mutex<FxHashMap<std::path::PathBuf, FxHashMap<String, String>>>> = OnceLock::new();
    let cache = DIR_CACHE.get_or_init(|| Mutex::new(FxHashMap::default()));
    let mut out = FxHashMap::default();
    let mut cur = Some(dir);
    while let Some(d) = cur {
        let bin = d.join("node_modules/.bin");
        if let Some(m) = cache.lock().unwrap().get(&bin) {
            for (k, v) in m {
                out.entry(k.clone()).or_insert_with(|| v.clone());
            }
            if d == root {
                break;
            }
            cur = d.parent();
            continue;
        }
        let mut local: FxHashMap<String, String> = FxHashMap::default();
        if let Ok(rd) = std::fs::read_dir(&bin) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if local.contains_key(&name) {
                    continue;
                }
                if let Ok(target) = std::fs::read_link(e.path()) {
                    let t = target.to_string_lossy().replace('\\', "/");
                    let t = t.trim_start_matches("../");
                    let mut parts = t.split('/');
                    let pkg = match parts.next() {
                        Some(s) if s.starts_with('@') => match parts.next() {
                            Some(n) => format!("{s}/{n}"),
                            None => continue,
                        },
                        Some(s) => s.to_string(),
                        None => continue,
                    };
                    local.insert(name, pkg);
                }
            }
        }
        for (k, v) in &local {
            out.entry(k.clone()).or_insert_with(|| v.clone());
        }
        cache.lock().unwrap().insert(bin, local);
        if d == root {
            break;
        }
        cur = d.parent();
    }
    out
}

fn line_col(text: &str, pos: usize) -> (u32, u32) {
    let pos = pos.min(text.len());
    let line = text[..pos].matches('\n').count() as u32 + 1;
    let col = pos - text[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    (line, col as u32 + 1)
}

pub fn analyze(project: &Project, graph: &Graph, workspaces: &[Workspace], setups: &[WorkspaceSetup]) -> Vec<Issue> {
    let root = &project.root;
    let n = workspaces.len();
    let mut referenced: Vec<FxHashSet<String>> = vec![FxHashSet::default(); n];
    let mut unlisted: Vec<FxHashMap<String, (String, u32, u32, u32)>> = vec![FxHashMap::default(); n];
    let mut unlisted_binaries: Vec<Vec<String>> = vec![vec![]; n];
    let chains: Vec<Vec<usize>> = (0..n).map(|i| chain(i, workspaces)).collect();
    let ws_names: FxHashSet<&str> = workspaces.iter().filter_map(|w| w.pkg_name()).collect();

    let mut modules_by_ws: Vec<Vec<usize>> = vec![vec![]; n];
    for (i, m) in graph.modules.iter().enumerate() {
        modules_by_ws[m.ws].push(i);
    }
    let lists = |ws: usize, pkg: &str| workspaces[ws].manifest.has_dep(pkg);
    let closest = |ws: usize, pkg: &str| chains[ws].iter().copied().find(|&c| lists(c, pkg));
    let installed = InstalledCache { root: root.to_path_buf(), map: Default::default(), by_query: Default::default() };

    let t = std::time::Instant::now();
    // 1. Imports.
    for m in &graph.modules {
        let ws = m.ws;
        let setup = &setups[ws];
        for (ii, r) in &m.edges {
            let imp = &m.facts.imports[*ii];
            let spec = imp.specifier.as_str();
            if spec.starts_with("bun:") || spec == "bun" {
                for p in ["bun-types", "@types/bun"] {
                    if let Some(c) = closest(ws, p) {
                        referenced[c].insert(p.into());
                    }
                }
                continue;
            }
            if let Target::Builtin = r.target {
                if let Some(c) = closest(ws, "@types/node") {
                    referenced[c].insert("@types/node".into());
                }
                continue;
            }
            let Some(pkg) = &r.package else { continue };
            let listed_at = closest(ws, pkg);
            if let Target::Internal(_) = r.target {
                if listed_at.is_none() {
                    continue; // tsconfig path alias into the repo
                }
            }
            if matches!(imp.kind, crate::extract::ImportKind::ConfigRef | crate::extract::ImportKind::Url) {
                continue;
            }
            let tpkg = types_name(pkg);
            if let Some(c) = closest(ws, &tpkg) {
                let own_types = installed.get(&workspaces[ws].dir, pkg).map(|i| i.has_types).unwrap_or(false);
                if !own_types {
                    referenced[c].insert(tpkg.clone());
                }
            }
            match listed_at {
                Some(c) => {
                    referenced[c].insert(pkg.clone());
                }
                None => {
                    if m.is_ignored || !m.reachable {
                        continue;
                    }
                    if workspaces[ws].pkg_name() == Some(pkg.as_str()) || ws_names.contains(pkg.as_str()) {
                        continue;
                    }
                    if IGNORED_DEPENDENCIES.contains(&pkg.as_str()) || setup.ignore_dependencies.iter().any(|x| x.matches(pkg)) {
                        continue;
                    }
                    if matches!(r.target, Target::Ignored) {
                        continue;
                    }
                    // type-only import satisfied by a listed @types package
                    if imp.is_type_only() && closest(ws, &tpkg).is_some() {
                        continue;
                    }
                    unlisted[ws].entry(pkg.clone()).or_insert((m.rel.clone(), imp.line, imp.col, imp.start));
                }
            }
        }
    }

    crate::perf("deps:imports", t);
    let t = std::time::Instant::now();
    // 2. Config-file strings and package.json strings → listed deps.
    for ws in 0..n {
        let mut strings: FxHashSet<&str> = FxHashSet::default();
        for &mi in &modules_by_ws[ws] {
            let m = &graph.modules[mi];
            if m.is_config {
                strings.extend(m.facts.strings.iter().map(|s| s.as_str()));
            }
        }
        strings.extend(workspaces[ws].manifest.strings.iter().map(|s| s.as_str()));
        if strings.is_empty() {
            continue;
        }
        // index: every string plus its package-like prefix (`a/b/c` → `a`, `@s/n/x` → `@s/n`)
        let mut index: FxHashSet<String> = FxHashSet::default();
        for s in &strings {
            index.insert((*s).to_string());
            if let Some(p) = crate::resolve::package_name(s) {
                index.insert(p);
            }
        }
        let refers = |dep: &str| -> bool {
            if index.contains(dep) {
                return true;
            }
            if dep.starts_with("@types/") {
                return false;
            }
            for p in PLUGIN_PREFIXES {
                if let Some(rest) = dep.strip_prefix(p) {
                    if index.contains(rest) {
                        return true;
                    }
                }
            }
            if let Some(scope) = dep.strip_prefix('@') {
                if let Some((sc, name)) = scope.split_once('/') {
                    for base in ["eslint-config", "prettier-config", "stylelint-config", "commitlint-config", "tsconfig", "babel-preset", "eslint-plugin"] {
                        if name == base && index.contains(&format!("@{sc}")) {
                            return true;
                        }
                        if let Some(rest) = name.strip_prefix(&format!("{base}-")) {
                            if index.contains(&format!("@{sc}/{rest}")) {
                                return true;
                            }
                        }
                    }
                }
            }
            false
        };
        for &c in &chains[ws] {
            let deps: Vec<String> = workspaces[c].manifest.dep_names().map(str::to_string).collect();
            for dep in deps {
                if referenced[c].contains(&dep) {
                    continue;
                }
                let base = dep.strip_prefix("@types/").map(|b| b.replace("__", "/"));
                let hit = refers(&dep) || base.as_deref().map(|b| refers(b) || (!b.contains('/') && index.contains(&format!("@{b}")))).unwrap_or(false);
                if hit {
                    referenced[c].insert(dep);
                }
            }
        }
    }

    crate::perf("deps:strings", t);
    let t = std::time::Instant::now();
    // 3. Tool enablers whose config files are present, binaries from scripts, husky hooks and workflows.
    let mut foreign_binaries: Vec<(usize, Vec<String>, Vec<String>)> = Vec::new();
    let mut pending_binaries: Vec<(usize, String, bool)> = Vec::new();
    for ws in 0..n {
        let w = &workspaces[ws];
        let setup = &setups[ws];
        for &ti in &setup.tools_with_config {
            for e in crate::patterns::TOOLS[ti].enablers {
                if e.ends_with('/') {
                    continue;
                }
                if let Some(c) = closest(ws, e) {
                    referenced[c].insert(e.to_string());
                }
            }
        }
        let script_use = scripts::analyze(&w.manifest, &w.dir);
        let mut binaries = script_use.binaries;
        let mut packages = script_use.packages;
        for &mi in &modules_by_ws[ws] {
            let m = &graph.modules[mi];
            if !m.is_config {
                continue;
            }
            let rel = ws_rel(root, &w.dir, &m.path);
            let is_husky = rel.starts_with(".husky/");
            let is_workflow = rel.starts_with(".github/workflows/") || rel.ends_with("/action.yml") || rel.ends_with("/action.yaml");
            if is_husky {
                if let Ok(text) = std::fs::read_to_string(&m.path) {
                    let u = scripts::shell_text(&text, &w.manifest, &w.dir);
                    binaries.extend(u.binaries);
                    packages.extend(u.packages);
                }
            } else if is_workflow {
                if let Ok(text) = std::fs::read_to_string(&m.path) {
                    for (dir, script) in workflow_runs(&text) {
                        let target_ws = dir.as_ref().map(|d| workspace_for_dir(&w.dir.join(d), workspaces)).unwrap_or(ws);
                        let tw = &workspaces[target_ws];
                        let u = scripts::shell_text(&script, &tw.manifest, &tw.dir);
                        if std::env::var_os("TIDY_DEBUG_BINARIES").is_some() {
                            eprintln!("binaries {} (dir {:?}): {:?}\n--- script ---\n{}\n---", rel, dir, u.binaries, script);
                        }
                        // knip parity: workflow scripts only mark known binaries as used, never report unknown ones.
                        foreign_binaries.push((target_ws, u.binaries, u.packages));
                    }
                }
            }
            let name = m.path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with("tsconfig") && name.ends_with(".json") {
                if let Ok(text) = std::fs::read_to_string(&m.path) {
                    if let Ok(v) = json5::from_str::<serde_json::Value>(&text) {
                        if v.pointer("/compilerOptions/importHelpers").and_then(|x| x.as_bool()).unwrap_or(false) {
                            packages.push("tslib".into());
                        }
                    }
                }
            }
        }
        // Tool-implied dependencies (platform packages, test runners pulled in by addons...).
        let strings: FxHashSet<String> =
            modules_by_ws[ws].iter().map(|&mi| &graph.modules[mi]).filter(|m| m.is_config).flat_map(|m| m.facts.strings.iter().cloned()).collect();
        for &ti in &setup.enabled_tools {
            match crate::patterns::TOOLS[ti].name {
                "capacitor" => {
                    for (dir, pkg) in [("android", "@capacitor/android"), ("ios", "@capacitor/ios")] {
                        if w.dir.join(dir).is_dir() {
                            packages.push(pkg.into());
                        }
                    }
                }
                "size-limit" => {
                    for &c in &chains[ws] {
                        packages.extend(workspaces[c].manifest.dep_names().filter(|d| d.starts_with("@size-limit/")).map(String::from));
                    }
                }
                "storybook" if strings.iter().any(|s| s == "@storybook/addon-vitest" || s == "@storybook/experimental-addon-test") => {
                    packages.extend(["vitest", "@vitest/browser", "@vitest/coverage-v8", "playwright"].map(String::from));
                }
                _ => {}
            }
        }
        for p in packages {
            if let Some(c) = closest(ws, &p) {
                referenced[c].insert(p);
            }
        }
        for b in binaries {
            pending_binaries.push((ws, b, true));
        }
    }
    for (ws, bins, pkgs) in foreign_binaries {
        for p in pkgs {
            if let Some(c) = closest(ws, &p) {
                referenced[c].insert(p);
            }
        }
        for b in bins {
            pending_binaries.push((ws, b, false));
        }
    }
    let mut providers_cache: FxHashMap<usize, FxHashMap<String, String>> = FxHashMap::default();
    for (ws, b, report) in pending_binaries {
        let w = &workspaces[ws];
        let setup = &setups[ws];
        let providers = providers_cache.entry(ws).or_insert_with(|| {
            let mut m = bin_providers(&w.dir, root);
            // binaries declared by installed listed packages (works with pnpm shims too)
            for &c in &chains[ws] {
                for dep in workspaces[c].manifest.dep_names() {
                    if let Some(inst) = installed.get(&workspaces[c].dir, dep) {
                        for b in &inst.bins {
                            m.entry(b.clone()).or_insert_with(|| dep.to_string());
                        }
                    }
                }
            }
            m
        });
        {
            if SYSTEM_BINARIES.contains(&b.as_str()) {
                continue;
            }
            let ignored = setup.ignore_binaries.iter().any(|x| x.matches(&b));
            let mut candidates: Vec<String> = Vec::new();
            if let Some(p) = providers.get(&b) {
                candidates.push(p.clone());
            }
            candidates.push(b.clone());
            for (bin, pkg) in KNOWN_BINARIES {
                if *bin == b {
                    candidates.push(pkg.to_string());
                }
            }
            let mut found = false;
            for cand in &candidates {
                if let Some(c) = closest(ws, cand) {
                    referenced[c].insert(cand.clone());
                    found = true;
                    break;
                }
            }
            if !found && report && !ignored && !unlisted_binaries[ws].contains(&b) {
                unlisted_binaries[ws].push(b);
            }
        }
    }

    crate::perf("deps:scripts+bins", t);
    let t = std::time::Instant::now();
    if let Some(name) = std::env::var_os("TIDY_DEBUG_DEPS") {
        let name = name.to_string_lossy().to_string();
        for ws in 0..n {
            if referenced[ws].contains(&name) {
                eprintln!("dep {name}: referenced in workspace {}", workspaces[ws].rel);
            }
        }
    }
    // 4. Report.
    let mut issues = Vec::new();
    for ws in 0..n {
        if setups[ws].is_ignored_workspace {
            continue;
        }
        let w = &workspaces[ws];
        let file = ws_rel(root, root, &w.manifest.path);
        let setup = &setups[ws];
        // host dependencies: dep → listed packages declaring it as a peer dependency
        let mut hosts: FxHashMap<String, Vec<String>> = FxHashMap::default();
        for &c in &chains[ws] {
            for h in workspaces[c].manifest.dep_names() {
                if let Some(inst) = installed.get(&workspaces[c].dir, h) {
                    for peer in &inst.peers {
                        hosts.entry(peer.clone()).or_default().push(h.to_string());
                    }
                }
            }
        }
        fn is_referenced(
            name: &str,
            ws: usize,
            chains: &[Vec<usize>],
            referenced: &[FxHashSet<String>],
            hosts: &FxHashMap<String, Vec<String>>,
            seen: &mut FxHashSet<String>,
        ) -> bool {
            if chains[ws].iter().any(|&c| referenced[c].contains(name)) {
                return true;
            }
            if !seen.insert(name.to_string()) {
                return false;
            }
            if let Some(hs) = hosts.get(name) {
                for h in hs {
                    if is_referenced(h, ws, chains, referenced, hosts, seen) {
                        return true;
                    }
                }
            }
            false
        }
        for (name, kind, pos) in &w.manifest.deps {
            let issue_kind = match kind {
                DepKind::Prod => IssueKind::Dependencies,
                DepKind::Dev => IssueKind::DevDependencies,
                _ => continue,
            };
            if referenced[ws].contains(name) || IGNORED_DEPENDENCIES.contains(&name.as_str()) || setup.ignore_dependencies.iter().any(|x| x.matches(name)) {
                continue;
            }
            let mut seen = FxHashSet::default();
            if hosts.contains_key(name) && is_referenced(name, ws, &chains, &referenced, &hosts, &mut seen) {
                if std::env::var_os("TIDY_DEBUG_DEPS").map(|v| v.to_string_lossy() == *name).unwrap_or(false) {
                    eprintln!("dep {name}: kept via host dependencies {:?}", hosts.get(name));
                }
                continue;
            }
            if let Some(base) = name.strip_prefix("@types/") {
                let base = base.replace("__", "/");
                let base = if base.contains('/') { format!("@{base}") } else { base };
                let mut seen = FxHashSet::default();
                if hosts.contains_key(&base) && is_referenced(&base, ws, &chains, &referenced, &hosts, &mut seen) {
                    continue;
                }
            }
            if ws_names.contains(name.as_str())
                && modules_by_ws[ws].iter().any(|&mi| graph.modules[mi].edges.iter().any(|(_, r)| r.package.as_deref() == Some(name)))
            {
                continue;
            }
            let (line, col) = line_col(&w.manifest.text, *pos);
            issues.push(Issue { kind: issue_kind, file: file.clone(), name: name.clone(), parent: None, line, col, pos: *pos as u32 });
        }
        let mut ul: Vec<(&String, &(String, u32, u32, u32))> = unlisted[ws].iter().collect();
        ul.sort();
        for (name, (f, line, col, pos)) in ul {
            issues.push(Issue { kind: IssueKind::Unlisted, file: f.clone(), name: name.clone(), parent: None, line: *line, col: *col, pos: *pos });
        }
        for b in &unlisted_binaries[ws] {
            issues.push(Issue { kind: IssueKind::Binaries, file: file.clone(), name: b.clone(), parent: None, line: 0, col: 0, pos: 0 });
        }
    }
    crate::perf("deps:report", t);
    issues
}

fn ws_rel(_root: &Path, base: &Path, p: &Path) -> String {
    p.strip_prefix(base).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

fn workspace_for_dir(dir: &Path, workspaces: &[Workspace]) -> usize {
    let mut norm = std::path::PathBuf::new();
    for c in dir.components() {
        match c {
            std::path::Component::ParentDir => {
                norm.pop();
            }
            std::path::Component::CurDir => {}
            other => norm.push(other),
        }
    }
    let mut best = 0;
    let mut best_len = 0;
    for (i, w) in workspaces.iter().enumerate() {
        if norm.starts_with(&w.dir) {
            let n = w.dir.components().count();
            if n > best_len {
                best_len = n;
                best = i;
            }
        }
    }
    best
}

/// Extract (working-directory, script) pairs from a GitHub workflow YAML.
pub fn workflow_runs(text: &str) -> Vec<(Option<String>, String)> {
    // Split into steps at `- ` list items that contain run:/uses:/name:, then read run and working-directory.
    let mut steps: Vec<Vec<&str>> = Vec::new();
    let mut step_indent: Option<usize> = None;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        let t = line.trim_start();
        let is_item = t.starts_with("- ");
        if is_item
            && (step_indent.is_none() || indent <= step_indent.unwrap())
            && (t.contains("run:")
                || t.contains("uses:")
                || t.contains("name:")
                || t.contains("id:")
                || t.contains("with:")
                || t.contains("working-directory:"))
        {
            step_indent = Some(indent);
            steps.push(vec![line]);
            continue;
        }
        if let Some(last) = steps.last_mut() {
            last.push(line);
        }
    }
    let mut out = Vec::new();
    for step in steps {
        let joined = step.join("\n");
        let run = workflow_run_lines(&joined);
        if run.trim().is_empty() {
            continue;
        }
        let dir = step.iter().find_map(|l| {
            let t = l.trim().trim_start_matches("- ").trim();
            t.strip_prefix("working-directory:").map(|v| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
        });
        out.push((dir, run));
    }
    out
}

/// Extract `run:` script bodies from a GitHub workflow YAML.
fn workflow_run_lines(text: &str) -> String {
    let mut out = String::new();
    let mut block_indent: Option<usize> = None;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        if let Some(bi) = block_indent {
            if line.trim().is_empty() || indent > bi {
                out.push_str(line.trim());
                out.push('\n');
                continue;
            }
            block_indent = None;
        }
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("run:").or_else(|| t.strip_prefix("- run:")) {
            let rest = rest.trim();
            if rest == "|" || rest == ">" || rest == "|-" || rest == ">-" {
                block_indent = Some(indent);
            } else {
                out.push_str(rest.trim_matches(|c| c == '"' || c == '\''));
                out.push('\n');
            }
        }
    }
    out
}
