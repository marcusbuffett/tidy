#!/usr/bin/env python3
"""Generate src/tools.rs (the tool-convention table) from knip's plugin sources.

usage: scripts/gen-tools.py [path/to/knip/checkout]   (default ~/.cache/tidy/knip)

Each knip plugin declares `enablers`, `config`, `entry` and `production` patterns; this script
extracts those statically (resolving the toCosmiconfig/toLilconfig/... helpers and spreads) and
writes them as a Rust table. Plugins whose patterns are computed at runtime are listed in OVERRIDES.
"""
import os, re, sys, json

KNIP = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser('~/.cache/tidy/knip')
PLUGINS = os.path.join(KNIP, 'packages/knip/src/plugins')
OUT = os.path.join(os.path.dirname(__file__), '..', 'src', 'tools.rs')

JS_EXTS = ['js', 'ts', 'cjs', 'mjs', 'cts', 'mts']
HELPERS = {
    'toCosmiconfig': (['json', 'yaml', 'yml', 'js', 'ts', 'cjs', 'mjs'], {'configDir': True}),
    'toLilconfig': (['json', 'ts', 'js', 'cjs', 'mjs'], {'configDir': True}),
    'toUnconfig': (['json', 'ts', 'mts', 'cts', 'js', 'mjs', 'cjs'], {'configDir': False, 'rcPrefix': '', 'rcSuffix': '', 'configFiles': False}),
    'toC12config': (['json', 'jsonc', 'json5', 'yaml', 'yml', 'js', 'ts', 'mjs', 'cjs', 'mts', 'cts', 'toml'], {'configDir': True}),
}
HOOKS = ['prepare-commit-msg', 'commit-msg', 'pre-{applypatch,commit,merge-commit,push,rebase,receive}', 'post-{checkout,commit,merge,rewrite}']

# Patterns knip computes at runtime (joins, maps, helper modules); values mirror knip's output.
OVERRIDES = {
    'metro': {'production': ['src/**/*.{android,ios,native,web}.{js,jsx,ts,tsx}']},
    'nuxt': {'production': ['app.{vue,jsx,tsx}', 'error.{vue,jsx,tsx}', 'router.options.ts', 'layouts/**/*.{vue,jsx,tsx}', 'middleware/**/*.{js,ts}', 'pages/**/*.{vue,jsx,tsx,js,ts}', 'plugins/**/*.{js,ts}', 'server/api/**/*.ts', 'server/middleware/**/*.ts', 'server/plugins/**/*.ts', 'server/routes/**/*.ts', 'server/tasks/**/*.ts']},
    'vike': {'production': ['{pages,renderer}/**/+*.{js,jsx,ts,tsx,vue,react,solid}']},
    'vite-plugin-pwa': {'production': ['src/sw.ts']},
    'expressive-code': {'enablers': ['astro-expressive-code', 'rehype-expressive-code', '@astrojs/starlight']},
}

def config_map(helper, name, opts):
    exts, builder = HELPERS[helper]
    cfg = {'rcPrefix': '.', 'rcSuffix': 'rc', 'configDir': True, 'configFiles': True, 'configFilesAllExtensions': False, 'additionalExtensions': []}
    cfg.update(builder); cfg.update(opts)
    exts = exts + cfg['additionalExtensions']
    p, s = cfg['rcPrefix'], cfg['rcSuffix']
    out = [f'{p}{name}{s}'] + ([f'.config/{name}{s}'] if cfg['configDir'] else [])
    out.append(f'{p}{name}{s}.{{{",".join(exts)}}}')
    cexts = [e for e in exts if cfg['configFilesAllExtensions'] or e in JS_EXTS]
    if cfg['configFiles']:
        out.append(f'{name}.config.{{{",".join(cexts)}}}')
    if cfg['configDir']:
        out.append(f'.config/{name}{s}.{{{",".join(exts)}}}')
    return out

def parse_opts(text):
    opts = {}
    for k, v in re.findall(r'(\w+):\s*(true|false|\'[^\']*\'|\[[^\]]*\])', text):
        if v in ('true', 'false'): opts[k] = v == 'true'
        elif v.startswith('['): opts[k] = re.findall(r"'([^']*)'", v)
        else: opts[k] = v.strip("'")
    return opts

