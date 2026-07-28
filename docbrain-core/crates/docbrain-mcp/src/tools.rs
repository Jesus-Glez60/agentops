//! Tool definitions and dispatch. Every tool takes an optional `org` argument
//! that becomes the `TenantContext` passed into `docbrain-graph` — omitting it
//! means "public caller," it never defaults to seeing everything. There is no
//! code path here that calls `docbrain-graph` without constructing a
//! `TenantContext` first (Docbrain-4's isolation guarantee lives at the store
//! layer; this layer just has to not bypass it, which the store's API makes
//! structurally hard to do wrong).

use docbrain_graph::{DocbrainStore, TenantContext, Visibility};
use docbrain_ingest::{discover, Ecosystem};
use serde_json::{json, Value};

use crate::protocol::{CallToolResult, ToolDefinition};

pub fn list_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "list_libraries",
            description: "List libraries visible to the caller (public + its own private ones, if 'org' is given).",
            input_schema: json!({
                "type": "object",
                "properties": { "org": { "type": "string", "description": "Optional org id for private-library access." } },
            }),
        },
        ToolDefinition {
            name: "get_library",
            description: "Get metadata for a library by slug.",
            input_schema: json!({
                "type": "object",
                "properties": { "slug": { "type": "string" }, "org": { "type": "string" } },
                "required": ["slug"],
            }),
        },
        ToolDefinition {
            name: "get_docs",
            description: "Get doc nodes for a library at an EXACT version (no nearest-version substitution).",
            input_schema: json!({
                "type": "object",
                "properties": { "slug": { "type": "string" }, "version": { "type": "string" }, "org": { "type": "string" } },
                "required": ["slug", "version"],
            }),
        },
        ToolDefinition {
            name: "get_changelog",
            description: "Get changelog entries between two exact versions.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "from_version": { "type": "string" },
                    "to_version": { "type": "string" },
                    "org": { "type": "string" },
                },
                "required": ["slug", "from_version", "to_version"],
            }),
        },
        ToolDefinition {
            name: "discover_library",
            description: "Docbrain-3: on-demand discovery. Looks up a package's docs/repo URLs via its registry (npm or pypi) and registers it. Discovery-order step (1) only — falls through to Ok(not found) if the registry has no such package, for the caller to try web search or ask the user next.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "ecosystem": { "type": "string", "enum": ["npm", "pypi"] },
                    "name": { "type": "string" },
                    "org": { "type": "string", "description": "If given, registers the library as private to this org instead of public." },
                },
                "required": ["ecosystem", "name"],
            }),
        },
    ]
}

fn tenant_from_args(args: &Value) -> TenantContext {
    match args.get("org").and_then(|v| v.as_str()) {
        Some(org) => TenantContext::org(org),
        None => TenantContext::public(),
    }
}

fn get_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

pub fn call_tool(store: &DocbrainStore, name: &str, args: &Value) -> Result<CallToolResult, String> {
    let result = match name {
        "list_libraries" => tool_list_libraries(store, args),
        "get_library" => tool_get_library(store, args),
        "get_docs" => tool_get_docs(store, args),
        "get_changelog" => tool_get_changelog(store, args),
        "discover_library" => tool_discover_library(store, args),
        other => return Err(format!("unknown tool '{other}'")),
    };

    Ok(match result {
        Ok(text) => CallToolResult::success(text),
        Err(e) => CallToolResult::error(e.to_string()),
    })
}

fn tool_list_libraries(store: &DocbrainStore, args: &Value) -> anyhow::Result<String> {
    let tenant = tenant_from_args(args);
    let libs = store.list_libraries(&tenant)?;
    if libs.is_empty() {
        return Ok("No libraries visible.".to_string());
    }
    Ok(libs.iter().map(|l| format!("- {} ({})", l.slug, l.name)).collect::<Vec<_>>().join("\n"))
}

fn tool_get_library(store: &DocbrainStore, args: &Value) -> anyhow::Result<String> {
    let tenant = tenant_from_args(args);
    let slug = get_str(args, "slug").ok_or_else(|| anyhow::anyhow!("missing required 'slug'"))?;
    match store.get_library(&tenant, slug)? {
        Some(l) => Ok(format!(
            "{} ({})\nrepo: {}\ndocs: {}\nvisibility: {:?}",
            l.slug,
            l.name,
            l.github_repo.as_deref().unwrap_or("-"),
            l.docs_url.as_deref().unwrap_or("-"),
            l.visibility
        )),
        None => anyhow::bail!("no library '{slug}' visible to this caller"),
    }
}

