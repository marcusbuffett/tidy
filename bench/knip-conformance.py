#!/usr/bin/env python3
"""Run tidy against knip's fixture-based test suite and compare issue counters.

Parses each `test(...)` block in knip's packages/knip/test/**/*.test.ts (fixture
categories only), extracts the fixture cwd, the createOptions options and the
expected `assert.deepEqual(counters, {...baseCounters, ...})` object, runs
`tidy --reporter json --no-exit-code [flags] <cwd>` and compares the counters tidy
implements (files, dependencies, devDependencies, optionalPeerDependencies,
unlisted, binaries, unresolved, exports, types, enumMembers).

Usage:
  bench/knip-conformance.py [--knip DIR] [--tidy BIN] [--filter SUBSTR] [--json OUT] [-v]

Tests with options tidy cannot take from the CLI are skipped (with the option
name as reason); tests whose shape the regex parser cannot handle are reported
as unparsed rather than guessed. Exit code is always 0.
"""
import argparse
import json
import os
import re
import subprocess
import sys
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor

CATEGORIES = [
    'compilers', 'dependencies', 'entry', 'exports', 'ignore', 'ignore-exports-used-in-file', 'imports',
    'language', 'namespaces', 'plugin-config', 'plugins', 're-exports', 'resolution', 'tags-hints', 'types',
    'workspaces',
]
COMPARED = ['files', 'dependencies', 'devDependencies', 'optionalPeerDependencies', 'unlisted', 'binaries',
            'unresolved', 'exports', 'types', 'enumMembers']
IGNORED = {'processed', 'total', 'nsExports', 'nsTypes', 'namespaceMembers', 'duplicates', 'catalog',
           'catalogReferences', 'cycles', 'classMembers'}
SKIP_OPTIONS = ['isIncludeEntryExports', 'tags', 'isStrict', 'isSession', 'isFix', 'fixTypes', 'args',
                'isUseTscFiles', 'includedIssueTypes', 'excludedIssueTypes', 'config', 'gitignore',
                'cache-location', 'cacheLocation']
SKIP_REASON = {'isIncludeEntryExports': 'includeEntryExports'}

RE_TEST = re.compile(r'^\s*test\((?:\'((?:[^\'\\]|\\.)*)\'|`([^`]*)`)', re.M)
RE_RESOLVE_VAR = re.compile(r'const\s+(\w+)\s*=\s*resolve\(\'([^\']+)\'\)')
RE_CREATE = re.compile(r'createOptions\(\{(.*?)\}\)', re.S)
RE_COUNTERS = re.compile(r'assert\.deepEqual\(counters,\s*(?:\{\s*\.\.\.baseCounters,?(.*?)\}|(baseCounters))\)', re.S)
RE_COUNTER_PAIR = re.compile(r'^\s*(\w+)\s*:\s*(\d+)\s*$')
RE_OPTION_KEY = re.compile(r'(?:^|,)\s*\'?([\w-]+)\'?\s*(?=[:,]|$)')


def iter_tests(path, text):
    """Yield test dicts: name, cwd, options text, expected counters, or parse_error."""
    starts = [(m.start(), m.group(1) or m.group(2)) for m in RE_TEST.finditer(text)]
    for i, (start, name) in enumerate(starts):
        end = starts[i + 1][0] if i + 1 < len(starts) else len(text)
        block = text[start:end]
        creates = list(RE_CREATE.finditer(block))
        expects = list(RE_COUNTERS.finditer(block))
        base = {'file': path, 'name': name}
        if not creates:
            yield {**base, 'parse_error': 'no createOptions/main call'}
            continue
        if len(creates) != len(expects):
            yield {**base, 'parse_error': f'{len(creates)} createOptions vs {len(expects)} counters asserts'}
            continue
        for n, (c, e) in enumerate(zip(creates, expects)):
            t = {**base, 'name': name if n == 0 else f'{name}#{n + 1}'}
            opts = ' '.join(c.group(1).split())
            cwd = resolve_cwd(text, start + c.start(), opts)
            if cwd is None:
                yield {**t, 'parse_error': f'cannot resolve cwd in: {opts}'}
                continue
            body = re.sub(r'//[^\n]*', '', e.group(1) or '')
            pairs = [RE_COUNTER_PAIR.match(x) for x in body.split(',') if x.strip()]
            if not all(pairs):
                yield {**t, 'parse_error': f'counters not literal ints: {" ".join(body.split())}'}
                continue
            expected = {m.group(1): int(m.group(2)) for m in pairs}
            yield {**t, 'cwd': cwd, 'options': opts, 'expected': expected}


