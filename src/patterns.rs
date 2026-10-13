//! Glob helpers (micromatch-ish syntax → globset) and the built-in tool conventions table.

use anyhow::Result;
use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};

/// Convert micromatch/fast-glob syntax used by knip-style configs into globset syntax.
/// Supports `?(a|b)` → `{,a,b}`, `@(a|b)` → `{a,b}`, `+(a|b)`/`*(a|b)` → `{a,b}` (approximate).
pub fn normalize_glob(p: &str) -> String {
    let p = p.trim_start_matches("./");
    let bytes = p.as_bytes();
    let mut out = String::with_capacity(p.len() + 8);
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if (c == '?' || c == '@' || c == '+' || c == '*' || c == '!') && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            // find the matching close paren (extglobs can nest)
            let mut depth = 0;
            let mut close = None;
            for (j, &b) in bytes.iter().enumerate().skip(i + 1) {
                if b == b'(' {
                    depth += 1;
                } else if b == b')' {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(j);
                        break;
                    }
                }
            }
            if let Some(close) = close {
                let inner = normalize_glob(&p[i + 2..close]);
                // split top-level `|`
                let mut alts = Vec::new();
                let mut d = 0;
                let mut start = 0;
                for (k, ch) in inner.char_indices() {
                    match ch {
                        '{' | '(' => d += 1,
                        '}' | ')' => d -= 1,
                        '|' if d == 0 => {
                            alts.push(inner[start..k].to_string());
                            start = k + 1;
                        }
                        _ => {}
                    }
                }
                alts.push(inner[start..].to_string());
                match c {
                    '?' => {
                        out.push_str("{,");
                        out.push_str(&alts.join(","));
                        out.push('}');
                    }
                    '!' => out.push('*'),
                    _ => {
                        out.push('{');
                        out.push_str(&alts.join(","));
                        out.push('}');
                    }
                }
                i = close + 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

pub fn expand_braces(p: &str) -> Vec<String> {
    let bytes = p.as_bytes();
    let Some(open) = p.find('{') else { return vec![p.to_string()] };
    // find matching close
    let mut depth = 0;
    let mut close = None;
    for (i, &c) in bytes.iter().enumerate().skip(open) {
        if c == b'{' {
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
            if depth == 0 {
                close = Some(i);
                break;
            }
        }
    }
    let Some(close) = close else { return vec![p.to_string()] };
    let inner = &p[open + 1..close];
    // split on top-level commas
    let mut alts = Vec::new();
    let mut d = 0;
    let mut start = 0;
    for (i, c) in inner.char_indices() {
        match c {
            '{' => d += 1,
            '}' => d -= 1,
            ',' if d == 0 => {
                alts.push(&inner[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    alts.push(&inner[start..]);
    let prefix = &p[..open];
    let suffix = &p[close + 1..];
    let mut out = Vec::new();
    for a in alts {
        for rest in expand_braces(&format!("{a}{suffix}")) {
            out.push(format!("{prefix}{rest}"));
        }
    }
    out
}

pub fn build_glob(p: &str) -> Result<Glob> {
    Ok(GlobBuilder::new(&normalize_glob(p)).literal_separator(true).build()?)
}

pub fn build_set(patterns: &[String]) -> Result<GlobSet> {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<std::collections::HashMap<Vec<String>, GlobSet>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Some(set) = cache.lock().unwrap().get(patterns) {
        return Ok(set.clone());
    }
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        for e in expand_braces(&normalize_glob(p)) {
            b.add(GlobBuilder::new(&e).literal_separator(true).build()?);
        }
    }
    let set = b.build()?;
    cache.lock().unwrap().insert(patterns.to_vec(), set.clone());
    Ok(set)
}

/// A pattern list with positive and negated (`!`) entries.
#[derive(Debug, Default)]
pub struct PatternSet {
    pub include: GlobSet,
    pub exclude: GlobSet,
    pub is_empty: bool,
}

impl PatternSet {
    pub fn new(patterns: &[String]) -> Result<PatternSet> {
        let (neg, pos): (Vec<&String>, Vec<&String>) = patterns.iter().partition(|p| p.starts_with('!'));
        let pos: Vec<String> = pos.into_iter().cloned().collect();
        let neg: Vec<String> = neg.into_iter().map(|p| p[1..].to_string()).collect();
        Ok(PatternSet { is_empty: pos.is_empty(), include: build_set(&pos)?, exclude: build_set(&neg)? })
    }
    pub fn is_match(&self, rel: &str) -> bool {
        !self.is_empty && self.include.is_match(rel) && !self.exclude.is_match(rel)
    }
}

pub const SOURCE_EXTS: &[&str] = &["js", "mjs", "cjs", "jsx", "ts", "tsx", "mts", "cts"];

pub fn is_source_ext(path: &str) -> bool {
    let Some(ext) = path.rsplit('.').next() else { return false };
    SOURCE_EXTS.contains(&ext) && path.contains('.')
}

/// A tool integration: when enabled, its config files and entry files are entry points.
pub struct Tool {
    pub name: &'static str,
    /// Dependency names enabling this tool: exact, `@scope/` (scope prefix) or `name-` (name prefix).
    pub enablers: &'static [&'static str],
    pub config: &'static [&'static str],
    /// Entry files only relevant outside production mode (tests, stories, mocks, ...).
    pub entry: &'static [&'static str],
    /// Entry files that are part of the production build.
    pub production: &'static [&'static str],
}

pub use crate::tools::TOOLS;
