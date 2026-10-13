//! package.json script parsing: binaries used and source files referenced.

use crate::patterns::is_source_ext;
use crate::workspace::Manifest;
use rustc_hash::FxHashSet;
use std::path::Path;

/// Shell keywords, builtins and ubiquitous system commands that are never npm binaries.
pub const SYSTEM_BINARIES: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "fish",
    "cd",
    "ls",
    "rm",
    "cp",
    "mv",
    "mkdir",
    "rmdir",
    "echo",
    "cat",
    "git",
    "node",
    "bun",
    "npm",
    "npx",
    "pnpm",
    "pnpx",
    "yarn",
    "deno",
    "docker",
    "docker-compose",
    "kubectl",
    "helm",
    "curl",
    "wget",
    "env",
    "export",
    "true",
    "false",
    "exit",
    "test",
    "sleep",
    "find",
    "grep",
    "egrep",
    "rg",
    "sed",
    "awk",
    "mapfile",
    "readarray",
    "comm",
    "paste",
    "join",
    "fold",
    "split",
    "rev",
    "nl",
    "od",
    "xxd",
    "hexdump",
    "install",
    "shopt",
    "complete",
    "bind",
    "builtin",
    "caller",
    "dirs",
    "disown",
    "enable",
    "fc",
    "fg",
    "bg",
    "jobs",
    "let",
    "logout",
    "popd",
    "pushd",
    "suspend",
    "times",
    "ulimit",
    "function",
    "select",
    "[",
    "[[",
    "!",
    "{",
    "}",
    "xargs",
    "tar",
    "zip",
    "unzip",
    "gzip",
    "touch",
    "chmod",
    "chown",
    "ln",
    "python",
    "python3",
    "pip",
    "pip3",
    "uv",
    "uvx",
    "cargo",
    "rustc",
    "make",
    "cmake",
    "open",
    "xdg-open",
    "kill",
    "pkill",
    "lsof",
    "which",
    "printf",
    "source",
    "set",
    "unset",
    "if",
    "then",
    "else",
    "elif",
    "fi",
    "for",
    "do",
    "done",
    "while",
    "until",
    "case",
    "esac",
    "in",
    "read",
    "tee",
    "head",
    "tail",
    "sort",
    "uniq",
    "wc",
    "tr",
    "cut",
    "diff",
    "date",
    "time",
    "jq",
    "yq",
    "gh",
    "op",
    "ssh",
    "scp",
    "rsync",
    "caddy",
    "java",
    "go",
    "mise",
    "direnv",
    "exec",
    "eval",
    "trap",
    "wait",
    "shift",
    "return",
    "break",
    "continue",
    "local",
    "declare",
    "typeset",
    "readonly",
    "alias",
    "cross-env",
    "sudo",
    "nohup",
    "nice",
    "command",
    "type",
    "hash",
    "pwd",
    "dirname",
    "basename",
    "realpath",
    "readlink",
    "stat",
    "du",
    "df",
    "ps",
    "top",
    "mktemp",
    "seq",
    "expr",
    "bc",
    "md5sum",
    "sha256sum",
    "sha1sum",
    "shasum",
    "cksum",
    "base64",
    "openssl",
    "gpg",
    "ssh-keygen",
    "ssh-agent",
    "ssh-add",
    "git-lfs",
    "brew",
    "apt",
    "apt-get",
    "pacman",
    "yay",
    "systemctl",
    "service",
    "aws",
    "gcloud",
    "az",
    "terraform",
    "pulumi",
    "flyctl",
    "fly",
    "heroku",
    "vercel",
    "netlify",
    "wrangler",
    "railway",
    "ngrok",
    "code",
    "cursor",
    "vim",
    "nvim",
    "nano",
    "less",
    "more",
    "clear",
    "reset",
    "tput",
    "stty",
    "watch",
    "timeout",
    "yes",
    "no",
    "tsc",
    "osascript",
    "say",
    "afplay",
    "pbcopy",
    "pbpaste",
    "xclip",
    "wl-copy",
    "notify-send",
    "caffeinate",
    "ulimit",
    "umask",
    "getconf",
    "nproc",
    "uname",
    "hostname",
    "whoami",
    "id",
    "groups",
    "sw_vers",
    "lsb_release",
    "ping",
    "nc",
    "netstat",
    "ss",
    "ip",
    "ifconfig",
    "dig",
    "nslookup",
    "host",
    "traceroute",
    "mtr",
    "iptables",
    "ufw",
    "xcodebuild",
    "xcrun",
    "pod",
    "gradle",
    "gradlew",
    "adb",
    "emulator",
    "simctl",
    "fastlane",
    "bundle",
    "gem",
    "ruby",
    "php",
    "composer",
    "dotnet",
    "mvn",
    "sbt",
    "scala",
    "kotlin",
    "swift",
    "perl",
    "lua",
    "r",
    "rscript",
    "julia",
    "ghc",
    "stack",
    "cabal",
    "elm",
    "nix",
    "nix-shell",
    "nix-build",
    "flake",
    "home-manager",
    "devbox",
    "devenv",
    "earthly",
    "dagger",
    "act",
    "lefthook",
    "pre-commit",
    "husky",
    "shellcheck",
    "shfmt",
    "hadolint",
    "actionlint",
    "codespell",
    "typos",
    "tilt",
    "skaffold",
    "minikube",
    "kind",
    "k3d",
    "k9s",
    "stern",
    "kubectx",
    "kubens",
    "istioctl",
    "linkerd",
    "argocd",
    "flux",
    "sops",
    "age",
    "vault",
    "consul",
    "nomad",
    "packer",
    "vagrant",
    "virsh",
    "qemu",
    "psql",
    "pg_dump",
    "pg_restore",
    "createdb",
    "dropdb",
    "mysql",
    "mysqldump",
    "sqlite3",
    "redis-cli",
    "mongo",
    "mongosh",
    "mongodump",
    "mongorestore",
    "prisma",
    "supabase",
    "turso",
    "neon",
    "planetscale",
    "pscale",
    "atlas",
    "dbmate",
    "goose",
    "migrate",
    "flyway",
    "liquibase",
    "sqlc",
    "ffmpeg",
    "ffprobe",
    "convert",
    "magick",
    "identify",
    "mogrify",
    "sips",
    "exiftool",
    "pngquant",
    "optipng",
    "jpegoptim",
    "cwebp",
    "dwebp",
    "gifsicle",
    "svgo",
    "inkscape",
    "rsvg-convert",
    "pandoc",
    "latex",
    "pdflatex",
    "xelatex",
    "lualatex",
    "bibtex",
    "biber",
    "makeindex",
    "dvipdf",
    "ps2pdf",
    "pdftotext",
    "pdftoppm",
    "qpdf",
    "mutool",
    "ghostscript",
    "gs",
];

