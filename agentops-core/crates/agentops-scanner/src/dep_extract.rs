use std::sync::LazyLock;

use regex::Regex;

use crate::types::Language;

static PY_IMPORT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(?:import\s+([\w.]+)|from\s+([\w.]+)\s+import)").unwrap());

static JS_IMPORT_FROM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"import\s+(?:[^'"]*\sfrom\s+)?['"]([^'"]+)['"]"#).unwrap());

static JS_REQUIRE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"require\(\s*['"]([^'"]+)['"]\s*\)"#).unwrap());

static GO_SINGLE_IMPORT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?m)^\s*import\s+"([^"]+)""#).unwrap());

static GO_BLOCK_IMPORT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)import\s*\(([^)]*)\)").unwrap());

static GO_QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([^"]+)""#).unwrap());

/// Extracts raw import/dependency targets from `source` as written (not yet
/// resolved to actual files on disk — resolution against the scanned file set
/// happens in `ranker::rank_files`, on a best-effort basis).
pub fn extract_deps(language: Language, source: &str) -> Vec<String> {
    match language {
        Language::Python => PY_IMPORT
            .captures_iter(source)
            .filter_map(|c| c.get(1).or_else(|| c.get(2)).map(|m| m.as_str().to_string()))
            .collect(),

        Language::TypeScript | Language::JavaScript => {
            let mut deps: Vec<String> = JS_IMPORT_FROM
                .captures_iter(source)
                .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
                .collect();
            deps.extend(
                JS_REQUIRE
                    .captures_iter(source)
                    .filter_map(|c| c.get(1).map(|m| m.as_str().to_string())),
            );
            deps
        }

        Language::Go => {
            let mut deps: Vec<String> = GO_SINGLE_IMPORT
                .captures_iter(source)
                .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
                .collect();
            for block in GO_BLOCK_IMPORT.captures_iter(source) {
                let body = &block[1];
                deps.extend(GO_QUOTED.captures_iter(body).map(|c| c[1].to_string()));
            }
            deps
        }
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
}
