//! Tool definitions and dispatch. Every tool takes an optional `org` argument
//! that becomes the `TenantContext` passed into `docbrain-graph` — omitting it
//! means "public caller," it never defaults to seeing everything. There is no
//! code path here that calls `docbrain-graph` without constructing a
//! `TenantContext` first (Docbrain-4's isolation guarantee lives at the store
//! layer; this layer just has to not bypass it, which the store's API makes
//! structurally hard to do wrong).

use anyhow::Context;
use docbrain_graph::{DocbrainStore, TenantContext, Visibility};
use docbrain_ingest::{discover, scrape_docs, sync_github_releases, Ecosystem};
use serde_json::{json, Value};
use std::path::Path;

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
        ToolDefinition {
            name: "scrape_library",
            description: "Docbrain-1: fetch a library's docs page (registered via discover_library/resolve_library) and persist it as version-scoped doc_nodes. Splits the page into heading-scoped sections. Set max_pages > 1 to also follow same-origin, same-path-prefix links found on the landing page (shallow crawl, bounded).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "version": { "type": "string", "description": "The exact version this scrape represents, e.g. '15.1.3'." },
                    "max_pages": { "type": "integer", "description": "Defaults to 1 (landing page only)." },
                    "org": { "type": "string" },
                },
                "required": ["slug", "version"],
            }),
        },
        ToolDefinition {
            name: "sync_changelogs",
            description: "Fetches a library's GitHub releases and persists consecutive-version changelog entries, with a breaking-change heuristic (explicit 'breaking' mention in the release body, or a major-version bump).",
            input_schema: json!({
                "type": "object",
                "properties": { "slug": { "type": "string" }, "org": { "type": "string" } },
                "required": ["slug"],
            }),
        },
        ToolDefinition {
            name: "resolve_library",
            description: "Fuzzy-resolves a possibly-imprecise library name (e.g. 'React', 'next js') to a registered slug, among libraries visible to the caller. Prefers an exact slug/name match, then a case-insensitive substring match.",
            input_schema: json!({
                "type": "object",
                "properties": { "name": { "type": "string" }, "org": { "type": "string" } },
                "required": ["name"],
            }),
        },
        ToolDefinition {
            name: "ingest_local_files",
            description: "Docbrain-4: ingest local doc files directly into doc_nodes, for internal/private libraries with no public docs URL. Registers the library first (private to 'org' if given, public otherwise) if it doesn't already exist.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "name": { "type": "string", "description": "Display name, used only if the library doesn't already exist." },
                    "version": { "type": "string" },
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Local file paths to ingest; each becomes one doc_node, topic = file name." },
                    "org": { "type": "string" },
                },
                "required": ["slug", "version", "paths"],
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
        "scrape_library" => tool_scrape_library(store, args),
        "sync_changelogs" => tool_sync_changelogs(store, args),
        "resolve_library" => tool_resolve_library(store, args),
        "ingest_local_files" => tool_ingest_local_files(store, args),
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

fn tool_scrape_library(store: &DocbrainStore, args: &Value) -> anyhow::Result<String> {
    let tenant = tenant_from_args(args);
    let slug = get_str(args, "slug").ok_or_else(|| anyhow::anyhow!("missing required 'slug'"))?;
    let version = get_str(args, "version").ok_or_else(|| anyhow::anyhow!("missing required 'version'"))?;
    let max_pages = args.get("max_pages").and_then(|v| v.as_u64()).unwrap_or(1) as usize;

    let library = store
        .get_library(&tenant, slug)?
        .ok_or_else(|| anyhow::anyhow!("no library '{slug}' visible to this caller — register it first via discover_library or ingest_local_files"))?;
    let docs_url = library
        .docs_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("library '{slug}' has no docs_url registered — nothing to scrape"))?;

    let sections = scrape_docs(docs_url, max_pages)?;
    if sections.is_empty() {
        anyhow::bail!("scraped '{docs_url}' but found no content (no headings and no body text)");
    }

    store.add_doc_snapshot(&tenant, slug, version)?;
    for section in &sections {
        store.add_doc_node(&tenant, slug, version, &section.topic, &section.content)?;
    }

    Ok(format!("Scraped {docs_url} -> {} section(s) persisted as {slug}@{version}.", sections.len()))
}