fn tool_get_docs(store: &DocbrainStore, args: &Value) -> anyhow::Result<String> {
    let tenant = tenant_from_args(args);
    let slug = get_str(args, "slug").ok_or_else(|| anyhow::anyhow!("missing required 'slug'"))?;
    let version = get_str(args, "version").ok_or_else(|| anyhow::anyhow!("missing required 'version'"))?;
    let nodes = store.get_doc_nodes(&tenant, slug, version)?;
    if nodes.is_empty() {
        return Ok(format!("No docs found for {slug}@{version}. Available versions: {:?}", store.list_doc_versions(&tenant, slug)?));
    }
    Ok(nodes.iter().map(|n| format!("## {}\n{}", n.topic, n.content)).collect::<Vec<_>>().join("\n\n"))
}

fn tool_get_changelog(store: &DocbrainStore, args: &Value) -> anyhow::Result<String> {
    let tenant = tenant_from_args(args);
    let slug = get_str(args, "slug").ok_or_else(|| anyhow::anyhow!("missing required 'slug'"))?;
    let from = get_str(args, "from_version").ok_or_else(|| anyhow::anyhow!("missing required 'from_version'"))?;
    let to = get_str(args, "to_version").ok_or_else(|| anyhow::anyhow!("missing required 'to_version'"))?;
    let entries = store.get_changelog(&tenant, slug, from, to)?;
    if entries.is_empty() {
        return Ok(format!("No changelog entries recorded between {from} and {to} for {slug}."));
    }
    Ok(entries
        .iter()
        .map(|e| format!("{}{}", if e.breaking { "[BREAKING] " } else { "" }, e.entry))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn tool_discover_library(store: &DocbrainStore, args: &Value) -> anyhow::Result<String> {
    let tenant = tenant_from_args(args);
    let ecosystem_str = get_str(args, "ecosystem").ok_or_else(|| anyhow::anyhow!("missing required 'ecosystem'"))?;
    let name = get_str(args, "name").ok_or_else(|| anyhow::anyhow!("missing required 'name'"))?;
    let ecosystem = match ecosystem_str {
        "npm" => Ecosystem::Npm,
        "pypi" => Ecosystem::PyPi,
        other => anyhow::bail!("invalid ecosystem '{other}', expected 'npm' or 'pypi'"),
    };

    let visibility = match &tenant {
        TenantContext::Org(id) => Visibility::Private(id.clone()),
        TenantContext::Public => Visibility::Public,
    };

    match discover(ecosystem, name)? {
        Some(found) => {
            store.add_library(&tenant, name, name, found.repo_url.as_deref(), found.docs_url.as_deref(), visibility)?;
            Ok(format!(
                "Discovered and registered '{name}'. docs: {} repo: {}",
                found.docs_url.as_deref().unwrap_or("(none published)"),
                found.repo_url.as_deref().unwrap_or("(none published)"),
            ))
        }
        None => Ok(format!(
            "'{name}' not found on {ecosystem_str}. Discovery-order step (1) exhausted — try a web search or ask the user for a docs URL next."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_tools_has_five_entries() {
        assert_eq!(list_tools().len(), 5);
    }

    #[test]
    fn get_library_respects_tenant_isolation() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let acme = TenantContext::org("acme");
        store.add_library(&acme, "acme-sdk", "Acme SDK", None, None, Visibility::Private("acme".into())).unwrap();

        let result = call_tool(&store, "get_library", &json!({"slug": "acme-sdk", "org": "globex"})).unwrap();
        assert!(result.is_error, "globex must not see acme's private library");
    }

    #[test]
    fn discover_library_registers_a_real_npm_package() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let result = call_tool(&store, "discover_library", &json!({"ecosystem": "npm", "name": "react"})).unwrap();
        assert!(!result.is_error, "{:?}", result.content);
        assert!(result.content[0].text.contains("Discovered and registered"));

        let lib = store.get_library(&TenantContext::public(), "react").unwrap();
        assert!(lib.is_some());
    }
}