/// Read the body of a `$( ... )` substitution (opening paren already consumed).
fn read_substitution(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut depth = 1;
    let mut inner = String::new();
    let mut q: Option<char> = None;
    for n in chars.by_ref() {
        if let Some(qc) = q {
            if n == qc {
                q = None;
            }
            inner.push(n);
            continue;
        }
        match n {
            '"' | '\'' => {
                q = Some(n);
                inner.push(n);
            }
            '(' => {
                depth += 1;
                inner.push(n);
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                inner.push(n);
            }
            _ => inner.push(n),
        }
    }
    inner
}

fn tokenize(script: &str) -> Vec<String> {
    let mut subs: Vec<String> = Vec::new();
    let mut tokens = tokenize_inner(script, &mut subs);
    // substitutions found inside double quotes are commands too
    let mut i = 0;
    while i < subs.len() {
        let inner = subs[i].clone();
        tokens.push(";".into());
        tokens.extend(tokenize_inner(&inner, &mut subs));
        tokens.push(";".into());
        i += 1;
    }
    tokens
}

fn tokenize_inner(script: &str, subs: &mut Vec<String>) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut chars = script.chars().peekable();
    let mut quote: Option<char> = None;
    let mut in_token = false;
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if c == '\\' && q == '"' {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            } else if c == '$' && q == '"' && chars.peek() == Some(&'(') {
                chars.next();
                let inner = read_substitution(&mut chars);
                subs.push(inner);
                cur.push_str("$SUB");
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                in_token = true;
            }
            '\\' => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    in_token = true;
                }
            }
            '$' if chars.peek() == Some(&'(') => {
                // command substitution: tokenize the inner command as its own command
                chars.next();
                let inner = read_substitution(&mut chars);
                if in_token {
                    tokens.push(std::mem::take(&mut cur));
                    in_token = false;
                }
                tokens.push(";".into());
                subs.push(inner);
            }
            '\n' => {
                if in_token {
                    tokens.push(std::mem::take(&mut cur));
                    in_token = false;
                }
                tokens.push(";".into());
            }
            '#' if !in_token => {
                // comment to end of line
                for n in chars.by_ref() {
                    if n == '\n' {
                        tokens.push(";".into());
                        break;
                    }
                }
            }
            c if c.is_whitespace() => {
                if in_token {
                    tokens.push(std::mem::take(&mut cur));
                    in_token = false;
                }
            }
            '&' | '|' | ';' | '(' | ')' => {
                if in_token {
                    tokens.push(std::mem::take(&mut cur));
                    in_token = false;
                }
                let mut op = c.to_string();
                if (c == '&' || c == '|') && chars.peek() == Some(&c) {
                    chars.next();
                    op.push(c);
                }
                tokens.push(op);
            }
            _ => {
                cur.push(c);
                in_token = true;
            }
        }
    }
    if in_token {
        tokens.push(cur);
    }
    tokens
}

