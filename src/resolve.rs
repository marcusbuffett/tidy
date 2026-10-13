//! Module resolution via oxc_resolver with per-directory tsconfig discovery.

use oxc_resolver::{EnforceExtension, ResolveError, ResolveOptions, Resolver, TsconfigDiscovery, TsconfigOptions, TsconfigReferences};
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub enum Target {
    /// A file inside the repository (not under node_modules).
    Internal(PathBuf),
    /// A file under node_modules (or outside the repo).
    External(PathBuf),
    Builtin,
    Unresolved(String),
    /// Specifiers we never try: `data:`, `virtual:`, `http(s):`, `\0...`
    Ignored,
}

#[derive(Debug, Clone)]
pub struct Resolved {
    /// Package name when the specifier is bare (`react`, `@scope/pkg/sub` → `@scope/pkg`).
    pub package: Option<String>,
    pub target: Target,
}

pub struct WorkspacePackage {
    pub dir: PathBuf,
    pub exports: Option<serde_json::Value>,
    pub entry_refs: Vec<String>,
}

pub struct Resolvers {
    root: PathBuf,
    base_options: ResolveOptions,
    base: Resolver,
    by_tsconfig: Mutex<FxHashMap<Option<PathBuf>, Arc<Resolver>>>,
    tsconfig_by_dir: Mutex<FxHashMap<PathBuf, Option<PathBuf>>>,
    /// Workspace packages by name, for source-first resolution of monorepo imports.
    pub workspace_packages: FxHashMap<String, WorkspacePackage>,
}