def resolve_cwd(text, pos, opts):
    m = re.search(r'cwd\s*:\s*resolve\(\'([^\']+)\'\)', opts)
    if m:
        return m.group(1)
    m = re.search(r'cwd\s*:\s*(\w+)', opts)
    var = m.group(1) if m else 'cwd' if re.search(r'(?:^|[{,]\s*)cwd\s*(?:,|$)', opts) else None
    if var is None:
        return None
    defs = [d for d in RE_RESOLVE_VAR.finditer(text[:pos]) if d.group(1) == var]
    return defs[-1].group(2) if defs else None


def options_to_flags(opts):
    """Return (flags, skip_reason). Keys outside {cwd, isProduction, workspace} cause a skip."""
    keys = [k for k in RE_OPTION_KEY.findall(opts) if k != 'true' and k != 'false']
    for k in keys:
        if k in SKIP_OPTIONS:
            return None, SKIP_REASON.get(k, k)
    flags = []
    if re.search(r'isProduction\s*:\s*true', opts):
        flags.append('--production')
    m = re.search(r'workspace\s*:\s*(\'[^\']*\'|\[[^\]]*\]|\w+)', opts)
    if m:
        v = m.group(1)
        if v.startswith('['):
            return None, 'workspace-array'
        if not v.startswith("'"):
            return None, 'workspace-variable'
        flags += ['--workspace', v.strip("'")]
    for k in keys:
        if k not in ('cwd', 'isProduction', 'workspace'):
            return None, f'unknown-option:{k}'
    if re.search(r'(?:^|[{,])\s*(isProduction|workspace)\s*(?:,|$)', opts):
        return None, 'shorthand-variable'
    return flags, None


def counters_from_json(data):
    c = {k: 0 for k in COMPARED}
    c['files'] = len(data.get('files', []))
    for issue in data.get('issues', []):
        for k in COMPARED[1:-1]:
            c[k] += len(issue.get(k, []))
        c['enumMembers'] += sum(len(v) for v in issue.get('enumMembers', {}).values())
    return c


