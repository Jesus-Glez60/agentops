use std::sync::LazyLock;

use regex::Regex;

use crate::types::Language;

static PY_IMPORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*(?:import\s+([\w.]+)|from\s+([\w.]+)\s+import)").unwrap());

static JS_IMPORT_FROM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"import\s+(?:[^'"]*\sfrom\s+)?['"]([^'"]+)['"]"#).unwrap());

static JS_REQUIRE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"require\(\s*['"]([^'"]+)['"]\s*\)"#).unwrap());

static GO_SINGLE_IMPORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?m)^\s*import\s+"([^"]+)""#).unwrap());

static GO_BLOCK_IMPORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)import\s*\(([^)]*)\)").unwrap());

static GO_QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([^"]+)""#).unwrap());

static RUST_USE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?use\s+([\w:]+)").unwrap());
static RUST_MOD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;").unwrap());

/// Only the quoted form (`#include "foo.h"`) — conventionally a
/// repo-relative/project header, so a plausible dependency-edge target.
/// The angle-bracket form (`#include <vector>`) is a system/external
/// header, deliberately not extracted here: it can never resolve to a
/// scanned file, and `ranker::rank_files`'s best-effort resolution would
/// just silently fail to match it anyway — same reasoning Go's own import
/// extraction doesn't try to special-case stdlib packages.
static CPP_INCLUDE: LazyLock<Regex> = LazyLock::new(|| Regex::new("(?m)^\\s*#include\\s*\"([^\"]+)\"").unwrap());

static CSHARP_USING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*using\s+(?:static\s+)?([\w.]+)\s*;").unwrap());

/// Extracts raw import/dependency targets from `source` as written (not yet
/// resolved to actual files on disk — resolution against the scanned file
/// set happens in `ranker::rank_files`, on a best-effort basis).
pub fn extract_deps(language: Language, source: &str) -> Vec<String> {
    match language {
        Language::Python => {
            PY_IMPORT.captures_iter(source).filter_map(|c| c.get(1).or_else(|| c.get(2)).map(|m| m.as_str().to_string())).collect()
        }

        Language::TypeScript | Language::JavaScript => {
            let mut deps: Vec<String> = JS_IMPORT_FROM.captures_iter(source).filter_map(|c| c.get(1).map(|m| m.as_str().to_string())).collect();
            deps.extend(JS_REQUIRE.captures_iter(source).filter_map(|c| c.get(1).map(|m| m.as_str().to_string())));
            deps
        }

        Language::Go => {
            let mut deps: Vec<String> = GO_SINGLE_IMPORT.captures_iter(source).filter_map(|c| c.get(1).map(|m| m.as_str().to_string())).collect();
            for block in GO_BLOCK_IMPORT.captures_iter(source) {
                let body = &block[1];
                deps.extend(GO_QUOTED.captures_iter(body).map(|c| c[1].to_string()));
            }
            deps
        }

        Language::Rust => {
            let mut deps: Vec<String> = RUST_USE.captures_iter(source).filter_map(|c| c.get(1).map(|m| m.as_str().to_string())).collect();
            deps.extend(RUST_MOD.captures_iter(source).filter_map(|c| c.get(1).map(|m| m.as_str().to_string())));
            deps
        }

        Language::Cpp => CPP_INCLUDE.captures_iter(source).filter_map(|c| c.get(1).map(|m| m.as_str().to_string())).collect(),

        Language::CSharp => CSHARP_USING.captures_iter(source).filter_map(|c| c.get(1).map(|m| m.as_str().to_string())).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_python_imports() {
        let src = "import os\nfrom collections import OrderedDict\nimport my_pkg.util\n";
        let deps = extract_deps(Language::Python, src);
        assert!(deps.contains(&"os".to_string()));
        assert!(deps.contains(&"collections".to_string()));
        assert!(deps.contains(&"my_pkg.util".to_string()));
    }

    #[test]
    fn extracts_js_imports_and_requires() {
        let src = "import { foo } from './foo';\nconst bar = require('./bar');\n";
        let deps = extract_deps(Language::JavaScript, src);
        assert!(deps.contains(&"./foo".to_string()));
        assert!(deps.contains(&"./bar".to_string()));
    }

    #[test]
    fn extracts_go_single_and_block_imports() {
        let src = "package main\n\nimport \"fmt\"\n\nimport (\n\t\"os\"\n\t\"strings\"\n)\n";
        let deps = extract_deps(Language::Go, src);
        assert!(deps.contains(&"fmt".to_string()));
        assert!(deps.contains(&"os".to_string()));
        assert!(deps.contains(&"strings".to_string()));
    }

    #[test]
    fn extracts_rust_use_and_mod() {
        let src = "use std::collections::HashMap;\nuse crate::graph::GraphStore;\npub use anyhow::Result;\nmod walker;\npub mod ast_extract;\n";
        let deps = extract_deps(Language::Rust, src);
        assert!(deps.contains(&"std::collections::HashMap".to_string()), "found: {deps:?}");
        assert!(deps.contains(&"crate::graph::GraphStore".to_string()), "found: {deps:?}");
        assert!(deps.contains(&"anyhow::Result".to_string()), "found: {deps:?}");
        assert!(deps.contains(&"walker".to_string()), "found: {deps:?}");
        assert!(deps.contains(&"ast_extract".to_string()), "found: {deps:?}");
    }

    #[test]
    fn extracts_cpp_quoted_includes_but_not_angle_bracket_ones() {
        let src = "#include <vector>\n#include \"foo.h\"\n#include \"sub/bar.hpp\"\n";
        let deps = extract_deps(Language::Cpp, src);
        assert!(deps.contains(&"foo.h".to_string()), "found: {deps:?}");
        assert!(deps.contains(&"sub/bar.hpp".to_string()), "found: {deps:?}");
        assert!(!deps.iter().any(|d| d.contains("vector")), "an angle-bracket system include must never be extracted: {deps:?}");
    }

    #[test]
    fn extracts_csharp_using_directives() {
        let src = "using System;\nusing System.Collections.Generic;\nusing static System.Math;\n";
        let deps = extract_deps(Language::CSharp, src);
        assert!(deps.contains(&"System".to_string()), "found: {deps:?}");
        assert!(deps.contains(&"System.Collections.Generic".to_string()), "found: {deps:?}");
        assert!(deps.contains(&"System.Math".to_string()), "found: {deps:?}");
    }
}
