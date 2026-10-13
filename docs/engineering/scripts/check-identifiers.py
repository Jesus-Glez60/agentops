#!/usr/bin/env python3
"""Checks that code-like identifiers in backticks exist in the repo.

Usage: docs/engineering/scripts/check-identifiers.py FILE.md...
Searches tracked files with `git grep -F` (docs/engineering excluded) and
paths with os.path.exists. Prints misses; exits 1 if any.
"""
import os, re, subprocess, sys

def tokens(paths):
    out = set()
    for f in paths:
        for t in re.findall(r'`([^`\n]+)`', open(f).read()):
            t = re.split(r'[ (=,]', t.strip())[0].rstrip(':;.')
            if len(t) < 3 or re.fullmatch(r'[\d.]+', t):
                continue
            if not re.search(r'[_:/.\-]|[a-z][A-Z]', t):
                continue
            if t.startswith(('http', '~', '$', '<', 'docs/engineering', '02-crates', 'survey/', 'scripts/')):
                continue
            out.add(t)
    return out

FILES = subprocess.run(['git', 'ls-files'], capture_output=True, text=True).stdout.split('\n')
OWN_DOCS = set(os.listdir(os.path.join(os.path.dirname(__file__), '..')))

def present(t):
    if t in OWN_DOCS:
        return True
    t = t.rstrip('*')  # wildcard families: CAP_*, candle-*, session_events*
    t = re.sub(r'\{[^}]*\}.*$', '', t)  # templates: proj_head_v{N}.safetensors
    # crate-relative file paths (src/walker.rs, tools.rs): match a tracked path suffix
    if re.search(r'\.(rs|toml|md|ya?ml|sql|ts|tsx|json|sh|mjs|py|astro)$', t):
        if any(f == t or f.endswith('/' + t) for f in FILES):
            return True
    cands = [t]
    # Struct.field / table.column: the field or column name must exist
    if re.fullmatch(r'[A-Za-z_][\w]*\.[a-z_]\w*', t):
        cands.append(t.split('.')[-1])
    if '::' in t:
        # a::b::C counts only if every segment exists somewhere
        return all(grep(x) for x in t.split('::') if len(x) > 2)
    path = t.split(':')[0]  # strip a trailing :line
    if '/' in path or re.search(r'\.\w+$', path):
        if os.path.exists(path):
            return True
        cands.append(path.split('/')[-1])
    return any(grep(c) for c in cands)

def grep(c):
    r = subprocess.run(['git', 'grep', '-qF', '-e', c, '--', '.', ':!docs/engineering'], capture_output=True)
    return r.returncode == 0

toks = tokens(sys.argv[1:])
miss = sorted(t for t in toks if not present(t))
print(f'{len(toks)} identifiers checked, {len(miss)} not found')
for m in miss:
    print('  MISS', m)
sys.exit(1 if miss else 0)