fn split_commands(tokens: Vec<String>) -> Vec<Vec<String>> {
    let mut cmds: Vec<Vec<String>> = vec![vec![]];
    let mut i = 0;
    while i < tokens.len() {
        let t = &tokens[i];
        if matches!(t.as_str(), "&&" | "||" | ";" | "|" | "&" | "(" | ")") {
            // `label)` case patterns and `name()` function definitions are not commands
            if t == ")" || t == "(" {
                if let Some(last) = cmds.last_mut() {
                    if last.len() == 1 {
                        last.clear();
                    }
                }
            }
            if !cmds.last().unwrap().is_empty() {
                cmds.push(vec![]);
            }
        } else if t.ends_with(')') && !t.contains('(') {
            // `all)` glued case label
            if cmds.last().map(|c| c.is_empty()).unwrap_or(true) {
                // skip
            } else {
                cmds.last_mut().unwrap().push(t.clone());
            }
        } else {
            cmds.last_mut().unwrap().push(t.clone());
        }
        i += 1;
    }
    cmds.retain(|c| !c.is_empty());
    cmds
}

fn is_env_assignment(t: &str) -> bool {
    let Some(eq) = t.find('=') else { return false };
    let name = &t[..eq];
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit())
}

#[derive(Default, Debug)]
pub struct ScriptUse {
    pub binaries: Vec<String>,
    pub files: Vec<String>,
    /// Packages referenced by path (`node_modules/<pkg>`) or by well-known flags.
    pub packages: Vec<String>,
}

/// (binary, argument) pairs implying an extra package.
const ARG_PACKAGES: &[(&str, &str, &str)] = &[
    ("oxlint", "--type-aware", "oxlint-tsgolint"),
    ("oxlint", "--type-check", "oxlint-tsgolint"),
    ("vitest", "--coverage", "@vitest/coverage-v8"),
    ("vitest", "--ui", "@vitest/ui"),
    ("vitest", "--browser", "@vitest/browser"),
    ("jest", "--coverage", "jest"),
    ("tsx", "--test", "tsx"),
    ("node", "--import=tsx", "tsx"),
    ("node", "--loader=tsx", "tsx"),
    ("node", "--import=jiti/register", "jiti"),
    ("node", "--loader=ts-node/esm", "ts-node"),
    ("node", "--import=ts-node/esm", "ts-node"),
    ("node", "--require=ts-node/register", "ts-node"),
    ("eslint", "--format=json", "eslint"),
    ("playwright", "--ui", "@playwright/test"),
];

struct Ctx<'a> {
    manifest: &'a Manifest,
    dir: &'a Path,
    out: ScriptUse,
    visited_scripts: FxHashSet<String>,
    functions: FxHashSet<String>,
}

