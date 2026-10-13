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
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        for e in expand_braces(&normalize_glob(p)) {
            b.add(GlobBuilder::new(&e).literal_separator(true).build()?);
        }
    }
    Ok(b.build()?)
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
    /// Dependency names enabling this tool (prefix match when ending with `/`).
    pub enablers: &'static [&'static str],
    pub config: &'static [&'static str],
    pub entry: &'static [&'static str],
    /// Entry files that are only relevant outside production mode (tests, stories, ...).
    pub dev: bool,
}

const E: &[&str] = &[];

pub const TOOLS: &[Tool] = &[
    Tool { name: "vitest", enablers: &["vitest"], config: &["vitest.config.{js,mjs,ts,cjs,mts,cts}", "vitest.{workspace,projects}.{js,mjs,ts,cjs,mts,cts,json}"], entry: &["**/*.{bench,test,test-d,spec}.?(c|m)[jt]s?(x)", "**/__mocks__/**/*.[jt]s?(x)"], dev: true },
    Tool { name: "jest", enablers: &["jest"], config: &["jest.config.{js,ts,mjs,cjs,json}"], entry: &["**/__tests__/**/*.[jt]s?(x)", "**/?(*.)+(spec|test).[jt]s?(x)", "**/__mocks__/**/*.[jt]s?(x)"], dev: true },
    Tool { name: "bun", enablers: &["bun-types", "@types/bun"], config: &["bunfig.toml"], entry: &["**/*.{test,spec}.{js,jsx,ts,tsx}", "**/*_{test,spec}.{js,jsx,ts,tsx}"], dev: true },
    Tool { name: "node-test", enablers: &["@types/node"], config: E, entry: &["**/*.test.{js,mjs,cjs,ts,mts,cts}"], dev: true },
    Tool { name: "mocha", enablers: &["mocha"], config: &[".mocharc.{js,cjs,yml,yaml,json}", ".mocharc"], entry: &["**/test/*.{js,cjs,mjs,ts}", "**/test/**/*.{spec,test}.{js,cjs,mjs,ts}"], dev: true },
    Tool { name: "ava", enablers: &["ava"], config: &["ava.config.{js,cjs,mjs}"], entry: &["test.{js,cjs,mjs,ts}", "{src,source}/test.{js,cjs,mjs,ts}", "**/__tests__/**/*.{js,cjs,mjs,ts}", "**/*.spec.{js,cjs,mjs,ts}", "**/*.test.{js,cjs,mjs,ts}", "**/test-*.{js,cjs,mjs,ts}", "**/test/**/*.{js,cjs,mjs,ts}", "**/tests/**/*.{js,cjs,mjs,ts}"], dev: true },
    Tool { name: "playwright", enablers: &["@playwright/test"], config: &["playwright.config.{js,ts,mjs}", "playwright-ct.config.{js,ts,mjs}"], entry: &["**/*.@(spec|test).?(c|m)[jt]s?(x)"], dev: true },
    Tool { name: "cypress", enablers: &["cypress"], config: &["cypress.config.{js,ts,mjs,cjs}"], entry: &["cypress/e2e/**/*.cy.{js,jsx,ts,tsx}", "cypress/support/**/*.{js,ts}", "cypress/plugins/**/*.{js,ts}", "cypress/**/*.spec.{js,ts}"], dev: true },
    Tool { name: "webdriverio", enablers: &["@wdio/cli"], config: &["wdio.conf.{js,ts}"], entry: &["**/*.{e2e,spec,test}.{js,ts}"], dev: true },
    Tool { name: "storybook", enablers: &["storybook", "@storybook/"], config: &[".storybook/{main,manager,preview,test-runner,vitest.setup}.{js,jsx,ts,tsx,mjs,mts}"], entry: &["**/*.@(mdx|stories.@(mdx|js|jsx|mjs|ts|tsx))", ".storybook/**/*.{js,jsx,ts,tsx}"], dev: true },
    Tool { name: "ladle", enablers: &["@ladle/react"], config: &[".ladle/config.mjs"], entry: &["**/*.stories.{js,jsx,ts,tsx,mdx}"], dev: true },
    Tool { name: "msw", enablers: &["msw"], config: E, entry: &["**/mocks/browser.{js,ts}", "**/mocks/server.{js,ts}", "**/mocks/handlers.{js,ts}"], dev: true },
    Tool { name: "next", enablers: &["next"], config: &["next.config.{js,ts,cjs,mjs}"], entry: &[
        "{instrumentation,instrumentation-client,middleware,proxy}.{js,ts}",
        "app/{,[(]*[)]/}{manifest,robots}.{js,ts}", "app/**/sitemap.{js,ts}",
        "app/**/{icon,apple-icon,opengraph-image,twitter-image}.{js,jsx,ts,tsx}",
        "app/global-{error,not-found}.{js,jsx,ts,tsx}",
        "app/**/{default,error,forbidden,loading,not-found,unauthorized}.{js,jsx,ts,tsx}",
        "app/**/{layout,page,route,template}.{js,jsx,ts,tsx}", "pages/**/*.{js,jsx,ts,tsx}",
        "src/{instrumentation,instrumentation-client,middleware,proxy}.{js,ts}",
        "src/app/{,[(]*[)]/}{manifest,robots}.{js,ts}", "src/app/**/sitemap.{js,ts}",
        "src/app/**/{icon,apple-icon,opengraph-image,twitter-image}.{js,jsx,ts,tsx}",
        "src/app/global-{error,not-found}.{js,jsx,ts,tsx}",
        "src/app/**/{default,error,forbidden,loading,not-found,unauthorized}.{js,jsx,ts,tsx}",
        "src/app/**/{layout,page,route,template}.{js,jsx,ts,tsx}", "src/pages/**/*.{js,jsx,ts,tsx}",
    ], dev: false },
    Tool { name: "remix", enablers: &["@remix-run/dev", "@remix-run/react"], config: &["remix.config.js", "vite.config.{js,ts}"], entry: &["app/root.{jsx,tsx}", "app/entry.{client,server}.{js,jsx,ts,tsx}", "app/routes/**/*.{js,jsx,ts,tsx}", "server.{js,ts}", "mocks/**/*.{js,ts}"], dev: false },
    Tool { name: "react-router", enablers: &["@react-router/dev"], config: &["react-router.config.{js,ts}"], entry: &["app/root.{jsx,tsx}", "app/routes.{js,ts}", "app/entry.{client,server}.{js,jsx,ts,tsx}", "app/routes/**/*.{js,jsx,ts,tsx}", "server.{js,ts}"], dev: false },
    Tool { name: "tanstack-router", enablers: &["@tanstack/react-router", "@tanstack/react-start"], config: &["tsr.config.json"], entry: &["src/routes/**/*.{ts,tsx}", "src/routeTree.gen.ts", "src/router.{ts,tsx}", "src/client.{ts,tsx}", "src/server.{ts,tsx}", "app/routes/**/*.{ts,tsx}", "app/routeTree.gen.ts", "app/router.{ts,tsx}", "app/client.{ts,tsx}", "app/ssr.{ts,tsx}"], dev: false },
    Tool { name: "expo", enablers: &["expo"], config: &["app.config.{ts,js}", "app.json", "eas.json"], entry: &["index.{js,jsx,ts,tsx}", "App.{js,jsx,ts,tsx}", "app/**/*.{js,jsx,ts,tsx}", "src/app/**/*.{js,jsx,ts,tsx}", "metro.config.js", "babel.config.js", "expo-env.d.ts"], dev: false },
    Tool { name: "react-native", enablers: &["react-native"], config: &["react-native.config.js"], entry: &["index.{js,jsx,ts,tsx}", "App.{js,jsx,ts,tsx}"], dev: false },
    Tool { name: "metro", enablers: &["metro"], config: &["metro.config.{js,cjs,json}"], entry: E, dev: false },
    Tool { name: "astro", enablers: &["astro"], config: &["astro.config.{js,cjs,mjs,ts}"], entry: &["src/content/config.{js,ts}", "src/content.config.{js,ts}", "src/pages/**/*.{astro,mdx,js,ts}", "src/middleware.{js,ts}", "src/actions/index.{js,ts}", "src/content/**/*.mdx"], dev: false },
    Tool { name: "sveltekit", enablers: &["@sveltejs/kit"], config: &["svelte.config.js"], entry: &["src/hooks.{server,client}.{js,ts}", "src/hooks.{js,ts}", "src/params/*.{js,ts}", "src/routes/**/+*.{js,ts,svelte}", "src/service-worker.{js,ts}", "src/app.html"], dev: false },
    Tool { name: "nuxt", enablers: &["nuxt"], config: &["nuxt.config.{js,mjs,ts}"], entry: &["app.vue", "error.vue", "pages/**/*.vue", "layouts/**/*.vue", "middleware/**/*.{js,ts}", "server/api/**/*.{js,ts}", "server/routes/**/*.{js,ts}", "server/middleware/**/*.{js,ts}", "server/plugins/**/*.{js,ts}", "plugins/**/*.{js,ts}", "composables/**/*.{js,ts}", "utils/**/*.{js,ts}"], dev: false },
    Tool { name: "gatsby", enablers: &["gatsby"], config: &["gatsby-{config,node,browser,ssr}.{js,jsx,ts,tsx}"], entry: &["src/api/**/*.{js,ts}", "src/pages/**/*.{js,jsx,ts,tsx}", "src/templates/**/*.{js,jsx,ts,tsx}", "src/html.{js,jsx,ts,tsx}"], dev: false },
    Tool { name: "docusaurus", enablers: &["@docusaurus/core"], config: &["docusaurus.config.{js,ts}", "sidebars.{js,ts}"], entry: &["src/pages/**/*.{js,jsx,ts,tsx}", "src/theme/**/*.{js,jsx,ts,tsx}"], dev: false },
    Tool { name: "vitepress", enablers: &["vitepress"], config: &[".vitepress/config.{js,ts,mjs,mts}"], entry: &[".vitepress/theme/index.{js,ts,mjs,mts}"], dev: false },
    Tool { name: "nest", enablers: &["@nestjs/core"], config: &["nest-cli.json"], entry: &["src/main.ts"], dev: false },
    Tool { name: "convex", enablers: &["convex"], config: &["convex.json"], entry: &["convex/**/*.{js,ts}"], dev: false },
    Tool { name: "wxt", enablers: &["wxt"], config: &["wxt.config.{js,ts}"], entry: &["entrypoints/**/*.{js,jsx,ts,tsx,html}", "src/entrypoints/**/*.{js,jsx,ts,tsx,html}", "modules/**/*.{js,ts}"], dev: false },
    Tool { name: "sentry", enablers: &["@sentry/"], config: E, entry: &["sentry.{client,server,edge}.config.{js,ts}", "src/sentry.{client,server,edge}.config.{js,ts}"], dev: false },
    Tool { name: "payload", enablers: &["payload"], config: &["payload.config.{js,ts}", "src/payload.config.{js,ts}"], entry: E, dev: false },
    Tool { name: "electron", enablers: &["electron"], config: &["electron-builder.{yml,yaml,json}", "electron.vite.config.{js,ts,mjs,mts}", "forge.config.{js,ts}"], entry: E, dev: false },
    Tool { name: "capacitor", enablers: &["@capacitor/core"], config: &["capacitor.config.{json,ts,js}"], entry: E, dev: false },
    Tool { name: "vite", enablers: &["vite"], config: &["vite.config.{js,mjs,ts,cjs,mts,cts}", "vite.workspace.{js,mjs,ts,cjs,mts,cts,json}"], entry: &["index.html"], dev: false },
    Tool { name: "webpack", enablers: &["webpack"], config: &["webpack.config.{js,ts,mjs,cjs}", "webpack.*.{js,ts,mjs,cjs}"], entry: E, dev: false },
    Tool { name: "rollup", enablers: &["rollup"], config: &["rollup.config.{js,cjs,mjs,ts}"], entry: E, dev: false },
    Tool { name: "rolldown", enablers: &["rolldown"], config: &["rolldown.config.{js,cjs,mjs,ts,mts}"], entry: E, dev: false },
    Tool { name: "rsbuild", enablers: &["@rsbuild/core", "@rspack/core", "@rslib/core"], config: &["rsbuild.config.{js,ts,mjs,mts}", "rspack.config.{js,ts,mjs,mts}", "rslib.config.{js,ts,mjs,mts}"], entry: E, dev: false },
    Tool { name: "tsup", enablers: &["tsup"], config: &["tsup.config.{js,ts,cjs,json}"], entry: E, dev: false },
    Tool { name: "tsdown", enablers: &["tsdown"], config: &["tsdown.config.{js,ts,mjs,mts,json}"], entry: E, dev: false },
    Tool { name: "unbuild", enablers: &["unbuild"], config: &["build.config.{js,cjs,mjs,ts,mts,cts,json}"], entry: E, dev: false },
    Tool { name: "parcel", enablers: &["parcel"], config: &[".parcelrc"], entry: E, dev: false },
    Tool { name: "eslint", enablers: &["eslint"], config: &["eslint.config.{js,cjs,mjs,ts,cts,mts}", ".eslintrc", ".eslintrc.{js,json,cjs,yml,yaml}"], entry: E, dev: true },
    Tool { name: "oxlint", enablers: &["oxlint"], config: &[".oxlintrc.json", "oxlint.config.{js,ts,mjs,mts}"], entry: E, dev: true },
    Tool { name: "biome", enablers: &["@biomejs/biome"], config: &["biome.json", "biome.jsonc"], entry: E, dev: true },
    Tool { name: "prettier", enablers: &["prettier"], config: &[".prettierrc", ".prettierrc.{json,js,cjs,mjs,ts,yml,yaml,toml,json5}", "prettier.config.{js,cjs,mjs,ts,mts,cts}"], entry: E, dev: true },
    Tool { name: "stylelint", enablers: &["stylelint"], config: &[".stylelintrc", ".stylelintrc.{json,yaml,yml,js,cjs,mjs}", "stylelint.config.{js,cjs,mjs}"], entry: E, dev: true },
    Tool { name: "babel", enablers: &["@babel/core"], config: &["babel.config.{json,js,cjs,mjs,cts}", ".babelrc", ".babelrc.{json,js,cjs,mjs,cts}"], entry: E, dev: false },
    Tool { name: "swc", enablers: &["@swc/core"], config: &[".swcrc"], entry: E, dev: false },
    Tool { name: "postcss", enablers: &["postcss"], config: &["postcss.config.{js,cjs,mjs,ts,json}", ".postcssrc", ".postcssrc.{json,js,cjs,mjs,ts}"], entry: E, dev: false },
    Tool { name: "tailwind", enablers: &["tailwindcss"], config: &["tailwind.config.{js,cjs,mjs,ts}"], entry: E, dev: false },
    Tool { name: "unocss", enablers: &["unocss"], config: &["uno.config.{js,ts,mjs,mts}", "unocss.config.{js,ts,mjs,mts}"], entry: E, dev: false },
    Tool { name: "wrangler", enablers: &["wrangler"], config: &["wrangler.{json,jsonc,toml}"], entry: E, dev: false },
    Tool { name: "drizzle", enablers: &["drizzle-kit"], config: &["drizzle.config.{ts,js,mjs,mts,json}"], entry: E, dev: false },
    Tool { name: "prisma", enablers: &["prisma", "@prisma/"], config: &["prisma.config.{ts,js,mjs,cjs,mts,cts}", ".config/prisma.{js,ts,mjs,cjs,mts,cts}", "prisma/schema.prisma", "schema.prisma", "prisma/**/*.prisma"], entry: &["prisma/seed.{js,ts}"], dev: false },
    Tool { name: "knex", enablers: &["knex"], config: &["knexfile.{js,ts,mjs,cjs}"], entry: E, dev: false },
    Tool { name: "graphql-codegen", enablers: &["@graphql-codegen/cli"], config: &["codegen.{ts,js,mjs,cts,json,yml,yaml}", "codegen.config.{ts,js,mjs,cts}", ".graphqlrc", ".graphqlrc.{json,yml,yaml,js,ts}", "graphql.config.{json,yml,yaml,js,ts,cjs,mjs}"], entry: E, dev: false },
    Tool { name: "relay", enablers: &["relay-compiler", "relay-runtime"], config: &["relay.config.{js,json,cjs}"], entry: E, dev: false },
    Tool { name: "i18next-parser", enablers: &["i18next-parser"], config: &["i18next-parser.config.{js,mjs,json,ts}"], entry: E, dev: true },
    Tool { name: "commitlint", enablers: &["@commitlint/cli"], config: &["commitlint.config.{js,cjs,mjs,ts,cts}", ".commitlintrc", ".commitlintrc.{json,yaml,yml,js,cjs,ts,cts}"], entry: E, dev: true },
    Tool { name: "lint-staged", enablers: &["lint-staged"], config: &[".lintstagedrc", ".lintstagedrc.{json,yaml,yml,mjs,js,cjs}", "lint-staged.config.{js,mjs,cjs,ts}"], entry: E, dev: true },
    Tool { name: "husky", enablers: &["husky"], config: &[".husky/*"], entry: E, dev: true },
    Tool { name: "simple-git-hooks", enablers: &["simple-git-hooks"], config: &[".simple-git-hooks.{json,js,cjs}", "simple-git-hooks.{json,js,cjs}"], entry: E, dev: true },
    Tool { name: "lefthook", enablers: &["lefthook"], config: &["lefthook.yml", "lefthook-local.yml", ".lefthook.yml"], entry: E, dev: true },
    Tool { name: "semantic-release", enablers: &["semantic-release"], config: &[".releaserc", ".releaserc.{json,yaml,yml,js,cjs}", "release.config.{js,cjs}"], entry: E, dev: true },
    Tool { name: "release-it", enablers: &["release-it"], config: &[".release-it.{json,js,cjs,ts,yml,yaml,toml}"], entry: E, dev: true },
    Tool { name: "changesets", enablers: &["@changesets/cli"], config: &[".changeset/config.json"], entry: E, dev: true },
    Tool { name: "typedoc", enablers: &["typedoc"], config: &["typedoc.{json,jsonc,js,cjs,mjs}", "typedoc.config.{js,cjs,mjs}"], entry: E, dev: true },
    Tool { name: "cspell", enablers: &["cspell"], config: &["cspell.{json,yaml,yml}", ".cspell.{json,yaml,yml}", "cspell.config.{js,cjs,mjs,json,yaml,yml}"], entry: E, dev: true },
    Tool { name: "markdownlint", enablers: &["markdownlint-cli", "markdownlint-cli2"], config: &[".markdownlint.{json,jsonc,yaml,yml}", ".markdownlint-cli2.{jsonc,yaml,cjs,mjs}"], entry: E, dev: true },
    Tool { name: "nyc", enablers: &["nyc", "c8"], config: &[".nycrc", ".nycrc.{json,yml,yaml}", "nyc.config.js", ".c8rc", ".c8rc.json"], entry: E, dev: true },
    Tool { name: "nodemon", enablers: &["nodemon"], config: &["nodemon.json"], entry: E, dev: true },
    Tool { name: "pm2", enablers: &["pm2"], config: &["ecosystem.config.{js,cjs,json}"], entry: E, dev: false },
    Tool { name: "netlify", enablers: &["@netlify/functions", "netlify-cli"], config: &["netlify.toml"], entry: &["netlify/functions/**/*.{js,mjs,cjs,ts,mts,cts}", "netlify/edge-functions/**/*.{js,mjs,cjs,ts,mts,cts}"], dev: false },
    Tool { name: "vercel", enablers: &["vercel", "@vercel/node"], config: &["vercel.json"], entry: &["api/**/*.{js,mjs,cjs,ts,mts,cts}"], dev: false },
    Tool { name: "sst", enablers: &["sst"], config: &["sst.config.ts"], entry: E, dev: false },
    Tool { name: "hardhat", enablers: &["hardhat"], config: &["hardhat.config.{js,cjs,mjs,ts}"], entry: &["scripts/**/*.{js,ts}", "test/**/*.{js,ts}", "deploy/**/*.{js,ts}"], dev: false },
    Tool { name: "size-limit", enablers: &["size-limit"], config: &[".size-limit.{json,js,cjs,ts}"], entry: E, dev: true },
    Tool { name: "typescript", enablers: &["typescript"], config: &["tsconfig.json", "tsconfig.*.json", "jsconfig.json"], entry: E, dev: false },
    Tool { name: "github-actions", enablers: E, config: &[".github/workflows/*.{yml,yaml}", ".github/**/action.{yml,yaml}"], entry: E, dev: true },
    Tool { name: "moonrepo", enablers: &["@moonrepo/cli"], config: &[".moon/*.yml", "moon.yml"], entry: E, dev: true },
    Tool { name: "nx", enablers: &["nx"], config: &["nx.json", "project.json"], entry: E, dev: true },
    Tool { name: "turbo", enablers: &["turbo"], config: &["turbo.json"], entry: E, dev: true },
    Tool { name: "dependency-cruiser", enablers: &["dependency-cruiser"], config: &[".dependency-cruiser.{js,cjs,mjs,json}"], entry: E, dev: true },
    Tool { name: "syncpack", enablers: &["syncpack"], config: &[".syncpackrc", ".syncpackrc.{json,yaml,yml,js,cjs}", "syncpack.config.{js,cjs,mjs}"], entry: E, dev: true },
    Tool { name: "svgo", enablers: &["svgo"], config: &["svgo.config.{js,cjs,mjs}"], entry: E, dev: false },
    Tool { name: "svgr", enablers: &["@svgr/cli", "@svgr/core"], config: &[".svgrrc", ".svgrrc.{json,yaml,yml,js,cjs}", "svgr.config.{js,cjs}"], entry: E, dev: false },
    Tool { name: "stryker", enablers: &["@stryker-mutator/core"], config: &["stryker.conf.{js,mjs,cjs,json}", "stryker.config.{js,mjs,cjs,json}"], entry: E, dev: true },
    Tool { name: "lost-pixel", enablers: &["lost-pixel"], config: &["lostpixel.config.{js,ts}"], entry: E, dev: true },
    Tool { name: "plop", enablers: &["plop"], config: &["plopfile.{cjs,mjs,js,ts}"], entry: E, dev: true },
    Tool { name: "taskfile", enablers: E, config: &["Taskfile.{yml,yaml}", "taskfile.{yml,yaml}"], entry: E, dev: true },
    Tool { name: "oclif", enablers: &["@oclif/core"], config: &[".oclifrc", "oclif.config.{js,ts}"], entry: &["src/commands/**/*.{js,ts}", "src/hooks/**/*.{js,ts}"], dev: false },
    Tool { name: "eleventy", enablers: &["@11ty/eleventy"], config: &[".eleventy.js", "eleventy.config.{js,cjs,mjs}"], entry: E, dev: false },
    Tool { name: "angular", enablers: &["@angular/core"], config: &["angular.json"], entry: &["src/main.ts", "src/polyfills.ts"], dev: false },
    Tool { name: "karma", enablers: &["karma"], config: &["karma.conf.{js,cjs,mjs,ts}"], entry: E, dev: true },
    Tool { name: "nitro", enablers: &["nitropack"], config: &["nitro.config.{js,ts}"], entry: &["routes/**/*.{js,ts}", "api/**/*.{js,ts}", "middleware/**/*.{js,ts}", "plugins/**/*.{js,ts}"], dev: false },
    Tool { name: "vike", enablers: &["vike"], config: &["pages/**/+config.{js,ts}", "renderer/+config.{js,ts}"], entry: &["pages/**/+*.{js,jsx,ts,tsx}", "renderer/+*.{js,jsx,ts,tsx}"], dev: false },
    Tool { name: "preconstruct", enablers: &["@preconstruct/cli"], config: E, entry: E, dev: false },
    Tool { name: "lockfile-lint", enablers: &["lockfile-lint"], config: &[".lockfile-lintrc", ".lockfile-lintrc.{json,yaml,yml,js,cjs}"], entry: E, dev: true },
    Tool { name: "npm-package-json-lint", enablers: &["npm-package-json-lint"], config: &[".npmpackagejsonlintrc", ".npmpackagejsonlintrc.json", "npmpackagejsonlint.config.js"], entry: E, dev: true },
    Tool { name: "changelogen", enablers: &["changelogen"], config: &["changelog.config.{json,js,cjs,mjs,ts}"], entry: E, dev: true },
    Tool { name: "bumpp", enablers: &["bumpp"], config: &["bump.config.{js,ts,mjs,mts}"], entry: E, dev: true },
    Tool { name: "dotenv", enablers: &["dotenv"], config: E, entry: E, dev: false },
    Tool { name: "cucumber", enablers: &["@cucumber/cucumber"], config: &["cucumber.{js,cjs,mjs,json,yaml,yml}"], entry: &["features/**/*.{js,ts}"], dev: true },
    Tool { name: "rstest", enablers: &["@rstest/core"], config: &["rstest.config.{js,ts,mjs,mts}"], entry: &["**/*.{test,spec}.?(c|m)[jt]s?(x)"], dev: true },
    Tool { name: "react-cosmos", enablers: &["react-cosmos"], config: &["cosmos.config.json"], entry: &["**/*.fixture.{js,jsx,ts,tsx}"], dev: true },
    Tool { name: "danger", enablers: &["danger"], config: &["dangerfile.{js,ts}"], entry: E, dev: true },
    Tool { name: "xo", enablers: &["xo"], config: &["xo.config.{js,cjs,mjs,ts}", ".xo-config", ".xo-config.{js,cjs,json}"], entry: E, dev: true },
    Tool { name: "mdx", enablers: &["@mdx-js/mdx", "@next/mdx"], config: E, entry: &["**/*.mdx"], dev: false },
    Tool { name: "starlight", enablers: &["@astrojs/starlight"], config: E, entry: &["src/content/docs/**/*.{md,mdx}"], dev: false },
    Tool { name: "remark", enablers: &["remark-cli"], config: &[".remarkrc", ".remarkrc.{json,yaml,yml,js,cjs,mjs}"], entry: E, dev: true },
    Tool { name: "execa", enablers: &["execa"], config: E, entry: E, dev: false },
    Tool { name: "zx", enablers: &["zx"], config: E, entry: &["**/*.mjs"], dev: false },
];
