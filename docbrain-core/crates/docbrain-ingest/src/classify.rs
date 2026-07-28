//! Classifies a raw import string (as extracted by `agentops-scanner`'s
//! `dep_extract`) into "is this a real third-party package, and if so which
//! registry/package name" — the missing link between a scanned repo's
//! dependency list and Docbrain-3's "diff detected dependencies against
//! docbrain, auto-discover what's missing."
//!
//! Deliberately takes a plain `&str` language tag (`"python"`, `"javascript"`,
//! `"typescript"`) rather than importing `agentops_scanner::Language` — this
//! crate has no dependency on `agentops-core`, keeping the two products
//! independently versionable per the plan's crate-boundary design. The
//! orchestration that actually calls both (`agentops-cli`) owns the glue.

use crate::Ecosystem;

/// Common Python stdlib module names — a relative import or one of these is
/// never a PyPI package. Not exhaustive (the full stdlib is ~200+ modules);
/// covers what's overwhelmingly common in real code, false negatives here
/// just mean an occasional stdlib module gets (harmlessly) treated as
/// "not found on PyPI" rather than skipped outright.
const PY_STDLIB: &[&str] = &[
    "os", "sys", "json", "re", "math", "random", "itertools", "functools", "collections", "typing",
    "pathlib", "subprocess", "threading", "asyncio", "logging", "unittest", "datetime", "time", "io",
    "abc", "dataclasses", "enum", "copy", "shutil", "socket", "struct", "hashlib", "base64", "uuid",
    "argparse", "csv", "sqlite3", "http", "urllib", "xml", "email", "string", "textwrap", "traceback",
    "warnings", "weakref", "contextlib", "importlib", "inspect", "pickle", "queue", "signal", "tempfile",
    "glob", "fnmatch", "platform", "getpass", "configparser", "dis", "ast", "keyword", "token", "tokenize",
];

/// Common Node.js builtin module names — never an npm package.
const NODE_BUILTINS: &[&str] = &[
    "fs", "path", "http", "https", "os", "crypto", "stream", "events", "util", "url", "querystring",
    "child_process", "cluster", "net", "tls", "dns", "zlib", "buffer", "assert", "process", "readline",
    "timers", "vm", "worker_threads", "perf_hooks", "module", "async_hooks",
];

/// Classifies one raw dependency string. Returns `None` for relative imports,
/// stdlib/builtin modules, or an unrecognized language — all of which mean
/// "not something to look up in a package registry."
pub fn classify_dependency(language: &str, raw: &str) -> Option<(Ecosystem, String)> {
    match language {
        "python" => classify_python(raw),
        "javascript" | "typescript" => classify_js(raw),
        _ => None,
    }
}

fn classify_python(raw: &str) -> Option<(Ecosystem, String)> {
    if raw.starts_with('.') {
        return None; // relative import
    }
    let top_level = raw.split('.').next().unwrap_or(raw);
    if PY_STDLIB.contains(&top_level) {
        return None;
    }
    Some((Ecosystem::PyPi, top_level.to_string()))
}

fn classify_js(raw: &str) -> Option<(Ecosystem, String)> {
    if raw.starts_with('.') || raw.starts_with('/') {
        return None; // relative or absolute local path
    }
    if NODE_BUILTINS.contains(&raw) || raw.starts_with("node:") {
        return None;
    }

    // Scoped packages ("@scope/name/sub/path") -> npm package is "@scope/name".
    // Unscoped ("name/sub/path") -> npm package is "name".
    let package_name = if let Some(rest) = raw.strip_prefix('@') {
        let mut parts = rest.splitn(2, '/');
        let scope = parts.next().unwrap_or(rest);
        match parts.next() {
            Some(name_and_rest) => {
                let name = name_and_rest.split('/').next().unwrap_or(name_and_rest);
                format!("@{scope}/{name}")
            }
            None => return None, // "@scope" with no package name — malformed
        }
    } else {
        raw.split('/').next().unwrap_or(raw).to_string()
    };

    Some((Ecosystem::Npm, package_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_relative_and_local_imports() {
        assert_eq!(classify_dependency("python", "./util"), None);
        assert_eq!(classify_dependency("python", ".relative"), None);
        assert_eq!(classify_dependency("javascript", "./util"), None);
        assert_eq!(classify_dependency("javascript", "../lib/thing"), None);
    }

    #[test]
    fn skips_stdlib_and_builtins() {
        assert_eq!(classify_dependency("python", "os"), None);
        assert_eq!(classify_dependency("python", "os.path"), None);
        assert_eq!(classify_dependency("javascript", "fs"), None);
        assert_eq!(classify_dependency("javascript", "node:fs"), None);
    }

    #[test]
    fn classifies_real_pypi_and_npm_packages() {
        assert_eq!(classify_dependency("python", "requests"), Some((Ecosystem::PyPi, "requests".to_string())));
        assert_eq!(classify_dependency("python", "django.db"), Some((Ecosystem::PyPi, "django".to_string())));
        assert_eq!(classify_dependency("javascript", "express"), Some((Ecosystem::Npm, "express".to_string())));
        assert_eq!(classify_dependency("typescript", "react/jsx-runtime"), Some((Ecosystem::Npm, "react".to_string())));
    }

    #[test]
    fn classifies_scoped_npm_packages() {
        assert_eq!(classify_dependency("javascript", "@babel/core"), Some((Ecosystem::Npm, "@babel/core".to_string())));
        assert_eq!(
            classify_dependency("typescript", "@radix-ui/react-dialog/dist/index"),
            Some((Ecosystem::Npm, "@radix-ui/react-dialog".to_string()))
        );
    }
}
