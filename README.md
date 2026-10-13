# tidy

Find unused files, exports, types, enum members, dependencies and binaries in
JavaScript/TypeScript projects. A drop-in replacement for [knip](https://knip.dev)
that reads the same configuration and runs 30-50x faster.

```
$ cd my-monorepo && tidy
Unused files (2)
apps/web/src/components/OldBanner.tsx
packages/ui/src/stories/Legacy.tsx
Unused exports (1)
formatLegacy  packages/commons/src/format.ts:12:14
Unused devDependencies (1)
@types/lodash  apps/api/package.json
```

## Why it is fast

knip builds a TypeScript `Program` per workspace and asks the type checker
questions; that is where 80% of its time goes. tidy never type-checks:

- **oxc** parses every file (`oxc_parser` + `oxc_semantic`, the engine behind
  Vite 8 / Rolldown / oxlint) in parallel with rayon. Symbol references from
  the semantic pass give in-file usage and `Enum.Member` / `ns.export` access
  without a type checker.
- **oxc_resolver** resolves imports with full tsconfig `paths`, `extends`,
  `customConditions` and package `exports` support, one resolver per tsconfig.
- The `ignore` crate (ripgrep's walker) lists files while honouring
  `.gitignore`.
- Config files (`vite.config.ts`, `next.config.js`, `knip.ts`, ...) are read
  statically: literal strings that look like files or globs become entry
  files, no plugin code is ever executed.

On a 1.37M-line monorepo (8,000 source files, 24 workspaces, 16 cores) a full
run takes about 300 ms where knip takes 13.4 s. See `bench/compare.sh`.

## Usage

```
tidy [DIR] [--reporter text|json|flat] [--include files,exports,...]
     [--exclude ...] [--production] [--workspace apps/web] [--perf]
     [--explain path/to/file.ts] [--config tidy.json]
```

- `--reporter json` prints knip's JSON shape, so existing tooling keeps working.
- `--explain <file>` shows how a file was classified, how each import resolved,
  which exports are used and by whom. Start here when a result looks wrong.
- `--perf` prints per-phase timings.
- Exit code is 1 when issues are found (`--no-exit-code` to disable).

## Configuration

tidy reads `tidy.json`, `tidy.ts`, `knip.json`, `knip.jsonc`, `knip.ts` or the
`knip` key in `package.json`. TypeScript configs are evaluated statically:
the default export must be a literal object (`satisfies KnipConfig`,
`defineConfig({...})` and `const config = {...}; export default config` all
work).

Supported keys: `entry`, `project`, `ignore`, `ignoreDependencies`,
`ignoreBinaries`, `ignoreUnresolved`, `ignoreWorkspaces`, `workspaces`,
`includeEntryExports`, `ignoreExportsUsedInFile`, `include`, `exclude`,
`rules` (`"off"`), `tags`, and per-tool overrides (`vitest: false`,
`playwright: { config: [...] }`).

## What is detected

| Issue type | Notes |
| --- | --- |
| files | project files not reachable from any entry file |
| exports / types | exports no other file imports (knip semantics: entry files, `@public`, type references inside used type exports, destructured bindings) |
| enumMembers | members never accessed as `Enum.Member` anywhere |
| dependencies / devDependencies | listed but never imported, referenced in config files, scripts or by a used package's peer dependencies |
| unlisted | imported packages missing from this workspace or its ancestors |
| binaries | binaries in package.json scripts not provided by any dependency |
| unresolved | relative/aliased imports that do not resolve |
| classMembers | opt-in via `--include classMembers` (static access only) |

Entry files come from: config `entry`, `package.json` `main`/`module`/`bin`/
`exports` (dist paths are mapped back to `src`), files run by `package.json`
scripts and GitHub workflows, declaration files included by tsconfig, and a
built-in table of ~100 tool conventions (vitest, jest, playwright, next,
expo, storybook, astro, sveltekit, wxt, ...).

## Development

```
cargo build --release
cargo test --release          # fixture projects under tests/fixtures
bench/compare.sh <repo> 5     # wall-clock comparison with knip
```