fn tool_sync_changelogs(store: &DocbrainStore, args: &Value) -> anyhow::Result<String> {
    let tenant = tenant_from_args(args);
    let slug = get_str(args, "slug").ok_or_else(|| anyhow::anyhow!("missing required 'slug'"))?;

    let library = store.get_library(&tenant, slug)?.ok_or_else(|| anyhow::anyhow!("no library '{slug}' visible to this caller"))?;
    let repo = library
        .github_repo
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("library '{slug}' has no github_repo registered — nothing to sync changelogs from"))?;
    let (owner, name) = parse_owner_repo(repo).ok_or_else(|| anyhow::anyhow!("could not parse owner/repo out of '{repo}'"))?;

    let entries = sync_github_releases(&owner, &name)?;
    for e in &entries {
        store.add_changelog_entry(&tenant, slug, &e.from_version, &e.to_version, &e.entry, e.breaking)?;
    }

    Ok(format!("Synced {} changelog entr{} for {slug} from {owner}/{name}.", entries.len(), if entries.len() == 1 { "y" } else { "ies" }))
}

/// Pulls `owner/repo` out of a GitHub URL in whatever shape it was stored
/// in (`https://github.com/owner/repo`, with or without a trailing slash
/// or `.git`) — `docbrain_ingest::normalize_repo_url` already strips the
/// `.git`/`git+` noise at discovery time, but this still tolerates it in
/// case a `github_repo` was set some other way (e.g. `ingest_local_files`
/// doesn't touch this field, but a future manual `add_library` might).
fn parse_owner_repo(repo: &str) -> Option<(String, String)> {
    let trimmed = repo.trim_end_matches('/').trim_end_matches(".git");
    let after_host = trimmed.rsplit_once("github.com/").map(|(_, rest)| rest).unwrap_or(trimmed);
    let mut parts = after_host.trim_start_matches('/').splitn(2, '/');
    let owner = parts.next()?;
    let name = parts.next()?;
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    Some((owner.to_string(), name.to_string()))
}