def run_test(t, tidy, knip_dir):
    cwd = os.path.join(knip_dir, 'packages', 'knip', t['cwd'])
    cmd = [tidy, '--reporter', 'json', '--no-exit-code', *t['flags'], cwd]
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
    except subprocess.TimeoutExpired:
        return {**t, 'status': 'crash', 'stderr': 'timeout after 30s'}
    try:
        actual = counters_from_json(json.loads(p.stdout))
    except json.JSONDecodeError:
        return {**t, 'status': 'crash', 'stderr': (p.stderr or p.stdout)[-600:], 'exit': p.returncode}
    expected = {k: t['expected'].get(k, 0) for k in COMPARED}
    matches = {k: expected[k] == actual[k] for k in COMPARED}
    return {**t, 'status': 'pass' if all(matches.values()) else 'fail', 'actual': actual,
            'expected_full': expected, 'matches': matches, 'cmd': cmd}


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--knip', default=os.path.expanduser('~/.cache/tidy/knip'))
    ap.add_argument('--tidy', default=os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                                                   'target', 'release', 'tidy'))
    ap.add_argument('--filter', default='', help='only tests whose category/file::name contains SUBSTR')
    ap.add_argument('--json', help='write full results as JSON to this path')
    ap.add_argument('-v', '--verbose', action='store_true', help='print every test result')
    a = ap.parse_args()

    test_root = os.path.join(a.knip, 'packages', 'knip', 'test')
    tests = []
    for cat in CATEGORIES:
        for root, _, files in os.walk(os.path.join(test_root, cat)):
            for f in sorted(files):
                if not f.endswith('.test.ts'):
                    continue
                path = os.path.join(root, f)
                rel = os.path.relpath(path, test_root)
                with open(path) as fh:
                    text = fh.read()
                for t in iter_tests(rel, text):
                    t['category'] = cat
                    t['id'] = f"{rel}::{t['name']}"
                    if a.filter in t['id']:
                        tests.append(t)

    runnable, skipped, unparsed = [], [], []
    for t in tests:
        if 'parse_error' in t:
            unparsed.append(t)
            continue
        flags, reason = options_to_flags(t['options'])
        if reason:
            skipped.append({**t, 'skip_reason': reason})
        else:
            runnable.append({**t, 'flags': flags})

    with ThreadPoolExecutor(max_workers=8) as ex:
        results = list(ex.map(lambda t: run_test(t, a.tidy, a.knip), runnable))

    passed = [r for r in results if r['status'] == 'pass']
    failed = [r for r in results if r['status'] == 'fail']
    crashed = [r for r in results if r['status'] == 'crash']
    pct = lambda n, d: f'{100 * n / d:.1f}%' if d else 'n/a'

    print(f'tidy: {a.tidy}\nknip: {a.knip}\n')
    print(f'passed {len(passed)}/{len(results)} runnable ({pct(len(passed), len(results))}), '
          f'failed {len(failed)}, crashed {len(crashed)}')
    print(f'skipped {len(skipped)}: ' + ', '.join(
        f'{k} {v}' for k, v in Counter(t['skip_reason'] for t in skipped).most_common()))
    print(f'unparsed {len(unparsed)}: ' + ', '.join(
        f'{k} {v}' for k, v in Counter(re.sub(r':.*', '', t['parse_error']) for t in unparsed).most_common()))

    print(f'\n{"category":<30} {"runnable":>8} {"passed":>7} {"pct":>7}')
    by_cat = defaultdict(list)
    for r in results:
        by_cat[r['category']].append(r)
    for cat in CATEGORIES:
        rs = by_cat.get(cat, [])
        n_pass = sum(r['status'] == 'pass' for r in rs)
        print(f'{cat:<30} {len(rs):>8} {n_pass:>7} {pct(n_pass, len(rs)):>7}')

    print(f'\n{"issue type":<26} {"relevant":>8} {"matches":>7} {"pct":>7}')
    per_type = {}
    for k in COMPARED:
        rel = [r for r in results if r['status'] != 'crash' and (r['expected_full'][k] or r['actual'][k])]
        ok = sum(r['matches'][k] for r in rel)
        per_type[k] = {'relevant': len(rel), 'matches': ok}
        print(f'{k:<26} {len(rel):>8} {ok:>7} {pct(ok, len(rel)):>7}')

    if failed or crashed:
        print('\nfailing tests:')
    for r in sorted(failed + crashed, key=lambda r: r['id']):
        if r['status'] == 'crash':
            detail = 'CRASH ' + ' '.join(r['stderr'].split())[-200:]
        else:
            detail = '; '.join(f"{k}: want {r['expected_full'][k]} got {r['actual'][k]}"
                               for k in COMPARED if not r['matches'][k])
        flags = (' [' + ' '.join(r['flags']) + ']') if r['flags'] else ''
        print(f"{r['id']}  {r['cwd']}{flags}  {detail}")
    if a.verbose:
        print('\nall results:')
        for r in sorted(results, key=lambda r: r['id']):
            print(f"{r['status']:<5} {r['id']}")
        for t in skipped:
            print(f"skip  {t['id']}  ({t['skip_reason']})")
        for t in unparsed:
            print(f"unpar {t['id']}  ({t['parse_error']})")

    if a.json:
        with open(a.json, 'w') as fh:
            json.dump({'summary': {'runnable': len(results), 'passed': len(passed), 'failed': len(failed),
                                   'crashed': len(crashed), 'skipped': len(skipped), 'unparsed': len(unparsed)},
                       'per_type': per_type, 'results': results, 'skipped': skipped, 'unparsed': unparsed},
                      fh, indent=1)
    return 0


if __name__ == '__main__':
    sys.exit(main())