impl Ctx<'_> {
    fn file_ref(&mut self, t: &str) {
        let t = t.trim_start_matches("./");
        if t.is_empty() || t.contains('*') || t.starts_with('-') {
            return;
        }
        if is_source_ext(t) && self.dir.join(t).is_file() {
            let s = t.to_string();
            if !self.out.files.contains(&s) {
                self.out.files.push(s);
            }
        }
    }

    fn run_script(&mut self, name: &str) {
        if !self.visited_scripts.insert(name.to_string()) {
            return;
        }
        if let Some((_, s)) = self.manifest.scripts.iter().find(|(k, _)| k == name) {
            let s = s.clone();
            self.script(&s);
        }
    }

    fn script(&mut self, script: &str) {
        let tokens = tokenize(script);
        // `name() {` / `function name` define shell functions, not binaries
        for w in tokens.windows(3) {
            if w[1] == "(" && w[2] == ")" {
                self.functions.insert(w[0].clone());
            }
        }
        for w in tokens.windows(2) {
            if w[0] == "function" {
                self.functions.insert(w[1].trim_end_matches("()").to_string());
            }
        }
        for cmd in split_commands(tokens) {
            self.command(&cmd);
        }
    }

    fn command(&mut self, cmd: &[String]) {
        let mut i = 0;
        // file refs anywhere in the command
        for t in cmd {
            self.file_ref(t);
        }
        while i < cmd.len() && is_env_assignment(&cmd[i]) {
            i += 1;
        }
        if i >= cmd.len() {
            return;
        }
        let bin = cmd[i].as_str();
        let rest = &cmd[i + 1..];
        match bin {
            "sh" | "bash" | "zsh" => {
                if let Some(p) = rest.iter().position(|t| t == "-c") {
                    if let Some(code) = rest.get(p + 1) {
                        let code = code.clone();
                        self.script(&code);
                    }
                } else {
                    self.command_args(&cmd[i..]);
                }
            }
            "cross-env" | "env" | "sudo" | "time" | "nice" | "exec" | "nohup" | "command" | "xvfb-run" => {
                let mut j = 0;
                while j < rest.len() && (is_env_assignment(&rest[j]) || rest[j].starts_with('-')) {
                    j += 1;
                }
                if bin == "cross-env" {
                    self.push_binary("cross-env");
                }
                self.command(&rest[j..]);
            }
            "dotenv" | "dotenvx" => {
                self.push_binary(bin);
                if let Some(p) = rest.iter().position(|t| t == "--") {
                    self.command(&rest[p + 1..]);
                }
            }
            "npx" | "bunx" | "pnpx" => {
                let mut j = 0;
                while j < rest.len() && rest[j].starts_with('-') {
                    if matches!(rest[j].as_str(), "-p" | "--package") {
                        j += 1;
                    }
                    j += 1;
                }
                if j < rest.len() {
                    self.push_binary(&strip_version(&rest[j]));
                    self.command_args(&rest[j..]);
                }
            }
            "npm" | "pnpm" | "yarn" | "bun" => {
                if bin == "bun" && rest.iter().any(|t| matches!(t.as_str(), "-e" | "--eval" | "-p" | "--print")) {
                    return;
                }
                let mut j = 0;
                let mut other_workspace = false;
                while j < rest.len() && rest[j].starts_with('-') {
                    if matches!(rest[j].as_str(), "--filter" | "-F" | "--cwd" | "-C" | "--workspace" | "-w" | "--elide-lines" | "--concurrency") {
                        if matches!(rest[j].as_str(), "--filter" | "-F" | "--cwd" | "-C" | "--workspace" | "-w") {
                            other_workspace = true;
                        }
                        j += 1;
                    } else if rest[j].starts_with("--filter=") || rest[j].starts_with("--cwd=") || rest[j].starts_with("--workspace=") {
                        other_workspace = true;
                    }
                    j += 1;
                }
                if other_workspace {
                    // script/binary belongs to another workspace's package.json; only file refs matter here
                    return;
                }
                if j >= rest.len() {
                    return;
                }
                let sub = rest[j].as_str();
                match sub {
                    "run" | "run-script" => {
                        let mut k = j + 1;
                        while k < rest.len() && rest[k].starts_with('-') {
                            if matches!(rest[k].as_str(), "--filter" | "-F" | "--cwd" | "-C" | "--elide-lines" | "--concurrency") {
                                k += 1;
                            }
                            k += 1;
                        }
                        if k < rest.len() {
                            if is_source_ext(&rest[k]) {
                                self.file_ref(&rest[k]);
                            } else {
                                self.run_script(&rest[k]);
                            }
                        }
                    }
                    "exec" | "x" | "dlx" | "create" => {
                        if let Some(b) = rest.get(j + 1) {
                            self.push_binary(&strip_version(b));
                            self.command_args(&rest[j + 1..]);
                        }
                    }
                    "install" | "i" | "add" | "remove" | "rm" | "update" | "up" | "ci" | "link" | "unlink" | "publish" | "pack" | "init" | "login"
                    | "logout" | "version" | "outdated" | "audit" | "cache" | "config" | "ls" | "list" | "why" | "info" | "view" | "dedupe" | "prune"
                    | "rebuild" | "patch" | "patch-commit" | "pm" | "upgrade" | "build" | "test" | "start" | "dev" | "lint" | "format" | "check"
                    | "workspaces" | "set-version" => {
                        // package-manager built-ins; `bun test`/`bun build` are bun's own. `npm test`/`pnpm build` etc. run scripts.
                        if bin != "bun"
                            || !matches!(
                                sub,
                                "install"
                                    | "i"
                                    | "add"
                                    | "remove"
                                    | "rm"
                                    | "update"
                                    | "link"
                                    | "unlink"
                                    | "pm"
                                    | "x"
                                    | "build"
                                    | "test"
                                    | "init"
                                    | "create"
                                    | "upgrade"
                                    | "patch"
                                    | "outdated"
                                    | "publish"
                                    | "audit"
                                    | "info"
                                    | "why"
                                    | "ci"
                            )
                        {
                            self.run_script(sub);
                        }
                    }
                    _ => {
                        if is_source_ext(sub) {
                            self.file_ref(sub);
                        } else if self.manifest.scripts.iter().any(|(k, _)| k == sub) {
                            self.run_script(sub);
                        } else if bin == "bun" || bin == "yarn" {
                            // `bun <binary>` / `yarn <binary>`
                            self.push_binary(&strip_version(sub));
                            self.command_args(&rest[j..]);
                        }
                    }
                }
            }
            "node" | "tsx" | "ts-node" | "ts-node-esm" | "jiti" | "esno" | "deno" | "vite-node" | "babel-node" | "nodemon" | "node-dev" | "ts-node-dev"
            | "tsx-watch" => {
                if bin != "node" && bin != "deno" {
                    self.push_binary(bin);
                }
                if rest.iter().any(|t| matches!(t.as_str(), "-e" | "--eval" | "-p" | "--print")) {
                    return;
                }
                self.command_args(&cmd[i..]);
                // `nodemon --exec "cmd"`
                if let Some(p) = rest.iter().position(|t| t == "--exec" || t == "-x") {
                    if let Some(sub) = rest.get(p + 1) {
                        let sub = sub.clone();
                        self.script(&sub);
                    }
                }
            }
            "concurrently" | "npm-run-all" | "npm-run-all2" | "run-s" | "run-p" | "wait-on" | "with-env" | "turbo" | "nx" | "lerna" | "wireit" => {
                self.push_binary(bin);
                for t in rest {
                    if t.starts_with('-') {
                        continue;
                    }
                    if t.contains(' ') {
                        let t = t.clone();
                        self.script(&t);
                    } else if bin == "npm-run-all" || bin == "npm-run-all2" || bin == "run-s" || bin == "run-p" {
                        // script names, possibly globs like `lint:*`
                        if let Some(prefix) = t.strip_suffix('*') {
                            let names: Vec<String> = self.manifest.scripts.iter().filter(|(k, _)| k.starts_with(prefix)).map(|(k, _)| k.clone()).collect();
                            for n in names {
                                self.run_script(&n);
                            }
                        } else {
                            self.run_script(t);
                        }
                    } else if bin == "concurrently" {
                        if let Some(name) = t.strip_prefix("npm:") {
                            let names: Vec<String> = if let Some(prefix) = name.strip_suffix('*') {
                                self.manifest.scripts.iter().filter(|(k, _)| k.starts_with(prefix)).map(|(k, _)| k.clone()).collect()
                            } else {
                                vec![name.to_string()]
                            };
                            for n in names {
                                self.run_script(&n);
                            }
                        } else {
                            let t = t.clone();
                            self.script(&t);
                        }
                    }
                }
            }
            _ => {
                if bin.contains('/') {
                    self.file_ref(bin);
                    self.node_modules_ref(bin);
                    return;
                }
                self.push_binary(bin);
                self.command_args(&cmd[i..]);
            }
        }
    }

    /// `node_modules/<pkg>/...` path tokens reference the package.
    fn node_modules_ref(&mut self, t: &str) {
        if let Some(i) = t.find("node_modules/") {
            let rest = &t[i + "node_modules/".len()..];
            let mut parts = rest.split('/');
            let name = match parts.next() {
                Some(s) if s.starts_with('@') => match parts.next() {
                    Some(n) => format!("{s}/{n}"),
                    None => return,
                },
                Some(s) if !s.is_empty() && s != ".bin" => s.to_string(),
                _ => return,
            };
            if !self.out.packages.contains(&name) {
                self.out.packages.push(name);
            }
        }
    }

    /// Scan arguments for config/require-style file references.
    fn command_args(&mut self, args: &[String]) {
        if let Some(bin) = args.first() {
            for (b, arg, pkg) in ARG_PACKAGES {
                if bin == b && args.iter().skip(1).any(|a| a == arg) && !self.out.packages.contains(&pkg.to_string()) {
                    self.out.packages.push(pkg.to_string());
                }
            }
        }
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            if matches!(
                a,
                "--config"
                    | "-c"
                    | "--require"
                    | "-r"
                    | "--import"
                    | "--loader"
                    | "--experimental-loader"
                    | "--preload"
                    | "--setup"
                    | "--env-file"
                    | "--file"
                    | "-f"
            ) {
                if let Some(v) = args.get(i + 1) {
                    self.file_ref(v);
                    self.node_modules_ref(v);
                    if matches!(a, "--require" | "-r" | "--import" | "--loader" | "--experimental-loader" | "--preload") {
                        if let Some(pkg) = crate::resolve::package_name(v) {
                            if !self.out.packages.contains(&pkg) {
                                self.out.packages.push(pkg);
                            }
                        }
                    }
                }
                i += 2;
                continue;
            }
            if let Some(v) = a.strip_prefix("--require=").or_else(|| a.strip_prefix("--import=")).or_else(|| a.strip_prefix("--loader=")) {
                if let Some(pkg) = crate::resolve::package_name(v) {
                    if !self.out.packages.contains(&pkg) {
                        self.out.packages.push(pkg);
                    }
                }
                self.file_ref(v);
            }
            self.node_modules_ref(a);
            if let Some((k, v)) = a.split_once('=') {
                if k.starts_with("--") {
                    self.file_ref(v);
                }
            }
            i += 1;
        }
    }

    fn push_binary(&mut self, b: &str) {
        let b = b.trim();
        if !is_valid_binary_name(b) || self.functions.contains(b) {
            return;
        }
        let s = b.to_string();
        if !self.out.binaries.contains(&s) {
            self.out.binaries.push(s);
        }
    }
}