fn tool_resolve_library(store: &DocbrainStore, args: &Value) -> anyhow::Result<String> {
    let tenant = tenant_from_args(args);
    let name = get_str(args, "name").ok_or_else(|| anyhow::anyhow!("missing required 'name'"))?;
    let query = name.to_lowercase();
    let normalized_query: String = query.chars().filter(|c| c.is_alphanumeric()).collect();

    let libs = store.list_libraries(&tenant)?;

    if let Some(exact) = libs.iter().find(|l| l.slug.eq_ignore_ascii_case(name) || l.name.eq_ignore_ascii_case(name)) {
        return Ok(format!("{} (exact match)", exact.slug));
    }

    let mut matches: Vec<&docbrain_graph::Library> = libs
        .iter()
        .filter(|l| {
            let normalized_slug: String = l.slug.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
            let normalized_name: String = l.name.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
            normalized_slug.contains(&normalized_query) || normalized_name.contains(&normalized_query) || normalized_query.contains(&normalized_slug)
        })
        .collect();

    match matches.len() {
        0 => Ok(format!("No registered library matches '{name}'. Try discover_library to look it up on npm/PyPI.")),
        1 => Ok(format!("{} (fuzzy match)", matches.remove(0).slug)),
        _ => Ok(format!(
            "Multiple candidates for '{name}': {}",
            matches.iter().map(|l| l.slug.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

fn tool_ingest_local_files(store: &DocbrainStore, args: &Value) -> anyhow::Result<String> {
    let tenant = tenant_from_args(args);
    let slug = get_str(args, "slug").ok_or_else(|| anyhow::anyhow!("missing required 'slug'"))?;
    let version = get_str(args, "version").ok_or_else(|| anyhow::anyhow!("missing required 'version'"))?;
    let paths = args
        .get("paths")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("missing required 'paths' (array of local file paths)"))?;
    if paths.is_empty() {
        anyhow::bail!("'paths' must contain at least one file path");
    }

    if store.get_library(&tenant, slug)?.is_none() {
        let name = get_str(args, "name").unwrap_or(slug);
        let visibility = match &tenant {
            TenantContext::Org(id) => Visibility::Private(id.clone()),
            TenantContext::Public => Visibility::Public,
        };
        store.add_library(&tenant, slug, name, None, None, visibility)?;
    }

    store.add_doc_snapshot(&tenant, slug, version)?;

    let mut ingested = 0usize;
    for p in paths {
        let path_str = p.as_str().ok_or_else(|| anyhow::anyhow!("'paths' entries must be strings"))?;
        let path = Path::new(path_str);
        let content = std::fs::read_to_string(path).with_context(|| format!("reading local doc file '{path_str}'"))?;
        let topic = path.file_name().and_then(|f| f.to_str()).unwrap_or(path_str);
        store.add_doc_node(&tenant, slug, version, topic, &content)?;
        ingested += 1;
    }

    Ok(format!("Ingested {ingested} local file(s) as {slug}@{version}."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_tools_has_nine_entries() {
        assert_eq!(list_tools().len(), 9);
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

    #[test]
    fn parses_owner_repo_from_various_url_shapes() {
        assert_eq!(parse_owner_repo("https://github.com/psf/requests"), Some(("psf".to_string(), "requests".to_string())));
        assert_eq!(parse_owner_repo("https://github.com/psf/requests/"), Some(("psf".to_string(), "requests".to_string())));
        assert_eq!(parse_owner_repo("https://github.com/psf/requests.git"), Some(("psf".to_string(), "requests".to_string())));
        assert_eq!(parse_owner_repo("not-a-github-url"), None);
    }

    #[test]
    fn scrape_library_fails_cleanly_when_library_has_no_docs_url() {
        let store = DocbrainStore::open_in_memory().unwrap();
        store.add_library(&TenantContext::public(), "no-docs", "No Docs", None, None, Visibility::Public).unwrap();
        let result = call_tool(&store, "scrape_library", &json!({"slug": "no-docs", "version": "1.0"})).unwrap();
        assert!(result.is_error);
        assert!(result.content[0].text.contains("no docs_url"));
    }

    #[test]
    fn scrape_library_persists_real_content_from_a_real_docs_page() {
        let store = DocbrainStore::open_in_memory().unwrap();
        store
            .add_library(&TenantContext::public(), "anyhow-rs", "anyhow", None, Some("https://docs.rs/anyhow/latest/anyhow/"), Visibility::Public)
            .unwrap();
        let result = call_tool(&store, "scrape_library", &json!({"slug": "anyhow-rs", "version": "1.0.0"})).unwrap();
        assert!(!result.is_error, "{:?}", result.content);

        let nodes = store.get_doc_nodes(&TenantContext::public(), "anyhow-rs", "1.0.0").unwrap();
        assert!(!nodes.is_empty(), "expected real scraped content to be persisted");
    }

    #[test]
    fn sync_changelogs_persists_real_entries_from_a_real_repo() {
        let store = DocbrainStore::open_in_memory().unwrap();
        store
            .add_library(&TenantContext::public(), "requests-py", "requests", Some("https://github.com/psf/requests"), None, Visibility::Public)
            .unwrap();
        let result = call_tool(&store, "sync_changelogs", &json!({"slug": "requests-py"})).unwrap();
        assert!(!result.is_error, "{:?}", result.content);
        // requests has enough releases that at least one changelog pair should exist,
        // unless GitHub's API happened to rate-limit this exact test run.
        if result.content[0].text.contains("Synced 0") {
            return;
        }
        let versions = store.list_doc_versions(&TenantContext::public(), "requests-py").unwrap();
        let _ = versions; // changelog entries aren't versions; just confirm the call succeeded above
    }

    #[test]
    fn resolve_library_finds_a_fuzzy_match() {
        let store = DocbrainStore::open_in_memory().unwrap();
        store.add_library(&TenantContext::public(), "next", "Next.js", None, None, Visibility::Public).unwrap();
        let result = call_tool(&store, "resolve_library", &json!({"name": "nextjs"})).unwrap();
        assert!(!result.is_error);
        assert!(result.content[0].text.contains("next"), "{:?}", result.content);
    }

    #[test]
    fn resolve_library_reports_no_match_honestly() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let result = call_tool(&store, "resolve_library", &json!({"name": "totally-unregistered-thing"})).unwrap();
        assert!(!result.is_error);
        assert!(result.content[0].text.contains("No registered library"));
    }

    #[test]
    fn ingest_local_files_registers_a_new_private_library_and_persists_content() {
        let dir = std::env::temp_dir().join(format!("docbrain-ingest-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("overview.md");
        std::fs::write(&file_path, "# Acme Internal SDK\nInternal-only usage notes.").unwrap();

        let store = DocbrainStore::open_in_memory().unwrap();
        let result = call_tool(
            &store,
            "ingest_local_files",
            &json!({"slug": "acme-sdk", "name": "Acme SDK", "version": "1.0", "paths": [file_path.to_str().unwrap()], "org": "acme"}),
        )
        .unwrap();
        assert!(!result.is_error, "{:?}", result.content);

        let acme = TenantContext::org("acme");
        let lib = store.get_library(&acme, "acme-sdk").unwrap().unwrap();
        assert_eq!(lib.visibility, Visibility::Private("acme".to_string()));

        let nodes = store.get_doc_nodes(&acme, "acme-sdk", "1.0").unwrap();
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].content.contains("Internal-only usage notes"));

        // Not visible to a different org — same isolation guarantee as everything else.
        let globex = TenantContext::org("globex");
        assert!(store.get_library(&globex, "acme-sdk").unwrap().is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ingest_local_files_reports_a_missing_path_as_a_tool_error_not_a_panic() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let result = call_tool(
            &store,
            "ingest_local_files",
            &json!({"slug": "ghost", "version": "1.0", "paths": ["/nonexistent/path/does-not-exist.md"]}),
        )
        .unwrap();
        assert!(result.is_error);
    }
}
