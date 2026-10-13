//! Per-file fact extraction: imports, exports, member usage, CommonJS shapes.
//! One parse + one semantic pass per file; everything else is derived from node/symbol tables.

use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast::AstKind;
use oxc_parser::{ParseOptions, Parser};
use oxc_semantic::{Semantic, SemanticBuilder, SymbolId};
use oxc_span::{GetSpan, SourceType, Span};
use rustc_hash::{FxHashMap, FxHashSet};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Members {
    All,
    Named(Vec<String>),
}

impl Import {
    /// All imported bindings are type-only (or the whole import is `import type`).
    pub fn is_type_only(&self) -> bool {
        if self.is_type {
            return true;
        }
        match &self.names {
            Names::Named(list) => !list.is_empty() && list.iter().all(|n| n.is_type),
            Names::All => false,
        }
    }
}

impl Members {
    pub fn merge(&mut self, other: &Members) -> bool {
        match (&mut *self, other) {
            (Members::All, _) => false,
            (_, Members::All) => {
                *self = Members::All;
                true
            }
            (Members::Named(a), Members::Named(b)) => {
                let mut changed = false;
                for m in b {
                    if !a.contains(m) {
                        a.push(m.clone());
                        changed = true;
                    }
                }
                changed
            }
        }
    }
    pub fn contains(&self, m: &str) -> bool {
        match self {
            Members::All => true,
            Members::Named(v) => v.iter().any(|x| x == m),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind {
    Static,
    ReExport,
    Dynamic,
    /// `lazy(() => import('./x'))`: a loader consumes only the default export when the module has one.
    Loader,
    Require,
    Mock,
    /// `import.meta.glob('./x/*.ts')` / `require.context(...)`: specifier is a glob.
    Glob,
    /// `new URL('./x', import.meta.url)`: may point at a directory; never reported as unresolved.
    Url,
    /// `/// <reference types|path="..." />`
    Reference,
    /// A relative path string inside a config file (setup files, reporters, ...).
    ConfigRef,
}

#[derive(Debug, Clone)]
pub struct ImportedName {
    /// Export name requested: a named export, `default`, or `*` for the namespace object.
    pub name: String,
    pub is_type: bool,
    /// Member accesses on the local binding (`E.Member`, `ns.export`).
    pub members: Members,
}

#[derive(Debug, Clone)]
pub enum Names {
    /// Whole module namespace reached in an opaque way (dynamic import passed around, `require()` result used as a value...).
    All,
    Named(Vec<ImportedName>),
}

#[derive(Debug, Clone)]
pub struct Import {
    pub specifier: String,
    pub kind: ImportKind,
    pub is_type: bool,
    pub names: Names,
    pub start: u32,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    Value,
    Type,
    Enum,
    Class,
    Namespace,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct Export {
    pub name: String,
    pub kind: ExportKind,
    pub members: Vec<String>,
    pub start: u32,
    pub line: u32,
    pub col: u32,
    pub used_in_file: bool,
    pub members_used_in_file: Members,
    /// `export { a as b } from 'x'` / `export * as ns from 'x'` / `import {a} from 'x'; export { a }`: (import index, imported name).
    pub reexport: Option<(usize, String)>,
    /// JSDoc tags on the export statement (`public`, `internal`, ...).
    pub tags: Vec<String>,
    /// Indices of other exports whose declarations reference this export's local binding.
    pub referenced_in: Vec<usize>,
    /// Declared through a destructuring pattern (`export const [a, setA] = ...`): in-file use counts.
    pub is_binding_element: bool,
    /// Local binding symbol (for referenced_in computation).
    symbol: Option<SymbolId>,
}

#[derive(Debug, Clone)]
pub struct ExportAll {
    pub import: usize,
}

#[derive(Debug, Default, Clone)]
pub struct FileFacts {
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
    pub export_alls: Vec<ExportAll>,
    pub is_cjs: bool,
    /// String literals (only collected for config files).
    pub strings: Vec<String>,
    /// String literals with the nearest enclosing object key (`entry: ['src/*.ts']` → ("entry", "src/*.ts")).
    pub keyed_strings: Vec<(String, String)>,
    pub parse_error: bool,
}

struct LineIndex(Vec<u32>);

impl LineIndex {
    fn new(src: &str) -> LineIndex {
        let mut v = vec![0u32];
        v.extend(memchr::memchr_iter(b'\n', src.as_bytes()).map(|i| i as u32 + 1));
        LineIndex(v)
    }
    fn line_col(&self, off: u32) -> (u32, u32) {
        let line = self.0.partition_point(|s| *s <= off);
        let start = self.0[line - 1];
        (line as u32, off - start + 1)
    }
}

pub fn source_type_for(path: &Path) -> Option<SourceType> {
    SourceType::from_path(path).ok()
}

/// Single-file components (`.svelte`, `.vue`, `.astro`): extract `<script>` blocks (and astro frontmatter)
/// so their imports/exports can be parsed as TypeScript. Returns the concatenated script source.
pub fn component_script(path: &Path, source: &str) -> Option<String> {
    let ext = path.extension()?.to_str()?;
    let mut out = String::new();
    match ext {
        "astro" => {
            // frontmatter between leading `---` fences
            let t = source.trim_start();
            if let Some(rest) = t.strip_prefix("---") {
                if let Some(end) = rest.find("\n---") {
                    out.push_str(&rest[..end]);
                }
            }
        }
        "svelte" | "vue" | "html" | "mdx" => {}
        _ => return None,
    }
    let bytes = source.as_bytes();
    let mut i = 0;
    while let Some(start) = source[i..].find("<script") {
        let open = i + start;
        let Some(tag_end) = source[open..].find('>') else { break };
        let tag = &source[open..open + tag_end];
        let body_start = open + tag_end + 1;
        let Some(close) = source[body_start..].find("</script") else { break };
        // In html only `<script type="module">` is handled by the bundler; other scripts are static assets.
        let is_module = tag.contains("type=\"module\"") || tag.contains("type='module'") || tag.contains("type=module");
        if ext == "html" && !is_module {
            i = body_start + close;
            continue;
        }
        if !tag.contains("src=") {
            out.push_str(&source[body_start..body_start + close]);
            out.push('\n');
        } else if let Some(src) = tag.split("src=").nth(1) {
            let q = src.chars().next().unwrap_or('"');
            let v: String = src.trim_start_matches(q).chars().take_while(|c| *c != q).collect();
            // `/src/main.ts` is relative to the vite root, which is where the html lives
            let v = if ext == "html" && v.starts_with('/') && !v.starts_with("//") { format!(".{v}") } else { v };
            if !v.contains("://") && !v.starts_with("//") {
                out.push_str(&format!("import \"{v}\";\n"));
            }
        }
        i = body_start + close;
        if i >= bytes.len() {
            break;
        }
    }
    if ext == "mdx" {
        // top-level `import`/`export` lines in MDX
        for line in source.lines() {
            let l = line.trim_start();
            if (l.starts_with("import ") || l.starts_with("export ")) && !out.contains(l) {
                out.push_str(l);
                out.push('\n');
            }
        }
    }
    Some(out)
}

pub fn is_component_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).map(|e| matches!(e, "svelte" | "vue" | "astro" | "mdx" | "html")).unwrap_or(false)
}

/// Cheap extraction for files outside the project (generated code, gitignored files reached via
/// imports): imports/exports from the module record only, no semantic analysis. Member usage is
/// unknown, so imported bindings count as fully used.
/// Import specifiers of very large generated files (multi-megabyte `.d.ts`), found by scanning for
/// `import`/`export ... from` statements and `import()`/`require()` calls instead of parsing.
pub fn extract_scan(source: &str) -> FileFacts {
    let mut facts = FileFacts::default();
    let mut seen: FxHashSet<String> = FxHashSet::default();
    let bytes = source.as_bytes();
    let mut push = |spec: &str, kind: ImportKind, start: usize, facts: &mut FileFacts| {
        if seen.insert(spec.to_string()) {
            facts.imports.push(Import { specifier: spec.to_string(), kind, is_type: false, names: Names::All, start: start as u32, line: 0, col: 0 });
        }
    };
    // quoted string right after position `i` (skipping spaces)
    let quoted = |mut i: usize| -> Option<(usize, usize)> {
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        let q = *bytes.get(i)?;
        if q != b'"' && q != b'\'' {
            return None;
        }
        let end = memchr::memchr(q, &bytes[i + 1..])? + i + 1;
        Some((i + 1, end))
    };
    // ` from "..."` after import/export statements
    let finder = memchr::memmem::Finder::new(b" from ");
    for pos in finder.find_iter(bytes) {
        // statement must start with import/export on this line
        let line_start = memchr::memrchr(b'\n', &bytes[..pos]).map(|i| i + 1).unwrap_or(0);
        let head = &source[line_start..pos];
        let head = head.trim_start();
        if !(head.starts_with("import") || head.starts_with("export")) {
            continue;
        }
        if let Some((a, b)) = quoted(pos + 6) {
            push(&source[a..b], ImportKind::Static, line_start, &mut facts);
        }
    }
    for needle in [&b"require("[..], &b"import("[..]] {
        let f = memchr::memmem::Finder::new(needle);
        for pos in f.find_iter(bytes) {
            if let Some((a, b)) = quoted(pos + needle.len()) {
                push(&source[a..b], ImportKind::Require, pos, &mut facts);
            }
        }
    }
    // side-effect imports: `import "x"`
    let f = memchr::memmem::Finder::new(b"import ");
    for pos in f.find_iter(bytes) {
        if let Some((a, b)) = quoted(pos + 7) {
            push(&source[a..b], ImportKind::Static, pos, &mut facts);
        }
    }
    facts
}

pub fn extract_light(path: &Path, source: &str) -> FileFacts {
    if is_component_file(path) {
        return extract(path, source, false);
    }
    // Generated giants (Prisma clients, GraphQL schemas...) only matter for the packages they import.
    if source.len() > 128 * 1024 {
        return extract_scan(source);
    }
    let Some(source_type) = source_type_for(path) else { return FileFacts::default() };
    let allocator = Allocator::default();
    let ret = Parser::new(&allocator, source, source_type).with_options(ParseOptions { preserve_parens: false, ..ParseOptions::default() }).parse();
    let lines = LineIndex::new(source);
    let mut facts = FileFacts { parse_error: ret.fatal_error, ..Default::default() };
    let mr = &ret.module_record;
    let mut by_request: FxHashMap<String, usize> = FxHashMap::default();
    for (request, _) in mr.requested_modules.iter() {
        let spec = request.to_string();
        let (line, col) = (0, 0);
        by_request.entry(spec.clone()).or_insert_with(|| {
            facts.imports.push(Import { specifier: spec, kind: ImportKind::Static, is_type: false, names: Names::Named(vec![]), start: 0, line, col });
            facts.imports.len() - 1
        });
    }
    for entry in &mr.import_entries {
        let spec = entry.module_request.name.to_string();
        let idx = by_request[&spec];
        let name = match &entry.import_name {
            oxc_syntax::module_record::ImportImportName::Name(n) => n.name.to_string(),
            oxc_syntax::module_record::ImportImportName::NamespaceObject => "*".to_string(),
            oxc_syntax::module_record::ImportImportName::Default(_) => "default".to_string(),
        };
        if let Names::Named(list) = &mut facts.imports[idx].names {
            list.push(ImportedName { name, is_type: entry.is_type, members: Members::All });
        }
    }
    fn export_name_of(n: &oxc_syntax::module_record::ExportExportName) -> String {
        match n {
            oxc_syntax::module_record::ExportExportName::Name(ns) => ns.name.to_string(),
            _ => "default".to_string(),
        }
    }
    for entry in mr.local_export_entries.iter() {
        let name = export_name_of(&entry.export_name);
        let start = entry.span.start;
        let (line, col) = lines.line_col(start);
        facts.exports.push(Export {
            name,
            kind: ExportKind::Unknown,
            members: vec![],
            start,
            line,
            col,
            used_in_file: false,
            members_used_in_file: Members::All,
            reexport: None,
            tags: vec![],
            referenced_in: vec![],
            is_binding_element: false,
            symbol: None,
        });
    }
    for entry in mr.indirect_export_entries.iter() {
        let Some(req) = &entry.module_request else { continue };
        let spec = req.name.to_string();
        let idx = by_request[&spec];
        let imported = match &entry.import_name {
            oxc_syntax::module_record::ExportImportName::Name(n) => n.name.to_string(),
            oxc_syntax::module_record::ExportImportName::All | oxc_syntax::module_record::ExportImportName::AllButDefault => "*".to_string(),
            oxc_syntax::module_record::ExportImportName::Null => continue,
        };
        if let Names::Named(list) = &mut facts.imports[idx].names {
            if imported != "*" {
                list.push(ImportedName { name: imported.clone(), is_type: entry.is_type, members: Members::All });
            }
        }
        let name = export_name_of(&entry.export_name);
        let (line, col) = lines.line_col(entry.span.start);
        facts.exports.push(Export {
            name,
            kind: ExportKind::Unknown,
            members: vec![],
            start: entry.span.start,
            line,
            col,
            used_in_file: false,
            members_used_in_file: Members::All,
            reexport: Some((idx, imported)),
            tags: vec![],
            referenced_in: vec![],
            is_binding_element: false,
            symbol: None,
        });
    }
    for entry in mr.star_export_entries.iter() {
        let Some(req) = &entry.module_request else { continue };
        let spec = req.name.to_string();
        let idx = by_request[&spec];
        facts.export_alls.push(ExportAll { import: idx });
    }
    facts
}

pub fn extract(path: &Path, source: &str, collect_strings: bool) -> FileFacts {
    if is_component_file(path) {
        let Some(script) = component_script(path, source) else { return FileFacts::default() };
        let mut facts = extract_source(&script, SourceType::tsx(), collect_strings);
        // component files are consumed by their framework: never report their exports
        for e in &mut facts.exports {
            e.tags.push("public".into());
        }
        return facts;
    }
    let Some(source_type) = source_type_for(path) else { return FileFacts::default() };
    extract_source(source, source_type, collect_strings)
}

fn extract_source(source: &str, source_type: SourceType, collect_strings: bool) -> FileFacts {
    let allocator = Allocator::default();
    let ret = Parser::new(&allocator, source, source_type).with_options(ParseOptions { preserve_parens: false, ..ParseOptions::default() }).parse();
    let program = ret.program;
    let semantic = SemanticBuilder::new().with_build_nodes(true).build(&program).semantic;
    let lines = LineIndex::new(source);
    let mut ex = Extractor {
        sem: &semantic,
        source,
        lines: &lines,
        comments: &program.comments,
        facts: FileFacts { parse_error: ret.fatal_error, ..Default::default() },
        import_symbols: FxHashMap::default(),
        collect_strings,
    };
    ex.run();
    ex.reference_directives();
    ex.compute_referenced_in();
    ex.facts
}

struct Extractor<'s, 'a> {
    sem: &'s Semantic<'a>,
    source: &'a str,
    lines: &'s LineIndex,
    comments: &'s [Comment],
    facts: FileFacts,
    /// local import binding symbol → (import index, imported name)
    import_symbols: FxHashMap<SymbolId, (usize, String)>,
    collect_strings: bool,
}

fn export_name(n: &ModuleExportName) -> String {
    match n {
        ModuleExportName::IdentifierName(i) => i.name.to_string(),
        ModuleExportName::IdentifierReference(i) => i.name.to_string(),
        ModuleExportName::StringLiteral(s) => s.value.to_string(),
    }
}

fn string_arg(e: &Expression) -> Option<String> {
    match e {
        Expression::StringLiteral(s) => Some(s.value.to_string()),
        Expression::TemplateLiteral(t) if t.expressions.is_empty() && t.quasis.len() == 1 => {
            Some(t.quasis[0].value.cooked.as_ref().map(|c| c.to_string()).unwrap_or_else(|| t.quasis[0].value.raw.to_string()))
        }
        _ => None,
    }
}

fn first_string_arg(args: &[Argument]) -> Option<String> {
    match args.first() {
        Some(Argument::StringLiteral(s)) => Some(s.value.to_string()),
        Some(Argument::TemplateLiteral(t)) if t.expressions.is_empty() && t.quasis.len() == 1 => Some(t.quasis[0].value.raw.to_string()),
        _ => None,
    }
}

fn is_global_ident(sem: &Semantic, e: &Expression, name: &str) -> bool {
    match e {
        Expression::Identifier(id) if id.name == name => match id.reference_id.get() {
            Some(rid) => sem.scoping().get_reference(rid).symbol_id().is_none(),
            None => true,
        },
        _ => false,
    }
}

fn is_import_meta_url(e: &Expression) -> bool {
    match e {
        Expression::StaticMemberExpression(m) => m.property.name == "url" && matches!(&m.object, Expression::ImportMeta(_)),
        _ => false,
    }
}

fn binding_names(p: &BindingPattern, out: &mut Vec<(String, u32, Option<SymbolId>)>) {
    match p {
        BindingPattern::BindingIdentifier(b) => out.push((b.name.to_string(), b.span.start, b.symbol_id.get())),
        BindingPattern::ObjectPattern(o) => {
            for prop in &o.properties {
                binding_names(&prop.value, out);
            }
            if let Some(r) = &o.rest {
                binding_names(&r.argument, out);
            }
        }
        BindingPattern::ArrayPattern(a) => {
            for el in a.elements.iter().flatten() {
                binding_names(el, out);
            }
            if let Some(r) = &a.rest {
                binding_names(&r.argument, out);
            }
        }
        BindingPattern::AssignmentPattern(a) => binding_names(&a.left, out),
    }
}

fn property_key_name(k: &PropertyKey) -> Option<String> {
    match k {
        PropertyKey::StaticIdentifier(i) => Some(i.name.to_string()),
        PropertyKey::StringLiteral(s) => Some(s.value.to_string()),
        PropertyKey::NumericLiteral(n) => Some(n.raw.as_ref().map(|r| r.to_string()).unwrap_or_else(|| n.value.to_string())),
        _ => None,
    }
}

impl<'s, 'a> Extractor<'s, 'a> {
    fn run(&mut self) {
        let sem = self.sem;
        let nodes = sem.nodes();
        // Pass 1: imports (so re-exports of import bindings can reference them).
        for node in nodes.iter() {
            match node.kind() {
                AstKind::ImportDeclaration(decl) => {
                    if !self.is_top_level(node.id()) {
                        continue;
                    }
                    self.import_declaration(decl);
                }
                AstKind::TSImportEqualsDeclaration(decl) => {
                    if let TSModuleReference::ExternalModuleReference(ext) = &decl.module_reference {
                        let (any, members) = self.member_usage(decl.id.symbol_id.get());
                        let _ = any;
                        let idx = self.push_import(
                            ext.expression.value.to_string(),
                            ImportKind::Require,
                            decl.import_kind.is_type(),
                            Names::Named(vec![ImportedName { name: "*".into(), is_type: decl.import_kind.is_type(), members }]),
                            decl.span.start,
                        );
                        if let Some(sym) = decl.id.symbol_id.get() {
                            self.import_symbols.insert(sym, (idx, "*".into()));
                        }
                    }
                }
                _ => {}
            }
        }
        // Pass 2: everything else.
        for node in nodes.iter() {
            match node.kind() {
                AstKind::ExportDeclaration(decl) => {
                    if self.is_top_level(node.id()) {
                        let tags = self.tags_before(decl.span.start);
                        self.declaration(&decl.declaration, &tags);
                    }
                }
                AstKind::ExportFromDeclaration(decl) => {
                    if self.is_top_level(node.id()) {
                        self.export_from(decl);
                    }
                }
                AstKind::ExportNamedDeclaration(decl) => {
                    if self.is_top_level(node.id()) {
                        self.export_named(decl);
                    }
                }
                AstKind::ExportDefaultDeclaration(decl) => {
                    if self.is_top_level(node.id()) {
                        self.export_default(decl);
                    }
                }
                AstKind::ExportAllDeclaration(decl) => {
                    if self.is_top_level(node.id()) {
                        self.export_all(decl);
                    }
                }
                AstKind::TSExportAssignment(decl) => {
                    if self.is_top_level(node.id()) {
                        let tags = self.tags_before(decl.span.start);
                        self.push_export("default".into(), ExportKind::Value, vec![], decl.span.start, false, Members::All, None, tags);
                    }
                }
                AstKind::ImportExpression(ie) => {
                    if let Some(spec) = string_arg(&ie.source) {
                        if self.is_inline_loader(node.id()) {
                            let names = Names::Named(vec![ImportedName { name: "default".into(), is_type: false, members: Members::All }]);
                            self.push_import(spec, ImportKind::Loader, false, names, ie.span.start);
                        } else {
                            let names = self.names_from_parent(node.id());
                            self.push_import(spec, ImportKind::Dynamic, false, names, ie.span.start);
                        }
                    }
                }
                AstKind::CallExpression(call) => self.call_expression(node.id(), call),
                AstKind::NewExpression(ne) => {
                    if is_global_ident(sem, &ne.callee, "URL") && ne.arguments.len() == 2 {
                        if let (Some(spec), Argument::StaticMemberExpression(_)) = (first_string_arg(&ne.arguments), &ne.arguments[1]) {
                            if let Some(e) = ne.arguments[1].as_expression() {
                                if is_import_meta_url(e) && (spec.starts_with('.') || spec.starts_with('/')) {
                                    self.push_import(spec, ImportKind::Url, false, Names::Named(vec![]), ne.span.start);
                                }
                            }
                        }
                    }
                }
                AstKind::AssignmentExpression(assign) => self.assignment(assign),
                AstKind::StringLiteral(s) if self.collect_strings => {
                    self.facts.strings.push(s.value.to_string());
                    if s.value.contains('*') || s.value.contains('/') || s.value.contains('.') {
                        // nearest enclosing object property key
                        let mut pid = nodes.parent_id(node.id());
                        let mut key: Option<String> = None;
                        for _ in 0..6 {
                            match nodes.kind(pid) {
                                AstKind::ObjectProperty(p) => {
                                    key = property_key_name(&p.key);
                                    break;
                                }
                                AstKind::ArrayExpression(_) | AstKind::ParenthesizedExpression(_) | AstKind::TSAsExpression(_) => pid = nodes.parent_id(pid),
                                _ => break,
                            }
                        }
                        if let Some(k) = key {
                            self.facts.keyed_strings.push((k, s.value.to_string()));
                        }
                    }
                }
                _ => {}
            }
        }
        // Deduplicate exports by name (overloads, `export default` + `export { x as default }`).
        let mut seen: FxHashMap<String, usize> = FxHashMap::default();
        let mut out: Vec<Export> = Vec::with_capacity(self.facts.exports.len());
        for e in std::mem::take(&mut self.facts.exports) {
            if let Some(&i) = seen.get(&e.name) {
                let prev = &mut out[i];
                prev.used_in_file |= e.used_in_file;
                if prev.kind == ExportKind::Unknown {
                    prev.kind = e.kind;
                }
                if prev.members.is_empty() {
                    prev.members = e.members;
                }
                prev.members_used_in_file.merge(&e.members_used_in_file);
                continue;
            }
            seen.insert(e.name.clone(), out.len());
            out.push(e);
        }
        self.facts.exports = out;
        if self.collect_strings {
            self.facts.strings.sort();
            self.facts.strings.dedup();
        }
    }

    /// `/// <reference types="x" />` and `/// <reference path="./x.d.ts" />`
    fn reference_directives(&mut self) {
        for c in self.comments {
            if c.is_block() {
                continue;
            }
            let text = &self.source[c.span.start as usize..c.span.end as usize];
            let Some(rest) = text.strip_prefix("///") else { continue };
            let rest = rest.trim();
            if !rest.starts_with("<reference") {
                continue;
            }
            for (attr, is_types) in [("types=", true), ("path=", false), ("lib=", false)] {
                if let Some(i) = rest.find(attr) {
                    let v = &rest[i + attr.len()..];
                    let q = v.chars().next().unwrap_or('"');
                    let v = v.trim_start_matches(q);
                    if let Some(end) = v.find(q) {
                        let value = v[..end].to_string();
                        if attr == "lib=" {
                            continue;
                        }
                        self.push_import(value, ImportKind::Reference, is_types, Names::Named(vec![]), c.span.start);
                    }
                }
            }
        }
    }

    fn is_top_level(&self, id: oxc_semantic::NodeId) -> bool {
        matches!(self.sem.nodes().parent_kind(id), AstKind::Program(_))
    }

    fn push_import(&mut self, specifier: String, kind: ImportKind, is_type: bool, names: Names, start: u32) -> usize {
        let (line, col) = self.lines.line_col(start);
        self.facts.imports.push(Import { specifier, kind, is_type, names, start, line, col });
        self.facts.imports.len() - 1
    }

    #[allow(clippy::too_many_arguments)]
    fn push_export(
        &mut self,
        name: String,
        kind: ExportKind,
        members: Vec<String>,
        start: u32,
        used_in_file: bool,
        members_used_in_file: Members,
        reexport: Option<(usize, String)>,
        tags: Vec<String>,
    ) {
        let (line, col) = self.lines.line_col(start);
        self.facts.exports.push(Export {
            name,
            kind,
            members,
            start,
            line,
            col,
            used_in_file,
            members_used_in_file,
            reexport,
            tags,
            referenced_in: vec![],
            is_binding_element: false,
            symbol: None,
        });
    }

    /// Attach the local symbol to the most recently pushed export.
    fn set_symbol(&mut self, sym: Option<SymbolId>) {
        if let Some(e) = self.facts.exports.last_mut() {
            e.symbol = sym;
        }
    }

    /// Fill `referenced_in`: for every export with a symbol, find the top-level statements that
    /// reference it and map those statements back to the exports they declare.
    /// knip's declaration-emit rule: an export stays alive while it is referenced (in a type position)
    /// from the *signature* of a used exported value: parameter/return/variable annotations, class
    /// member types, `typeof x`. References inside function bodies or inside other type aliases and
    /// interfaces do not count (tsc would flag those as unused too).
    fn compute_referenced_in(&mut self) {
        let nodes = self.sem.nodes();
        // top-level statement span start → value export indices declared by it
        let mut by_stmt: FxHashMap<u32, Vec<usize>> = FxHashMap::default();
        for (i, e) in self.facts.exports.iter().enumerate() {
            if matches!(e.kind, ExportKind::Type | ExportKind::Enum | ExportKind::Namespace) {
                continue;
            }
            if let Some(sym) = e.symbol {
                let decl = self.sem.symbol_declaration(sym);
                let mut id = decl.id();
                loop {
                    let pid = nodes.parent_id(id);
                    if matches!(nodes.kind(pid), AstKind::Program(_)) {
                        break;
                    }
                    id = pid;
                }
                by_stmt.entry(nodes.kind(id).span().start).or_default().push(i);
            }
        }
        for i in 0..self.facts.exports.len() {
            let Some(sym) = self.facts.exports[i].symbol else { continue };
            let mut refs: Vec<usize> = Vec::new();
            for r in self.sem.symbol_references(sym) {
                let mut pid = nodes.parent_id(r.node_id());
                while matches!(nodes.kind(pid), AstKind::TSQualifiedName(_)) {
                    pid = nodes.parent_id(pid);
                }
                if !matches!(nodes.kind(pid), AstKind::TSTypeQuery(_) | AstKind::TSTypeReference(_)) {
                    continue;
                }
                // Walk to the top-level statement. A parameter / return-type / class-member annotation
                // (at any depth) is part of the emitted declaration; a reference inside a function body
                // that is not such an annotation is an implementation detail.
                let mut id = r.node_id();
                let mut signature = false;
                let mut in_body = false;
                loop {
                    let pid = nodes.parent_id(id);
                    if matches!(nodes.kind(pid), AstKind::Program(_)) {
                        break;
                    }
                    if !in_body {
                        match nodes.kind(pid) {
                            AstKind::FormalParameter(_) | AstKind::PropertyDefinition(_) | AstKind::MethodDefinition(_) => signature = true,
                            AstKind::Function(_) | AstKind::ArrowFunctionExpression(_) if matches!(nodes.kind(id), AstKind::TSTypeAnnotation(_)) => {
                                signature = true
                            }
                            _ => {}
                        }
                    }
                    if !signature {
                        if matches!(nodes.kind(pid), AstKind::FunctionBody(_)) {
                            in_body = true;
                        }
                        // expression-bodied arrow: `() => ({} as T)` is a body reference, `(x: T) =>` is not
                        if let AstKind::ArrowFunctionExpression(a) = nodes.kind(pid) {
                            if a.body.span().start <= nodes.kind(id).span().start && nodes.kind(id).span().end <= a.body.span().end {
                                in_body = true;
                            }
                        }
                    }
                    id = pid;
                }
                if in_body && !signature {
                    continue;
                }
                if let Some(list) = by_stmt.get(&nodes.kind(id).span().start) {
                    for &j in list {
                        if j != i && !refs.contains(&j) {
                            refs.push(j);
                        }
                    }
                }
            }
            self.facts.exports[i].referenced_in = refs;
        }
    }

    /// JSDoc tags of the block comment directly preceding `start`.
    fn tags_before(&self, start: u32) -> Vec<String> {
        let idx = self.comments.partition_point(|c| c.span.end <= start);
        if idx == 0 {
            return vec![];
        }
        let c = &self.comments[idx - 1];
        if !c.is_block() {
            return vec![];
        }
        let between = &self.source[c.span.end as usize..start as usize];
        if !between.trim().is_empty() {
            return vec![];
        }
        let text = &self.source[c.span.start as usize..c.span.end as usize];
        if !text.starts_with("/**") {
            return vec![];
        }
        text.split('@')
            .skip(1)
            .filter_map(|t| t.split(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '-').next())
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// How a local binding is used: (referenced at all, members accessed).
    fn member_usage(&self, sym: Option<SymbolId>) -> (bool, Members) {
        let Some(sym) = sym else { return (false, Members::Named(vec![])) };
        let sem = self.sem;
        let nodes = sem.nodes();
        let mut names: Vec<String> = Vec::new();
        let mut all = false;
        let mut any = false;
        for r in sem.symbol_references(sym) {
            let nid = r.node_id();
            let ident_span = nodes.get_node(nid).kind().span();
            let parent = nodes.parent_node(nid);
            match parent.kind() {
                AstKind::ExportSpecifier(_) | AstKind::ExportDefaultDeclaration(_) => continue,
                AstKind::StaticMemberExpression(m) if m.object.span() == ident_span => {
                    let n = m.property.name.to_string();
                    if !names.contains(&n) {
                        names.push(n);
                    }
                }
                AstKind::ComputedMemberExpression(m) if m.object.span() == ident_span => match string_arg(&m.expression) {
                    Some(n) => {
                        if !names.contains(&n) {
                            names.push(n);
                        }
                    }
                    None => all = true,
                },
                AstKind::TSQualifiedName(q) if q.left.span() == ident_span => {
                    let n = q.right.name.to_string();
                    if !names.contains(&n) {
                        names.push(n);
                    }
                }
                _ => all = true,
            }
            any = true;
        }
        (any, if all { Members::All } else { Members::Named(names) })
    }

    fn used_in_file(&self, sym: Option<SymbolId>) -> bool {
        let Some(sym) = sym else { return false };
        let nodes = self.sem.nodes();
        self.sem.symbol_references(sym).any(|r| !matches!(nodes.parent_kind(r.node_id()), AstKind::ExportSpecifier(_) | AstKind::ExportDefaultDeclaration(_)))
    }

    fn import_declaration(&mut self, decl: &ImportDeclaration<'a>) {
        let is_type = decl.import_kind.is_type();
        let mut names = Vec::new();
        let mut syms: Vec<(SymbolId, String)> = Vec::new();
        if let Some(specs) = &decl.specifiers {
            for s in specs {
                match s {
                    ImportDeclarationSpecifier::ImportSpecifier(s) => {
                        let (_, members) = self.member_usage(s.local.symbol_id.get());
                        let name = export_name(&s.imported);
                        if let Some(sym) = s.local.symbol_id.get() {
                            syms.push((sym, name.clone()));
                        }
                        names.push(ImportedName { name, is_type: is_type || s.import_kind.is_type(), members });
                    }
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                        let (_, members) = self.member_usage(s.local.symbol_id.get());
                        if let Some(sym) = s.local.symbol_id.get() {
                            syms.push((sym, "default".into()));
                        }
                        names.push(ImportedName { name: "default".into(), is_type, members });
                    }
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                        let (_, members) = self.member_usage(s.local.symbol_id.get());
                        if let Some(sym) = s.local.symbol_id.get() {
                            syms.push((sym, "*".into()));
                        }
                        names.push(ImportedName { name: "*".into(), is_type, members });
                    }
                }
            }
        }
        let idx = self.push_import(decl.source.value.to_string(), ImportKind::Static, is_type, Names::Named(names), decl.span.start);
        for (sym, name) in syms {
            self.import_symbols.insert(sym, (idx, name));
        }
    }

    fn export_from(&mut self, decl: &ExportFromDeclaration<'a>) {
        let tags = self.tags_before(decl.span.start);
        let type_only = decl.export_kind.is_type();
        // export { a as b } from 'x'
        let mut names = Vec::new();
        let mut pending = Vec::new();
        for s in &decl.specifiers {
            let imported = export_name(&s.local);
            let exported = export_name(&s.exported);
            let is_type = type_only || s.export_kind.is_type();
            names.push(ImportedName { name: imported.clone(), is_type, members: Members::Named(vec![]) });
            pending.push((exported, imported, is_type, s.exported.span().start));
        }
        let idx = self.push_import(decl.source.value.to_string(), ImportKind::ReExport, type_only, Names::Named(names), decl.span.start);
        for (exported, imported, is_type, start) in pending {
            let kind = if is_type { ExportKind::Type } else { ExportKind::Unknown };
            // `import { X } from './m'; export { X } from './m'`: the local binding's references decide
            // type visibility for the re-export (see compute_referenced_in)
            let local = self
                .import_symbols
                .iter()
                .find(|(_, (i, n))| *n == imported && self.facts.imports[*i].specifier == decl.source.value.as_str())
                .map(|(sym, _)| *sym);
            self.push_export(exported, kind, vec![], start, false, Members::Named(vec![]), Some((idx, imported)), tags.clone());
            if local.is_some() {
                self.set_symbol(local);
            }
        }
    }

    fn export_named(&mut self, decl: &ExportNamedDeclaration<'a>) {
        let tags = self.tags_before(decl.span.start);
        let type_only = decl.export_kind.is_type();
        // export { a as b }
        for s in &decl.specifiers {
            let exported = export_name(&s.exported);
            let is_type = type_only || s.export_kind.is_type();
            let start = s.exported.span().start;
            let sym = match &s.local {
                ModuleExportName::IdentifierReference(r) => r.reference_id.get().and_then(|rid| self.sem.scoping().get_reference(rid).symbol_id()),
                _ => None,
            };
            if let Some(sym) = sym {
                if let Some((idx, imported)) = self.import_symbols.get(&sym).cloned() {
                    let kind = if is_type { ExportKind::Type } else { ExportKind::Unknown };
                    self.push_export(exported, kind, vec![], start, false, Members::Named(vec![]), Some((idx, imported)), tags.clone());
                    // the import binding's references decide type visibility (see compute_referenced_in)
                    self.set_symbol(Some(sym));
                    continue;
                }
                let (kind, members) = self.classify_symbol(sym);
                let kind = if is_type && kind != ExportKind::Enum { ExportKind::Type } else { kind };
                let used = self.used_in_file(Some(sym));
                let (_, mu) = self.member_usage(Some(sym));
                self.push_export(exported, kind, members, start, used, mu, None, tags.clone());
                self.set_symbol(Some(sym));
            } else {
                let kind = if is_type { ExportKind::Type } else { ExportKind::Unknown };
                self.push_export(exported, kind, vec![], start, false, Members::Named(vec![]), None, tags.clone());
            }
        }
    }

    fn classify_symbol(&self, sym: SymbolId) -> (ExportKind, Vec<String>) {
        match self.sem.symbol_declaration(sym).kind() {
            AstKind::Class(_) => (ExportKind::Class, vec![]),
            AstKind::TSTypeAliasDeclaration(_) | AstKind::TSInterfaceDeclaration(_) => (ExportKind::Type, vec![]),
            AstKind::TSEnumDeclaration(e) => (ExportKind::Enum, enum_members(e)),
            AstKind::TSNamespaceDeclaration(_) => (ExportKind::Namespace, vec![]),
            AstKind::VariableDeclarator(_) | AstKind::Function(_) => (ExportKind::Value, vec![]),
            _ => (ExportKind::Value, vec![]),
        }
    }

    fn declaration(&mut self, d: &Declaration<'a>, tags: &[String]) {
        match d {
            Declaration::VariableDeclaration(v) => {
                for decl in &v.declarations {
                    let mut names = Vec::new();
                    binding_names(&decl.id, &mut names);
                    let is_pattern = !matches!(decl.id, BindingPattern::BindingIdentifier(_));
                    for (name, start, sym) in names {
                        let used = self.used_in_file(sym);
                        let (_, mu) = self.member_usage(sym);
                        self.push_export(name, ExportKind::Value, vec![], start, used, mu, None, tags.to_vec());
                        self.set_symbol(sym);
                        if let Some(e) = self.facts.exports.last_mut() {
                            e.is_binding_element = is_pattern;
                        }
                    }
                }
            }
            Declaration::FunctionDeclaration(f) => {
                if let Some(id) = &f.id {
                    let used = self.used_in_file(id.symbol_id.get());
                    self.push_export(id.name.to_string(), ExportKind::Value, vec![], id.span.start, used, Members::Named(vec![]), None, tags.to_vec());
                    self.set_symbol(id.symbol_id.get());
                }
            }
            Declaration::ClassDeclaration(c) => {
                if let Some(id) = &c.id {
                    let used = self.used_in_file(id.symbol_id.get());
                    let (_, mu) = self.member_usage(id.symbol_id.get());
                    self.push_export(id.name.to_string(), ExportKind::Class, class_members(c), id.span.start, used, mu, None, tags.to_vec());
                    self.set_symbol(id.symbol_id.get());
                }
            }
            Declaration::TSTypeAliasDeclaration(t) => {
                let used = self.used_in_file(t.id.symbol_id.get());
                self.push_export(t.id.name.to_string(), ExportKind::Type, vec![], t.id.span.start, used, Members::Named(vec![]), None, tags.to_vec());
                self.set_symbol(t.id.symbol_id.get());
            }
            Declaration::TSInterfaceDeclaration(t) => {
                let used = self.used_in_file(t.id.symbol_id.get());
                self.push_export(t.id.name.to_string(), ExportKind::Type, vec![], t.id.span.start, used, Members::Named(vec![]), None, tags.to_vec());
                self.set_symbol(t.id.symbol_id.get());
            }
            Declaration::TSEnumDeclaration(e) => {
                let used = self.used_in_file(e.id.symbol_id.get());
                let (_, mu) = self.member_usage(e.id.symbol_id.get());
                self.push_export(e.id.name.to_string(), ExportKind::Enum, enum_members(e), e.id.span.start, used, mu, None, tags.to_vec());
                self.set_symbol(e.id.symbol_id.get());
            }
            Declaration::TSNamespaceDeclaration(n) => {
                let used = self.used_in_file(n.id.symbol_id.get());
                let (_, mu) = self.member_usage(n.id.symbol_id.get());
                self.push_export(n.id.name.to_string(), ExportKind::Namespace, vec![], n.id.span.start, used, mu, None, tags.to_vec());
                self.set_symbol(n.id.symbol_id.get());
            }
            Declaration::TSImportEqualsDeclaration(i) => {
                let used = self.used_in_file(i.id.symbol_id.get());
                self.push_export(i.id.name.to_string(), ExportKind::Value, vec![], i.id.span.start, used, Members::Named(vec![]), None, tags.to_vec());
            }
            Declaration::TSExternalModuleDeclaration(_) | Declaration::TSGlobalDeclaration(_) => {}
        }
    }

    fn export_default(&mut self, decl: &ExportDefaultDeclaration<'a>) {
        let tags = self.tags_before(decl.span.start);
        let (kind, used, members, mu) = match &decl.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                let sym = f.id.as_ref().and_then(|i| i.symbol_id.get());
                (ExportKind::Value, self.used_in_file(sym), vec![], Members::Named(vec![]))
            }
            ExportDefaultDeclarationKind::ClassDeclaration(c) => {
                let sym = c.id.as_ref().and_then(|i| i.symbol_id.get());
                (ExportKind::Class, self.used_in_file(sym), class_members(c), self.member_usage(sym).1)
            }
            ExportDefaultDeclarationKind::TSInterfaceDeclaration(t) => {
                (ExportKind::Type, self.used_in_file(t.id.symbol_id.get()), vec![], Members::Named(vec![]))
            }
            other => {
                // `export default foo` where foo is an import binding → re-export
                if let Some(e) = other.as_expression() {
                    if let Expression::Identifier(id) = e {
                        let sym = id.reference_id.get().and_then(|rid| self.sem.scoping().get_reference(rid).symbol_id());
                        if let Some(sym) = sym {
                            if let Some((idx, imported)) = self.import_symbols.get(&sym).cloned() {
                                self.push_export(
                                    "default".into(),
                                    ExportKind::Unknown,
                                    vec![],
                                    decl.span.start,
                                    false,
                                    Members::Named(vec![]),
                                    Some((idx, imported)),
                                    tags,
                                );
                                return;
                            }
                            let (kind, members) = self.classify_symbol(sym);
                            let used = self.used_in_file(Some(sym));
                            let (_, mu) = self.member_usage(Some(sym));
                            self.push_export("default".into(), kind, members, decl.span.start, used, mu, None, tags);
                            return;
                        }
                    }
                }
                (ExportKind::Value, false, vec![], Members::Named(vec![]))
            }
        };
        self.push_export("default".into(), kind, members, decl.span.start, used, mu, None, tags);
    }

    fn export_all(&mut self, decl: &ExportAllDeclaration<'a>) {
        let is_type = decl.export_kind.is_type();
        let idx = self.push_import(decl.source.value.to_string(), ImportKind::ReExport, is_type, Names::Named(vec![]), decl.span.start);
        match &decl.exported {
            Some(name) => {
                let tags = self.tags_before(decl.span.start);
                let kind = if is_type { ExportKind::Type } else { ExportKind::Namespace };
                self.push_export(export_name(name), kind, vec![], name.span().start, false, Members::Named(vec![]), Some((idx, "*".into())), tags);
            }
            None => self.facts.export_alls.push(ExportAll { import: idx }),
        }
    }

    /// Names demanded from a dynamic import / require() based on how the call result is consumed.
    /// `fn(() => import('./x'))` / `fn(async () => { return await import('./x') })`: the import is the
    /// whole result of a function passed as a call argument (route loaders, `lazy()`).
    fn is_inline_loader(&self, id: oxc_semantic::NodeId) -> bool {
        let nodes = self.sem.nodes();
        let mut pid = nodes.parent_id(id);
        while matches!(nodes.kind(pid), AstKind::AwaitExpression(_) | AstKind::ParenthesizedExpression(_) | AstKind::TSAsExpression(_)) {
            pid = nodes.parent_id(pid);
        }
        // `() => import('./x')`: expression-bodied arrow, the import is the body itself
        let func = if matches!(nodes.kind(pid), AstKind::ArrowFunctionExpression(_)) {
            pid
        } else if matches!(nodes.kind(pid), AstKind::ReturnStatement(_)) {
            let body = nodes.parent_id(pid);
            if !matches!(nodes.kind(body), AstKind::FunctionBody(_)) {
                return false;
            }
            let func = nodes.parent_id(body);
            if !matches!(nodes.kind(func), AstKind::ArrowFunctionExpression(_) | AstKind::Function(_)) {
                return false;
            }
            func
        } else {
            return false;
        };
        matches!(nodes.kind(nodes.parent_id(func)), AstKind::CallExpression(_))
    }

    fn names_from_parent(&self, id: oxc_semantic::NodeId) -> Names {
        let nodes = self.sem.nodes();
        let mut pid = nodes.parent_id(id);
        // unwrap `await` and `(...)`
        loop {
            match nodes.kind(pid) {
                AstKind::AwaitExpression(_)
                | AstKind::ParenthesizedExpression(_)
                | AstKind::TSAsExpression(_)
                | AstKind::TSNonNullExpression(_)
                | AstKind::TSSatisfiesExpression(_) => {
                    pid = nodes.parent_id(pid);
                }
                _ => break,
            }
        }
        // `const [a, { b }] = await Promise.all([import('x'), import('y')])`
        if let AstKind::ArrayExpression(arr) = nodes.kind(pid) {
            let my_span = nodes.kind(id).span();
            let index = arr.elements.iter().position(|el| el.as_expression().map(|e| e.span() == my_span).unwrap_or(false));
            let call_id = nodes.parent_id(pid);
            if let (Some(index), AstKind::CallExpression(call)) = (index, nodes.kind(call_id)) {
                let is_promise_all =
                    matches!(&call.callee, Expression::StaticMemberExpression(m) if m.property.name == "all" || m.property.name == "allSettled");
                if is_promise_all {
                    let mut p2 = nodes.parent_id(call_id);
                    while matches!(nodes.kind(p2), AstKind::AwaitExpression(_) | AstKind::ParenthesizedExpression(_) | AstKind::TSAsExpression(_)) {
                        p2 = nodes.parent_id(p2);
                    }
                    if let AstKind::VariableDeclarator(v) = nodes.kind(p2) {
                        if let BindingPattern::ArrayPattern(ap) = &v.id {
                            return match ap.elements.get(index) {
                                Some(Some(BindingPattern::BindingIdentifier(b))) => {
                                    let (_, members) = self.member_usage(b.symbol_id.get());
                                    Names::Named(vec![ImportedName { name: "*".into(), is_type: false, members }])
                                }
                                Some(Some(BindingPattern::ObjectPattern(o))) => {
                                    if o.rest.is_some() {
                                        return Names::All;
                                    }
                                    let mut names = Vec::new();
                                    for p in &o.properties {
                                        match property_key_name(&p.key) {
                                            Some(k) => names.push(ImportedName { name: k, is_type: false, members: Members::All }),
                                            None => return Names::All,
                                        }
                                    }
                                    Names::Named(names)
                                }
                                Some(None) => Names::Named(vec![]),
                                _ => Names::All,
                            };
                        }
                    }
                }
            }
        }
        // `import('./x').then(m => m.a)` / `.then(({ a }) => ...)`
        if let AstKind::StaticMemberExpression(m) = nodes.kind(pid) {
            if m.property.name == "then" {
                if let AstKind::CallExpression(call) = nodes.kind(nodes.parent_id(pid)) {
                    if let Some(arg) = call.arguments.first().and_then(|a| a.as_expression()) {
                        let params = match arg {
                            Expression::ArrowFunctionExpression(f) => Some(&f.params),
                            Expression::FunctionExpression(f) => Some(&f.params),
                            _ => None,
                        };
                        if let Some(params) = params {
                            if let Some(first) = params.items.first() {
                                match &first.pattern {
                                    BindingPattern::BindingIdentifier(b) => {
                                        let (_, members) = self.member_usage(b.symbol_id.get());
                                        return Names::Named(vec![ImportedName { name: "*".into(), is_type: false, members }]);
                                    }
                                    BindingPattern::ObjectPattern(o) if o.rest.is_none() => {
                                        let mut names = Vec::new();
                                        for p in &o.properties {
                                            match property_key_name(&p.key) {
                                                Some(k) => names.push(ImportedName { name: k, is_type: false, members: Members::All }),
                                                None => return Names::All,
                                            }
                                        }
                                        return Names::Named(names);
                                    }
                                    _ => {}
                                }
                            } else {
                                return Names::Named(vec![]);
                            }
                        }
                    }
                }
            }
        }
        match nodes.kind(pid) {
            AstKind::VariableDeclarator(v) => match &v.id {
                BindingPattern::BindingIdentifier(b) => {
                    let (_, members) = self.member_usage(b.symbol_id.get());
                    Names::Named(vec![ImportedName { name: "*".into(), is_type: false, members }])
                }
                BindingPattern::ObjectPattern(o) => {
                    if o.rest.is_some() {
                        return Names::All;
                    }
                    let mut names = Vec::new();
                    for p in &o.properties {
                        match property_key_name(&p.key) {
                            Some(k) => names.push(ImportedName { name: k, is_type: false, members: Members::All }),
                            None => return Names::All,
                        }
                    }
                    Names::Named(names)
                }
                _ => Names::All,
            },
            AstKind::StaticMemberExpression(m) => {
                let name = m.property.name.to_string();
                if name == "then" || name == "catch" || name == "finally" {
                    return Names::All;
                }
                Names::Named(vec![ImportedName { name, is_type: false, members: Members::All }])
            }
            AstKind::ExpressionStatement(_) => Names::Named(vec![]),
            _ => Names::All,
        }
    }

    fn call_expression(&mut self, id: oxc_semantic::NodeId, call: &CallExpression<'a>) {
        match &call.callee {
            Expression::Identifier(callee) if callee.name == "require" => {
                if let Some(spec) = first_string_arg(&call.arguments) {
                    let names = self.names_from_parent(id);
                    self.push_import(spec, ImportKind::Require, false, names, call.span.start);
                    self.facts.is_cjs = true;
                }
            }
            Expression::StaticMemberExpression(m) => {
                let prop = m.property.name.as_str();
                if matches!(&m.object, Expression::Identifier(obj) if obj.name == "require") {
                    if prop == "resolve" {
                        if let Some(spec) = first_string_arg(&call.arguments) {
                            self.push_import(spec, ImportKind::Require, false, Names::Named(vec![]), call.span.start);
                        }
                    } else if prop == "context" {
                        if let Some(dir) = first_string_arg(&call.arguments) {
                            let recursive = matches!(call.arguments.get(1), Some(Argument::BooleanLiteral(b)) if b.value);
                            let glob = format!("{}/{}", dir.trim_end_matches('/'), if recursive { "**/*" } else { "*" });
                            self.push_import(glob, ImportKind::Glob, false, Names::All, call.span.start);
                        }
                    }
                } else if let Expression::ImportMeta(_) = &m.object {
                    if prop == "resolve" {
                        if let Some(spec) = first_string_arg(&call.arguments) {
                            self.push_import(spec, ImportKind::Dynamic, false, Names::Named(vec![]), call.span.start);
                        }
                    } else if prop == "glob" || prop == "globEager" {
                        let mut globs = Vec::new();
                        match call.arguments.first() {
                            Some(Argument::ArrayExpression(a)) => {
                                for el in &a.elements {
                                    if let Some(e) = el.as_expression() {
                                        if let Some(s) = string_arg(e) {
                                            globs.push(s);
                                        }
                                    }
                                }
                            }
                            Some(a) => {
                                if let Some(e) = a.as_expression() {
                                    if let Some(s) = string_arg(e) {
                                        globs.push(s);
                                    }
                                }
                            }
                            None => {}
                        }
                        for g in globs {
                            self.push_import(g, ImportKind::Glob, false, Names::All, call.span.start);
                        }
                    }
                } else if let Expression::Identifier(obj) = &m.object {
                    let obj_name = obj.name.as_str();
                    if (obj_name == "jest" || obj_name == "vi" || obj_name == "vitest")
                        && matches!(
                            prop,
                            "mock"
                                | "doMock"
                                | "unmock"
                                | "requireActual"
                                | "requireMock"
                                | "importActual"
                                | "importMock"
                                | "createMockFromModule"
                                | "deepUnmock"
                                | "setMock"
                        )
                    {
                        if let Some(spec) = first_string_arg(&call.arguments) {
                            self.push_import(spec, ImportKind::Mock, false, Names::All, call.span.start);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn assignment(&mut self, assign: &AssignmentExpression<'a>) {
        let sem = self.sem;
        let AssignmentTarget::StaticMemberExpression(left) = &assign.left else { return };
        let is_module_exports = |e: &Expression| -> bool {
            match e {
                Expression::StaticMemberExpression(m) => m.property.name == "exports" && is_global_ident(sem, &m.object, "module"),
                _ => false,
            }
        };
        if left.property.name == "exports" && is_global_ident(sem, &left.object, "module") {
            // module.exports = ...
            self.facts.is_cjs = true;
            match &assign.right {
                Expression::ObjectExpression(o) => {
                    for p in &o.properties {
                        match p {
                            ObjectPropertyKind::ObjectProperty(p) => {
                                if let Some(k) = property_key_name(&p.key) {
                                    let start = p.key.span().start;
                                    self.push_export(k, ExportKind::Value, vec![], start, false, Members::Named(vec![]), None, vec![]);
                                }
                            }
                            ObjectPropertyKind::SpreadProperty(_) => {
                                self.push_export("default".into(), ExportKind::Value, vec![], assign.span.start, false, Members::All, None, vec![]);
                            }
                        }
                    }
                }
                _ => self.push_export("default".into(), ExportKind::Value, vec![], assign.span.start, false, Members::All, None, vec![]),
            }
        } else if is_module_exports(&left.object) || is_global_ident(sem, &left.object, "exports") {
            // module.exports.foo = / exports.foo =
            self.facts.is_cjs = true;
            let name = left.property.name.to_string();
            self.push_export(name, ExportKind::Value, vec![], left.property.span.start, false, Members::Named(vec![]), None, vec![]);
        }
    }
}

fn enum_members(e: &TSEnumDeclaration) -> Vec<String> {
    e.body
        .members
        .iter()
        .filter_map(|m| match &m.id {
            TSEnumMemberName::Identifier(i) => Some(i.name.to_string()),
            TSEnumMemberName::String(s) => Some(s.value.to_string()),
            TSEnumMemberName::ComputedString(s) => Some(s.value.to_string()),
            TSEnumMemberName::ComputedTemplateString(_) => None,
        })
        .collect()
}

fn class_members(c: &Class) -> Vec<String> {
    let mut out = Vec::new();
    for el in &c.body.body {
        let (key, is_private) = match el {
            ClassElement::MethodDefinition(m) => {
                (&m.key, m.accessibility == Some(TSAccessibility::Private) || matches!(m.key, PropertyKey::PrivateIdentifier(_)))
            }
            ClassElement::PropertyDefinition(p) => {
                (&p.key, p.accessibility == Some(TSAccessibility::Private) || matches!(p.key, PropertyKey::PrivateIdentifier(_)))
            }
            ClassElement::AccessorProperty(a) => (&a.key, matches!(a.key, PropertyKey::PrivateIdentifier(_))),
            _ => continue,
        };
        if is_private {
            continue;
        }
        if let Some(k) = property_key_name(key) {
            if k != "constructor" && !out.contains(&k) {
                out.push(k);
            }
        }
    }
    out
}

#[allow(dead_code)]
pub fn span_of<T: GetSpan>(t: &T) -> Span {
    t.span()
}
