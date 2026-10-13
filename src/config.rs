//! Configuration: `tidy.json` / `knip.json` (knip-compatible subset).

use anyhow::{Context, Result};
use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RawWorkspaceConfig {
    #[serde(default)]
    pub entry: Option<StringOrVec>,
    #[serde(default)]
    pub project: Option<StringOrVec>,
    #[serde(default)]
    pub ignore: Option<StringOrVec>,
    #[serde(default)]
    pub ignore_dependencies: Vec<String>,
    #[serde(default)]
    pub ignore_binaries: Vec<String>,
    #[serde(default)]
    pub ignore_unresolved: Vec<String>,
    #[serde(default)]
    pub include_entry_exports: Option<bool>,
    #[serde(default)]
    pub ignore_exports_used_in_file: Option<Value>,
    /// Plugin-specific keys (`vitest: false`, `playwright: { config: [...] }`).
    #[serde(flatten)]
    pub plugins: FxHashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum StringOrVec {
    One(String),
    Many(Vec<String>),
}

impl StringOrVec {
    pub fn to_vec(&self) -> Vec<String> {
        match self {
            StringOrVec::One(s) => vec![s.clone()],
            StringOrVec::Many(v) => v.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RawConfig {
    #[serde(flatten)]
    pub root: RawWorkspaceConfig,
    #[serde(default)]
    pub workspaces: FxHashMap<String, RawWorkspaceConfig>,
    #[serde(default)]
    pub ignore_workspaces: Vec<String>,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub rules: FxHashMap<String, String>,
}

impl RawConfig {
    /// Issue types turned off via `rules: { type: "off" }`.
    pub fn rules_off(&self) -> Vec<String> {
        self.rules.iter().filter(|(_, v)| v.as_str() == "off").map(|(k, _)| k.clone()).collect()
    }
}

/// Resolved plugin override for one workspace.
#[derive(Debug, Clone, Default)]
pub struct PluginOverride {
    pub disabled: bool,
    pub config: Option<Vec<String>>,
    pub entry: Option<Vec<String>>,
    pub project: Option<Vec<String>>,
}

const CONFIG_FILES: &[&str] = &[
    "tidy.json",
    "tidy.jsonc",
    "tidy.ts",
    "tidy.js",
    "knip.json",
    "knip.jsonc",
    ".knip.json",
    ".knip.jsonc",
    "knip.ts",
    "knip.js",
    "knip.mjs",
    "knip.config.ts",
    "knip.config.js",
];

pub fn load(root: &Path, explicit: Option<&Path>) -> Result<(RawConfig, Option<PathBuf>)> {
    let parse_any = |p: &Path, text: &str| -> Result<RawConfig> {
        let is_code = p.extension().map(|e| matches!(e.to_str(), Some("ts" | "js" | "mjs" | "mts" | "cjs" | "cts"))).unwrap_or(false);
        if is_code {
            parse_ts_config(p, text)
        } else {
            parse_config(text)
        }
    };
    if let Some(p) = explicit {
        let text = std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
        return Ok((parse_any(p, &text).with_context(|| format!("parsing {}", p.display()))?, Some(p.to_path_buf())));
    }
    for name in CONFIG_FILES {
        let p = root.join(name);
        if p.is_file() {
            let text = std::fs::read_to_string(&p)?;
            return Ok((parse_any(&p, &text).with_context(|| format!("parsing {}", p.display()))?, Some(p)));
        }
    }
    // package.json "knip" / "tidy" key
    let pj = root.join("package.json");
    if let Ok(text) = std::fs::read_to_string(&pj) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            for key in ["tidy", "knip"] {
                if let Some(cfg) = v.get(key) {
                    let cfg: RawConfig = serde_json::from_value(cfg.clone())?;
                    return Ok((cfg, Some(pj)));
                }
            }
        }
    }
    Ok((RawConfig::default(), None))
}

fn parse_config(text: &str) -> Result<RawConfig> {
    // json5 is a superset of JSONC: comments and trailing commas are fine.
    let v: Value = json5::from_str(text).context("config is not valid JSON/JSONC")?;
    Ok(serde_json::from_value(v)?)
}

/// Statically evaluate a `knip.ts`/`tidy.ts` config: the default export (or the object passed to
/// `defineConfig`, or the initializer of the exported variable) must be a literal object.
fn parse_ts_config(path: &Path, text: &str) -> Result<RawConfig> {
    let v = crate::jsvalue::default_export_literal(path, text).context("config must export a literal object (no runtime evaluation)")?;
    Ok(serde_json::from_value(v)?)
}

/// Plugin override from a raw config value (`false`, `true`, or an object).
pub fn plugin_override(v: &Value) -> Option<PluginOverride> {
    match v {
        Value::Bool(false) => Some(PluginOverride { disabled: true, ..Default::default() }),
        Value::Bool(true) => Some(PluginOverride::default()),
        Value::Object(o) => {
            let get = |k: &str| -> Option<Vec<String>> {
                o.get(k).map(|x| match x {
                    Value::String(s) => vec![s.clone()],
                    Value::Array(a) => a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect(),
                    _ => vec![],
                })
            };
            Some(PluginOverride { disabled: false, config: get("config"), entry: get("entry"), project: get("project") })
        }
        _ => None,
    }
}

/// Matcher for `ignoreDependencies`/`ignoreBinaries` entries: exact name or `/regex/`.
#[derive(Debug, Clone)]
pub enum NameMatcher {
    Exact(String),
    Glob(globset::GlobMatcher),
}

impl NameMatcher {
    pub fn new(s: &str) -> NameMatcher {
        if s.contains('*') {
            if let Ok(g) = globset::Glob::new(s) {
                return NameMatcher::Glob(g.compile_matcher());
            }
        }
        if let Some(inner) = s.strip_prefix('/').and_then(|x| x.strip_suffix('/')) {
            // Approximate JS regex via glob when it is a simple prefix/suffix pattern; else exact.
            let simple = inner.trim_start_matches('^').trim_end_matches('$');
            if !simple.chars().any(|c| "[]()|+?\\{}".contains(c)) {
                let pat = format!(
                    "{}{}{}",
                    if inner.starts_with('^') { "" } else { "*" },
                    simple.replace(".*", "*"),
                    if inner.ends_with('$') { "" } else { "*" }
                );
                if let Ok(g) = globset::Glob::new(&pat) {
                    return NameMatcher::Glob(g.compile_matcher());
                }
            }
        }
        NameMatcher::Exact(s.to_string())
    }
    pub fn matches(&self, name: &str) -> bool {
        match self {
            NameMatcher::Exact(s) => s == name,
            NameMatcher::Glob(g) => g.is_match(name),
        }
    }
}

pub fn matchers(list: &[String]) -> Vec<NameMatcher> {
    list.iter().map(|s| NameMatcher::new(s)).collect()
}
