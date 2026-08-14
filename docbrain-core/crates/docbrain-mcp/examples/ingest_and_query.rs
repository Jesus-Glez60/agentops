//! Manual end-to-end smoke test: scrape real Next.js 16 docs, then run a
//! semantic search against them through the actual MCP tool dispatch path.
//! Not part of the product — a throwaway example for verifying the
//! ingestion/chunking/embedding/search pipeline against a real site.
//!
//! Run with: cargo run -p docbrain-mcp --example ingest_and_query

use std::path::Path;

use docbrain_graph::{DocbrainStore, SqliteDocbrainStore};
use serde_json::json;

fn main() -> anyhow::Result<()> {
    let db_path = std::env::temp_dir().join("docbrain-nextjs-demo.sqlite3");
    let _ = std::fs::remove_file(&db_path);
    let store = SqliteDocbrainStore::open(&db_path)?;

    store.add_library("nextjs", "Next.js", None, Some("vercel/next.js"), Some("https://nextjs.org/docs/app/getting-started"))?;

    println!("Scraping Next.js 16 docs (this hits the real network)...");
    let outcomes = docbrain_mcp::call_tool(
        &store,
        Path::new(&db_path),
        "scrape_library",
        &json!({ "slug": "nextjs", "version": "16", "max_pages": 6 }),
    )
    .map_err(|e| anyhow::anyhow!(e))?;
    print_result("scrape_library", &outcomes);

    let lib = store.get_library("nextjs")?.unwrap();
    println!("\nLibrary now has {} doc nodes across versions {:?}.\n", lib.total_nodes, lib.versions);

    let query = "How do I define a dynamic route segment in the app router?";
    println!("=== PLANNING STAGE: search_docs({query:?}) — code examples excluded by default ===\n");
    let planning = docbrain_mcp::call_tool(&store, Path::new(&db_path), "search_docs", &json!({ "query": query, "top_k": 3, "max_tokens": 1200 }))
        .map_err(|e| anyhow::anyhow!(e))?;
    print_result("search_docs (planning)", &planning);

    // Find a top-level hit ("## ...") whose section actually references a
    // code example (not every hit has one — e.g. a table-only reference
    // section) — a real caller would just try get_code_examples on
    // whichever node id looked most relevant to what it's about to build.
    let text = &planning.content[0].text;
    let node_id = text
        .split("\n## ")
        .find(|block| block.contains("[Code example"))
        .and_then(|block| block.split("(node ").nth(1))
        .and_then(|rest| rest.split(',').next())
        .and_then(|id| id.parse::<i64>().ok())
        .expect("expected at least one hit referencing a code example in the planning-stage result");

    println!("=== IMPLEMENTATION STAGE: get_code_examples(node_id: {node_id}) ===\n");
    let implementation = docbrain_mcp::call_tool(&store, Path::new(&db_path), "get_code_examples", &json!({ "node_id": node_id })).map_err(|e| anyhow::anyhow!(e))?;
    print_result("get_code_examples (implementation)", &implementation);

    Ok(())
}

fn print_result(tool: &str, result: &docbrain_mcp::CallToolResult) {
    println!("--- {tool} result (is_error={}) ---", result.is_error);
    for block in &result.content {
        println!("{}", block.text);
    }
    println!("--- end {tool} ---\n");
}
