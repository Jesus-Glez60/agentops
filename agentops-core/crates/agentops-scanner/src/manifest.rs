//! Extracts a repo's *declared* dependency versions from its manifest
//! files — distinct from `dep_extract`'s raw-import scanning, which finds
//! *which* packages a repo imports but never a version string. Feeds
//! `docbrain`'s repo-library-version tracking (see
//! `agentops-mcp::sync_docs`), so a "declared 16.2.12, latest indexed 16"
//! mismatch can be shown, not just "this repo uses Next.js."
//!
//! Best-effort and silent: a missing or malformed manifest file
//! contributes zero entries, never a panic or error — most repos won't
//! have every manifest kind, and a manifest this doesn't understand
//! shouldn't block the rest of `sync_docs`.

use std::path::Path;

use serde_json::Value as JsonValue;
use toml::Value as TomlValue;

/// One `(library_name, declared_version)` pair read directly from a
/// manifest — a raw semver string/range as written (`"^16.0.0"`,
/// `"16.2.12"`), not normalized or range-parsed. Comparison against
/// docbrain's indexed versions happens at read time, in the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredDependency {
    pub name: String,
    pub version: String,
}

/// Parses `package.json`'s `dependencies`/`devDependencies` and
/// `Cargo.toml`'s `[dependencies]`, if present, at `root`. Missing files
/// are skipped; a file that fails to parse contributes nothing from that
/// file rather than aborting the whole scan.
pub fn extract_declared_dependencies(root: &Path) -> Vec<DeclaredDependency> {
    let mut deps = parse_package_json(&root.join("package.json"));
    deps.extend(parse_cargo_toml(&root.join("Cargo.toml")));
    deps
}

fn parse_package_json(path: &Path) -> Vec<DeclaredDependency> {
    let Ok(content) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(value) = serde_json::from_str::<JsonValue>(&content) else { return Vec::new() };

    let mut deps = Vec::new();
    for section in ["dependencies", "devDependencies"] {
        let Some(obj) = value.get(section).and_then(JsonValue::as_object) else { continue };
        for (name, version) in obj {
            if let Some(version) = version.as_str() {
                deps.push(DeclaredDependency { name: name.clone(), version: version.to_string() });
            }
        }
    }
    deps
}

fn parse_cargo_toml(path: &Path) -> Vec<DeclaredDependency> {
    let Ok(content) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(table) = content.parse::<toml::Table>() else { return Vec::new() };
    let Some(deps_table) = table.get("dependencies").and_then(TomlValue::as_table) else { return Vec::new() };

    let mut deps = Vec::new();
    for (name, value) in deps_table {
        // Bare-string form (`serde = "1"`) or table form with a `version`
        // key (`serde = { version = "1", features = [...] }`); a
        // path/git-only dependency has no `version` key and is skipped —
        // nothing to compare against docbrain's indexed versions.
        let version = value.as_str().or_else(|| value.as_table().and_then(|t| t.get("version")).and_then(TomlValue::as_str));
        if let Some(version) = version {
            deps.push(DeclaredDependency { name: name.clone(), version: version.to_string() });
        }
    }
    deps
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn missing_manifests_yield_no_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        assert!(extract_declared_dependencies(dir.path()).is_empty());
    }

    #[test]
    fn parses_package_json_dependencies_and_dev_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), r#"{"dependencies": {"next": "16.2.12"}, "devDependencies": {"typescript": "^5.0.0"}}"#).unwrap();

        let deps = extract_declared_dependencies(dir.path());
        assert!(deps.contains(&DeclaredDependency { name: "next".to_string(), version: "16.2.12".to_string() }));
        assert!(deps.contains(&DeclaredDependency { name: "typescript".to_string(), version: "^5.0.0".to_string() }));
    }

    #[test]
    fn malformed_package_json_yields_no_dependencies_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{ not valid json").unwrap();
        assert!(extract_declared_dependencies(dir.path()).is_empty());
    }

    #[test]
    fn parses_cargo_toml_bare_string_and_table_forms() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            r#"
[dependencies]
serde = "1"
tokio = { version = "1.40", features = ["full"] }
local-crate = { path = "../local-crate" }
"#,
        )
        .unwrap();

        let deps = extract_declared_dependencies(dir.path());
        assert!(deps.contains(&DeclaredDependency { name: "serde".to_string(), version: "1".to_string() }));
        assert!(deps.contains(&DeclaredDependency { name: "tokio".to_string(), version: "1.40".to_string() }));
        assert!(!deps.iter().any(|d| d.name == "local-crate"), "path-only dependencies have no version to compare");
    }

    #[test]
    fn both_manifests_present_are_combined() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), r#"{"dependencies": {"next": "16.0.0"}}"#).unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[dependencies]\nserde = \"1\"\n").unwrap();

        let deps = extract_declared_dependencies(dir.path());
        assert_eq!(deps.len(), 2);
    }
}
