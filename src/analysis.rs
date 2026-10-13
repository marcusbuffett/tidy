//! Turn the module graph into file/export/unresolved issues.

use crate::extract::{ExportKind, ImportKind, Members};
use crate::graph::Graph;
use crate::issues::{Issue, IssueKind};
use crate::project::{Project, WorkspaceSetup};
use crate::resolve::Target;

pub struct Options<'a> {
    pub production: bool,
    pub tags: &'a [String],
    pub class_members: bool,
}

pub fn analyze(project: &Project, graph: &Graph, setups: &[WorkspaceSetup], opts: &Options) -> Vec<Issue> {
    let _ = project;
    let mut issues = Vec::new();
    let (tag_include, tag_exclude): (Vec<&str>, Vec<&str>) = {
        let mut inc = vec![];
        let mut exc = vec![];
        for t in opts.tags {
            if let Some(x) = t.strip_prefix('-') {
                exc.push(x);
            } else if let Some(x) = t.strip_prefix('+') {
                inc.push(x);
            } else {
                inc.push(t.as_str());
            }
        }
        (inc, exc)
    };
    for m in &graph.modules {
        let setup = &setups[m.ws];
        if setup.is_ignored_workspace || m.is_ignored {
            continue;
        }
        if m.is_project && !m.reachable && !m.is_entry {
            issues.push(Issue { kind: IssueKind::Files, file: m.rel.clone(), name: String::new(), parent: None, line: 0, col: 0, pos: 0 });
            continue;
        }
        if !m.reachable {
            continue;
        }
        // unresolved imports
        for (ii, r) in &m.edges {
            if let Target::Unresolved(_) = r.target {
                let imp = &m.facts.imports[*ii];
                if r.package.is_some() || setup.ignore_unresolved.iter().any(|x| x.matches(&imp.specifier)) {
                    continue;
                }
                if matches!(imp.kind, ImportKind::Url | ImportKind::Reference | ImportKind::ConfigRef | ImportKind::Glob) {
                    continue;
                }
                if m.rel.ends_with(".d.ts") || m.rel.ends_with(".d.mts") || m.rel.ends_with(".d.cts") {
                    continue;
                }
                issues.push(Issue {
                    kind: IssueKind::Unresolved,
                    file: m.rel.clone(),
                    name: imp.specifier.clone(),
                    parent: None,
                    line: imp.line,
                    col: imp.col,
                    pos: imp.start,
                });
            }
        }
        if !m.is_project {
            continue;
        }
        if m.is_entry && (!setup.include_entry_exports || m.tool_entry) {
            continue;
        }
        if opts.production && m.dev_entry {
            continue;
        }
        for (i, e) in m.facts.exports.iter().enumerate() {
            if e.tags.iter().any(|t| t == "public" || t == "alias" || t == "beta") {
                continue;
            }
            // TS namespaces are containers; knip does not report them either.
            if e.kind == ExportKind::Namespace {
                continue;
            }
            if !tag_exclude.is_empty() && e.tags.iter().any(|t| tag_exclude.contains(&t.as_str())) {
                continue;
            }
            if !tag_include.is_empty() && !e.tags.iter().any(|t| tag_include.contains(&t.as_str())) {
                continue;
            }
            let used = &m.export_used[i];
            let kind_key = match e.kind {
                ExportKind::Type => "types",
                ExportKind::Enum => "enumMembers",
                ExportKind::Class => "classMembers",
                _ => "exports",
            };
            match used {
                None => {
                    // Referenced inside another export that is itself used (transitively): not dead.
                    if referenced_in_used_export(m, i) {
                        continue;
                    }
                    let ieuf = &setup.ignore_exports_used_in_file;
                    if e.used_in_file
                        && (e.is_binding_element || ieuf.applies(if e.kind == ExportKind::Type { "types" } else { "exports" }) || ieuf.applies(kind_key))
                    {
                        continue;
                    }
                    let kind = if matches!(e.kind, ExportKind::Type | ExportKind::Enum) { IssueKind::Types } else { IssueKind::Exports };
                    issues.push(Issue { kind, file: m.rel.clone(), name: e.name.clone(), parent: None, line: e.line, col: e.col, pos: e.start });
                }
                Some(members) => {
                    let report_members = match e.kind {
                        ExportKind::Enum => true,
                        ExportKind::Class => opts.class_members,
                        _ => false,
                    };
                    if !report_members || matches!(members, Members::All) || matches!(e.members_used_in_file, Members::All) {
                        continue;
                    }
                    let kind = if e.kind == ExportKind::Enum { IssueKind::EnumMembers } else { IssueKind::ClassMembers };
                    for mem in &e.members {
                        if members.contains(mem) || e.members_used_in_file.contains(mem) {
                            continue;
                        }
                        issues.push(Issue {
                            kind,
                            file: m.rel.clone(),
                            name: mem.clone(),
                            parent: Some(e.name.clone()),
                            line: e.line,
                            col: e.col,
                            pos: e.start,
                        });
                    }
                }
            }
        }
    }
    issues
}

/// knip semantics: an export referenced (in a type position) by the declaration of another export
/// that is used externally is part of the public type surface and not reported.
fn referenced_in_used_export(m: &crate::graph::Module, i: usize) -> bool {
    m.facts.exports[i].referenced_in.iter().any(|&j| m.export_used[j].is_some())
}