/// Collect `compilerOptions.customConditions` through the `extends` chain of a tsconfig.
fn custom_conditions(path: &Path, depth: usize) -> Vec<String> {
    if depth > 8 {
        return vec![];
    }
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    let Ok(v) = json5::from_str::<serde_json::Value>(&text) else { return vec![] };
    let mut out: Vec<String> = v
        .pointer("/compilerOptions/customConditions")
        .and_then(|c| c.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let dir = path.parent().unwrap_or(Path::new("."));
    let extends: Vec<String> = match v.get("extends") {
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        _ => vec![],
    };
    for e in extends {
        let candidates: Vec<PathBuf> = if e.starts_with('.') || e.starts_with('/') {
            vec![dir.join(&e), dir.join(format!("{e}.json"))]
        } else {
            let mut c = Vec::new();
            let mut cur = Some(dir);
            while let Some(d) = cur {
                let base = d.join("node_modules").join(&e);
                c.push(base.clone());
                c.push(PathBuf::from(format!("{}.json", base.display())));
                c.push(base.join("tsconfig.json"));
                cur = d.parent();
            }
            c
        };
        if let Some(found) = candidates.into_iter().find(|c| c.is_file()) {
            for cc in custom_conditions(&found, depth + 1) {
                if !out.contains(&cc) {
                    out.push(cc);
                }
            }
        }
    }
    out
}

pub fn package_name(specifier: &str) -> Option<String> {
    if specifier.is_empty() || specifier.starts_with('.') || specifier.starts_with('/') || specifier.starts_with('#') || specifier.starts_with('\\') {
        return None;
    }
    if specifier.contains(':') {
        // node:fs, bun:test, npm:foo, data:, virtual: ...
        return None;
    }
    let mut parts = specifier.split('/');
    let first = parts.next()?;
    if first.starts_with('@') {
        let second = parts.next()?;
        if second.is_empty() {
            return None;
        }
        Some(format!("{first}/{second}"))
    } else {
        Some(first.to_string())
    }
}

pub fn is_ignored_specifier(s: &str) -> bool {
    s.starts_with("data:")
        || s.starts_with("virtual:")
        || s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with('\0')
        || s.starts_with("\\0")
        || s.starts_with("npm:")
        || s.starts_with("jsr:")
        || s.starts_with("bun:")
        || s.starts_with("cloudflare:")
        || s.starts_with("workerd:")
        || s.starts_with("astro:")
        || s.starts_with("~icons/")
        || s.starts_with("unplugin-")
        || s.starts_with("$app/")
        || s.starts_with("$env/")
        || s.starts_with("$lib/")
        || s.starts_with("#app")
        || s.starts_with("#imports")
        || s.starts_with("nuxt:")
        || s.starts_with("svelte/")
        || s.starts_with("vite/")
}

impl Resolvers {
    pub fn new(root: &Path) -> Resolvers {
        let base_options = ResolveOptions {
            extensions: [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".json", ".node", ".d.ts"].iter().map(|s| s.to_string()).collect(),
            extension_alias: vec![
                (".js".into(), vec![".ts".into(), ".tsx".into(), ".js".into(), ".jsx".into()]),
                (".jsx".into(), vec![".tsx".into(), ".jsx".into()]),
                (".mjs".into(), vec![".mts".into(), ".mjs".into()]),
                (".cjs".into(), vec![".cts".into(), ".cjs".into()]),
            ],
            condition_names: ["types", "import", "require", "node", "bun", "browser", "default", "development"].iter().map(|s| s.to_string()).collect(),
            main_fields: ["types", "source", "module", "main"].iter().map(|s| s.to_string()).collect(),
            enforce_extension: EnforceExtension::Auto,
            builtin_modules: true,
            symlinks: true,
            ..ResolveOptions::default()
        };
        let base = Resolver::new(base_options.clone());
        Resolvers { root: root.to_path_buf(), base_options, base, by_tsconfig: Mutex::default(), tsconfig_by_dir: Mutex::default(), workspace_packages: FxHashMap::default() }
    }

    fn tsconfig_for_dir(&self, dir: &Path) -> Option<PathBuf> {
        if let Some(v) = self.tsconfig_by_dir.lock().unwrap().get(dir) {
            return v.clone();
        }
        let mut cur = Some(dir);
        let mut found = None;
        let mut visited: Vec<PathBuf> = Vec::new();
        while let Some(d) = cur {
            if let Some(v) = self.tsconfig_by_dir.lock().unwrap().get(d) {
                found = v.clone();
                break;
            }
            visited.push(d.to_path_buf());
            let candidate = d.join("tsconfig.json");
            if candidate.is_file() {
                found = Some(candidate);
                break;
            }
            if d == self.root {
                break;
            }
            cur = d.parent();
        }
        let mut cache = self.tsconfig_by_dir.lock().unwrap();
        for d in visited {
            cache.insert(d, found.clone());
        }
        found
    }

    fn resolver_for(&self, tsconfig: Option<PathBuf>) -> Arc<Resolver> {
        let mut map = self.by_tsconfig.lock().unwrap();
        if let Some(r) = map.get(&tsconfig) {
            return r.clone();
        }
        // Independent caches per tsconfig: sharing one cache across tsconfigs that `extends` a
        // common base poisons `paths` resolution (observed with oxc_resolver 11).
        let r = match &tsconfig {
            Some(p) => {
                let mut opts = self.base_options.clone();
                opts.tsconfig = Some(TsconfigDiscovery::Manual(TsconfigOptions { config_file: p.clone(), references: TsconfigReferences::Auto }));
                let custom = custom_conditions(p, 0);
                if !custom.is_empty() {
                    let mut conds = custom;
                    conds.extend(opts.condition_names.iter().cloned());
                    opts.condition_names = conds;
                }
                Arc::new(Resolver::new(opts))
            }
            None => Arc::new(self.base.clone_with_options(self.base_options.clone())),
        };
        map.insert(tsconfig, r.clone());
        r
    }

    pub fn resolve(&self, from_dir: &Path, specifier: &str) -> Resolved {
        let package = package_name(specifier);
        if is_ignored_specifier(specifier) {
            return Resolved { package: None, target: Target::Ignored };
        }
        let tsconfig = self.tsconfig_for_dir(from_dir);
        let resolver = self.resolver_for(tsconfig.clone());
        let mut result = resolver.resolve(from_dir, specifier);
        if matches!(result, Err(ResolveError::TsconfigNotFound(_) | ResolveError::TsconfigLoadFailed { .. } | ResolveError::TsconfigCircularExtend(_) | ResolveError::TsconfigSelfReference(_))) {
            // Broken tsconfig (e.g. `extends` a package that is not installed): resolve without it.
            result = self.resolver_for(None).resolve(from_dir, specifier);
        }
        if std::env::var_os("TIDY_DEBUG_RESOLVE").is_some() {
            if let Err(e) = &result {
                eprintln!("resolve {} from {} (tsconfig {:?}): {}", specifier, from_dir.display(), tsconfig, e);
            }
        }
        // Monorepo packages: when the published entry (dist/build) is missing or not source, prefer the source file.
        if let Some(pkg) = &package {
            if let Some(wp) = self.workspace_packages.get(pkg) {
                let subpath = specifier[pkg.len()..].to_string();
                let ok = matches!(&result, Ok(r) if r.path().starts_with(&wp.dir) && !r.path().components().any(|c| c.as_os_str() == "node_modules") && r.path().extension().map(|e| matches!(e.to_str(), Some("ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs"))).unwrap_or(false) && !is_build_output(r.path()));
                if !ok {
                    if let Some(src) = workspace_source(wp, &subpath) {
                        return Resolved { package, target: Target::Internal(src) };
                    }
                }
            }
        }
        let target = match result {
            Ok(res) => {
                let p = res.path().to_path_buf();
                if p.starts_with(&self.root) && !p.components().any(|c| c.as_os_str() == "node_modules") {
                    Target::Internal(p)
                } else {
                    Target::External(p)
                }
            }
            Err(ResolveError::Builtin { .. }) => Target::Builtin,
            Err(ResolveError::Ignored(_)) => Target::Ignored,
            Err(e) => Target::Unresolved(e.to_string()),
        };
        Resolved { package, target }
    }
}


fn is_build_output(p: &Path) -> bool {
    p.components().any(|c| matches!(c.as_os_str().to_str(), Some("dist" | "build" | "lib" | "out" | "esm" | "cjs" | ".output")))
}

/// Find the source file behind a workspace package subpath (`.` or `./sub`) using package.json
/// `exports` targets (preferring source files), then main/module/source/types fields, then src/index.
fn workspace_source(wp: &WorkspacePackage, subpath: &str) -> Option<PathBuf> {
    let key = if subpath.is_empty() { ".".to_string() } else { format!(".{subpath}") };
    let mut targets: Vec<String> = Vec::new();
    if let Some(serde_json::Value::Object(exports)) = &wp.exports {
        if let Some(v) = exports.get(&key) {
            collect_targets(v, &mut targets);
        } else if key == "." && !exports.keys().any(|k| k.starts_with('.')) {
            // exports is a conditions object for the root
            collect_targets(&serde_json::Value::Object(exports.clone()), &mut targets);
        } else {
            // wildcard subpaths like "./*": map
            for (k, v) in exports {
                if let Some(prefix) = k.strip_suffix('*') {
                    if let Some(rest) = key.strip_prefix(prefix) {
                        let mut t = Vec::new();
                        collect_targets(v, &mut t);
                        targets.extend(t.into_iter().map(|x| x.replace('*', rest)));
                    }
                }
            }
        }
    } else if let Some(serde_json::Value::String(s)) = &wp.exports {
        if key == "." {
            targets.push(s.clone());
        }
    }
    if key == "." {
        targets.extend(wp.entry_refs.iter().cloned());
    }
    let is_src = |t: &str| t.rsplit('.').next().map(|e| matches!(e, "ts" | "tsx" | "mts" | "cts")).unwrap_or(false) && !t.contains("/dist/") && !t.starts_with("./dist") && !t.starts_with("dist");
    // 1. existing source targets
    for t in targets.iter().filter(|t| is_src(t)) {
        let p = wp.dir.join(t.trim_start_matches("./"));
        if p.is_file() {
            return Some(p);
        }
    }
    // 2. dist → src mapping
    for t in &targets {
        if let Some(m) = map_dist_to_src(&wp.dir, t) {
            return Some(m);
        }
    }
    // 3. conventional
    if key == "." {
        for c in ["src/index.ts", "src/index.tsx", "src/index.mts", "index.ts", "src/index.js", "index.js"] {
            let p = wp.dir.join(c);
            if p.is_file() {
                return Some(p);
            }
        }
    } else {
        let sub = subpath.trim_start_matches('/');
        for c in [format!("src/{sub}.ts"), format!("src/{sub}.tsx"), format!("src/{sub}/index.ts"), format!("src/{sub}/index.tsx"), format!("{sub}.ts"), format!("{sub}/index.ts")] {
            let p = wp.dir.join(c);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn collect_targets(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_targets(x, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| collect_targets(x, out)),
        _ => {}
    }
}

pub fn map_dist_to_src(dir: &Path, r: &str) -> Option<PathBuf> {
    let r = r.trim_start_matches("./");
    let mut parts: Vec<&str> = r.split('/').collect();
    if parts.len() < 2 || !matches!(parts[0], "dist" | "build" | "lib" | "out" | "esm" | "cjs" | "output") {
        return None;
    }
    parts[0] = "src";
    let joined = parts.join("/");
    let stem = joined
        .strip_suffix(".d.ts")
        .or_else(|| joined.strip_suffix(".d.mts"))
        .or_else(|| joined.strip_suffix(".d.cts"))
        .or_else(|| joined.rsplit_once('.').map(|(s, _)| s))
        .unwrap_or(&joined);
    // build/modern/index.js → src/index.ts (drop one intermediate segment if needed)
    let mut stems = vec![stem.to_string()];
    let segs: Vec<&str> = stem.split('/').collect();
    if segs.len() > 2 {
        stems.push(format!("{}/{}", segs[0], segs[2..].join("/")));
    }
    for st in stems {
        for ext in ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"] {
            let cand = dir.join(format!("{st}.{ext}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}