def parse_array(body, consts, imports):
    items, bad = [], False
    i = 0
    token = re.compile(r"'((?:[^'\\]|\\.)*)'|\"((?:[^\"\\]|\\.)*)\"|/((?:[^/\\]|\\.)+)/[a-z]*|\.\.\.(\w+)\b(?!\()|(?:\.\.\.)?(\w+)\(")
    while True:
        m = token.search(body, i)
        if not m: break
        i = m.end()
        if m.group(1) is not None or m.group(2) is not None:
            items.append(m.group(1) if m.group(1) is not None else m.group(2))
        elif m.group(3):
            items.append('/' + m.group(3) + '/')
        elif m.group(4):
            ref = m.group(4)
            if ref in consts: items.extend(consts[ref])
            elif ref in imports: items.extend(imports[ref])
            else: bad = True
        elif m.group(5):
            fn = m.group(5)
            depth, j = 1, i
            while depth and j < len(body):
                depth += {'(': 1, ')': -1}.get(body[j], 0); j += 1
            args = body[i:j - 1]; i = j
            if fn in HELPERS:
                name = re.match(r"\s*'([^']+)'", args)
                if not name: bad = True; continue
                items.extend(config_map(fn, name.group(1), parse_opts(args[name.end():])))
            else:
                bad = True
    return items, bad

def enabler(e):
    if not e.startswith('/'): return e
    r = e[1:-1].lstrip('^').replace('\\/', '/')
    r = re.sub(r'(\.\*|\$)+$', '', r)
    return r

def plugin(name, cache):
    if name in cache: return cache[name]
    try: src = open(f'{PLUGINS}/{name}/index.ts').read()
    except FileNotFoundError: return None
    imports = {}
    for m in re.finditer(r"import \{([^}]*)\} from '\.\./(\w[\w-]*)/index\.ts'", src):
        dep = plugin(m.group(2), cache)
        for part in m.group(1).split(','):
            part = part.strip()
            if not part: continue
            orig, alias = (part.split(' as ') + [None])[:2]
            orig = orig.strip(); alias = (alias or orig).strip()
            if dep and orig in dep['arrays']: imports[alias] = dep['arrays'][orig]
    imports['gitHookPaths'] = [f'.husky/{h}' for h in HOOKS]
    consts, bad = {}, set()
    for m in re.finditer(r'^(?:export )?const (\w+)(?::\s*[\w\[\]<>| ]+)?\s*=\s*\[(.*?)\];', src, re.S | re.M):
        items, b = parse_array(m.group(2), consts, imports)
        consts[m.group(1)] = items
        if b: bad.add(m.group(1))
    rec = {'arrays': consts, 'bad': bad, 'simple': 'hasDependency(dependencies, enablers);' in src}
    cache[name] = rec
    return rec

def main():
    # Patterns knip builds at runtime (template strings, helper modules) and tools knip folds into
    # other plugins; the values mirror knip's output and are applied where extraction found nothing.
    fallback = json.load(open(os.path.join(os.path.dirname(__file__), 'tools-fallback.json')))
    cache = {}
    names = sorted(d for d in os.listdir(PLUGINS) if os.path.isdir(os.path.join(PLUGINS, d)))
    rows = []
    for n in names:
        rec = plugin(n, cache)
        if rec is None: continue
        a = rec['arrays']
        get = lambda k: [x for x in a.get(k, []) if x != 'package.json']
        fields = {'enablers': [enabler(e) for e in a.get('enablers', [])], 'config': get('config'), 'entry': get('entry'), 'production': get('production')}
        for k, v in OVERRIDES.get(n, {}).items(): fields[k] = v
        fb = fallback.get(n, {})
        if not (fields['entry'] or fields['production']):
            fields['entry'], fields['production'] = fb.get('entry', []), fb.get('production', [])
        if not fields['config'] and fb.get('config'):
            fields['config'] = fb['config']
        for k in list(fields):
            if k in rec['bad'] and k not in OVERRIDES.get(n, {}):
                print(f'warning: {n}.{k} has runtime-computed items; emitting the static part', file=sys.stderr)
        if n == 'size-limit':  # knip lists its lilconfig files under `entry`
            fields['config'] += fields['entry']; fields['entry'] = []
        rows.append((n, fields))
    for n, fb in fallback.items():
        if n not in dict(rows):
            rows.append((n, {k: fb.get(k, []) for k in ['enablers', 'config', 'entry', 'production']}))
    rows.sort()
    def arr(v): return '&[' + ', '.join('"' + x.replace('"', '\\"') + '"' for x in v) + ']' if v else 'E'
    lines = ['//! Generated by scripts/gen-tools.py from knip\'s plugin sources; do not edit by hand.', '//! Each row mirrors a knip plugin: `enablers` are dependency names (a trailing `/` matches a scope),', '//! `config` files are parsed for references, `entry`/`production` are entry-file patterns.', '', 'use crate::patterns::Tool;', '', 'const E: &[&str] = &[];', '', 'pub static TOOLS: &[Tool] = &[']
    for n, f in rows:
        lines.append(f'    Tool {{ name: "{n}", enablers: {arr(f["enablers"])}, config: {arr(f["config"])}, entry: {arr(f["entry"])}, production: {arr(f["production"])} }},')
    lines.append('];')
    open(OUT, 'w').write('\n'.join(lines) + '\n')
    print(f'wrote {len(rows)} tools to {os.path.relpath(OUT)}')

main()
