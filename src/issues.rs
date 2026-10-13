//! Issue model shared by analysis and reporters.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IssueKind {
    Files,
    Dependencies,
    DevDependencies,
    OptionalPeerDependencies,
    Unlisted,
    Binaries,
    Unresolved,
    Exports,
    Types,
    EnumMembers,
    ClassMembers,
    Duplicates,
}

impl IssueKind {
    pub const ALL: &'static [IssueKind] = &[
        IssueKind::Files,
        IssueKind::Dependencies,
        IssueKind::DevDependencies,
        IssueKind::OptionalPeerDependencies,
        IssueKind::Unlisted,
        IssueKind::Binaries,
        IssueKind::Unresolved,
        IssueKind::Exports,
        IssueKind::Types,
        IssueKind::EnumMembers,
        IssueKind::ClassMembers,
        IssueKind::Duplicates,
    ];
    pub fn key(self) -> &'static str {
        match self {
            IssueKind::Files => "files",
            IssueKind::Dependencies => "dependencies",
            IssueKind::DevDependencies => "devDependencies",
            IssueKind::OptionalPeerDependencies => "optionalPeerDependencies",
            IssueKind::Unlisted => "unlisted",
            IssueKind::Binaries => "binaries",
            IssueKind::Unresolved => "unresolved",
            IssueKind::Exports => "exports",
            IssueKind::Types => "types",
            IssueKind::EnumMembers => "enumMembers",
            IssueKind::ClassMembers => "classMembers",
            IssueKind::Duplicates => "duplicates",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            IssueKind::Files => "Unused files",
            IssueKind::Dependencies => "Unused dependencies",
            IssueKind::DevDependencies => "Unused devDependencies",
            IssueKind::OptionalPeerDependencies => "Referenced optional peerDependencies",
            IssueKind::Unlisted => "Unlisted dependencies",
            IssueKind::Binaries => "Unlisted binaries",
            IssueKind::Unresolved => "Unresolved imports",
            IssueKind::Exports => "Unused exports",
            IssueKind::Types => "Unused exported types",
            IssueKind::EnumMembers => "Unused exported enum members",
            IssueKind::ClassMembers => "Unused exported class members",
            IssueKind::Duplicates => "Duplicate exports",
        }
    }
    pub fn parse(s: &str) -> Option<IssueKind> {
        IssueKind::ALL.iter().copied().find(|k| k.key().eq_ignore_ascii_case(s))
    }
    /// Included by default (knip parity: classMembers is opt-in).
    pub fn default_on(self) -> bool {
        !matches!(self, IssueKind::ClassMembers)
    }
}

#[derive(Debug, Clone)]
pub struct Issue {
    pub kind: IssueKind,
    /// Root-relative path.
    pub file: String,
    pub name: String,
    /// Parent symbol for members (`Enum` for `Enum.Member`).
    pub parent: Option<String>,
    pub line: u32,
    pub col: u32,
    pub pos: u32,
}
