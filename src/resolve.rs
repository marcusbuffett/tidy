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

pub struct Resolvers {
    root: PathBuf,
    base_options: ResolveOptions,
    base: Resolver,
    by_tsconfig: Mutex<FxHashMap<Option<PathBuf>, Arc<Resolver>>>,
    tsconfig_by_dir: Mutex<FxHashMap<PathBuf, Option<PathBuf>>>,
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
        Resolvers { root: root.to_path_buf(), base_options, base, by_tsconfig: Mutex::default(), tsconfig_by_dir: Mutex::default() }
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
