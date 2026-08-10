//! Classifies a raw import string into "is this a real third-party
//! package, and if so which registry/package name" — unchanged in logic
//! from `main` (no bugs were found here in the rebuild audit), reimplemented
//! rather than copy-pasted per the clean-rebuild instruction.
//!
//! Deliberately takes a plain `&str` language tag rather than depending on
//! `agentops-scanner`'s `Language` type — this crate has no dependency on
//! `agentops-core`, keeping the two products independently versionable.

use crate::discover::Ecosystem;

/// Common Python stdlib module names — a relative import or one of these is
/// never a PyPI package. Not exhaustive; false negatives here just mean an
/// occasional stdlib module gets (harmlessly) looked up and reported "not
/// found" rather than skipped outright. Kept as inline data for now — the
/// audit's suggestion to move this to a versioned external file is a real
/// improvement but not required for correctness, and is deferred to avoid
/// scope creep on this pass.
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

/// Classifies one raw dependency string. Returns `None` for relative
/// imports, stdlib/builtin modules, or an unrecognized language.
pub fn classify_dependency(language: &str, raw: &str) -> Option<(Ecosystem, String)> {
    match language {
        "python" => classify_python(raw),
        "javascript" | "typescript" => classify_js(raw),
        _ => None,
    }
}

fn classify_python(raw: &str) -> Option<(Ecosystem, String)> {
    if raw.starts_with('.') {
        return None;
    }
    let top_level = raw.split('.').next().unwrap_or(raw);
    if PY_STDLIB.contains(&top_level) {
        return None;
    }
    Some((Ecosystem::PyPi, top_level.to_string()))
}

fn classify_js(raw: &str) -> Option<(Ecosystem, String)> {
    if raw.starts_with('.') || raw.starts_with('/') {
        return None;
    }
    if NODE_BUILTINS.contains(&raw) || raw.starts_with("node:") {
        return None;
    }

    let package_name = if let Some(rest) = raw.strip_prefix('@') {
        let mut parts = rest.splitn(2, '/');
        let scope = parts.next().unwrap_or(rest);
        match parts.next() {
            Some(name_and_rest) => {
                let name = name_and_rest.split('/').next().unwrap_or(name_and_rest);
                format!("@{scope}/{name}")
            }
            None => return None,
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
        assert_eq!(classify_dependency("javascript", "../lib/thing"), None);
    }

    #[test]
    fn skips_stdlib_and_builtins() {
        assert_eq!(classify_dependency("python", "os"), None);
        assert_eq!(classify_dependency("javascript", "node:fs"), None);
    }

    #[test]
    fn classifies_real_pypi_and_npm_packages() {
        assert_eq!(classify_dependency("python", "django.db"), Some((Ecosystem::PyPi, "django".to_string())));
        assert_eq!(classify_dependency("typescript", "react/jsx-runtime"), Some((Ecosystem::Npm, "react".to_string())));
    }

    #[test]
    fn classifies_scoped_npm_packages() {
        assert_eq!(
            classify_dependency("typescript", "@radix-ui/react-dialog/dist/index"),
            Some((Ecosystem::Npm, "@radix-ui/react-dialog".to_string()))
        );
    }
}