/// npm binary names: lowercase-ish identifiers like `tsc`, `relay-compiler`, `eas`.
pub fn is_valid_binary_name(b: &str) -> bool {
    !b.is_empty()
        && b.len() < 64
        && b.chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false)
        && b.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && !b.ends_with('.')
        && b.chars().any(|c| c.is_ascii_lowercase())
}

/// Remove heredoc bodies (`<<EOF ... EOF`) so their contents are not parsed as commands.
fn strip_heredocs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        out.push_str(line);
        out.push('\n');
        if let Some(pos) = line.find("<<") {
            let rest = line[pos + 2..].trim_start_matches('-').trim();
            let term: String = rest.trim_start_matches(['\'', '"']).chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
            if !term.is_empty() {
                for l in lines.by_ref() {
                    if l.trim() == term {
                        break;
                    }
                }
            }
        }
    }
    out
}

fn strip_version(s: &str) -> String {
    // `vitest@latest` → `vitest`, `@scope/pkg@1` → `@scope/pkg`
    if let Some(rest) = s.strip_prefix('@') {
        match rest.find('@') {
            Some(i) => format!("@{}", &rest[..i]),
            None => s.to_string(),
        }
    } else {
        s.split('@').next().unwrap_or(s).to_string()
    }
}

/// Analyze all scripts (and `lint-staged`/`husky`-like command strings elsewhere in package.json).
pub fn analyze(manifest: &Manifest, dir: &Path) -> ScriptUse {
    let mut ctx = Ctx { manifest, dir, out: ScriptUse::default(), visited_scripts: FxHashSet::default(), functions: FxHashSet::default() };
    let names: Vec<String> = manifest.scripts.iter().map(|(k, _)| k.clone()).collect();
    for n in names {
        ctx.run_script(&n);
    }
    for c in &manifest.commands {
        ctx.script(c);
    }
    // Plain file references elsewhere in package.json (e.g. `"types": "src/index.ts"`, `"prisma": {"schema": ...}`)
    for s in &manifest.strings {
        if !s.contains(' ') {
            ctx.file_ref(s);
        }
    }
    ctx.out
}

pub fn script_file_refs(manifest: &Manifest, dir: &Path) -> Vec<String> {
    analyze(manifest, dir).files
}

/// Parse shell-ish files (husky hooks, GitHub workflow `run:` lines) for binaries.
pub fn shell_text(text: &str, manifest: &Manifest, dir: &Path) -> ScriptUse {
    let mut ctx = Ctx { manifest, dir, out: ScriptUse::default(), visited_scripts: FxHashSet::default(), functions: FxHashSet::default() };
    ctx.script(&strip_heredocs(text));
    ctx.out
}
