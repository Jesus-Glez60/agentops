//! Indexes a real, already-scanned repo's graph store into Qdrant and runs
//! real semantic queries against it. Usage:
//!   cargo run --release -p agentops-embeddings --example index_and_query -- \
//!     <path to repo's .context/graph.db> <repo name> <query> [query...]

use agentops_embeddings::{collect_index_items, SemanticIndex};
use agentops_graph::SqliteGraphStore;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let db_path = args.next().expect("usage: index_and_query <graph.db path> <repo name> <query...>");
    let repo = args.next().expect("repo name required");
    let queries: Vec<String> = args.collect();

    let store = SqliteGraphStore::open(std::path::Path::new(&db_path))?;
    let items = collect_index_items(&store, &repo)?;
    let mut index = SemanticIndex::connect("http://localhost:6334", "agentops_demo")?;
    index.ensure_collection().await?;

    let count = index.index_items(&items).await?;
    println!("Indexed {count} nodes from {db_path}\n");

    for query in &queries {
        println!("query: {query:?}");
        let hits = index.search(query, 3, Some(&repo)).await?;
        for hit in hits {
            let label = hit.name.as_deref().unwrap_or("(unnamed)");
            println!("  {:.3}  [{}] {}", hit.score, hit.kind, label);
        }
        println!();
    }

    Ok(())
}
