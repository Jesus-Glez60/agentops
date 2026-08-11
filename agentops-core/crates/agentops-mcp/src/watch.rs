//! Local file-watcher auto-rescan (Phase 5, 1.0 roadmap, Module 4).
//! `notify-debouncer-full`'s API confirmed via a standalone probe, not
//! assumed: `new_debouncer(timeout, tick_rate, handler)` +
//! `.watch(path, RecursiveMode::Recursive)`.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use notify_debouncer_full::notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebounceEventResult};

/// A changed path under `.context`/`.agentops`/`.git` is this process's
/// *own* write-back (the graph db, notes, or a commit) — not a real source
/// edit. Without filtering these out, every rescan's own writes would
/// immediately re-trigger the debouncer, rescanning forever.
fn is_agentops_output_path(path: &Path) -> bool {
    path.components().any(|c| matches!(c.as_os_str().to_str(), Some(".context") | Some(".agentops") | Some(".git")))
}

/// Watches `path` and calls `scan_and_persist` on every debounced batch of
/// real (non-self-triggered) filesystem events, forever — returns only on
/// an unrecoverable setup error (e.g. the path doesn't exist). Blocks the
/// calling thread.
///
/// Debounced-event callbacks run on `notify-debouncer-full`'s own
/// background thread, not inside any ambient async runtime — calling
/// `scan_and_persist` (sync, may internally reach
/// `PostgresGraphStore::block_on` if `AGENTOPS_DATABASE_URL` is set)
/// directly from here is safe by the same reasoning that already fixed
/// `agentops-cli`'s own `main()`: no runtime is already running on this
/// thread, so there's no nested-`block_on` panic risk.
pub fn watch_and_rescan(path: &Path, with_embeddings: bool) -> Result<()> {
    let path_owned = path.to_path_buf();

    let mut debouncer = new_debouncer(Duration::from_millis(500), None, move |result: DebounceEventResult| match result {
        Ok(events) => {
            if !events.iter().any(|e| e.paths.iter().any(|p| !is_agentops_output_path(p))) {
                return;
            }
            match crate::scan::scan_and_persist(&path_owned, with_embeddings) {
                Ok(summary) => println!(
                    "Rescanned {}: {} files, {} symbols, {} dependency edges ({} files pruned, {} symbols pruned)",
                    path_owned.display(),
                    summary.files,
                    summary.symbols,
                    summary.dependency_edges,
                    summary.pruned_files,
                    summary.pruned_symbols
                ),
                Err(e) => eprintln!("watch: rescan of {} failed: {e}", path_owned.display()),
            }
        }
        Err(errors) => eprintln!("watch: filesystem-event error: {errors:?}"),
    })?;

    // Deliberately NOT one `debouncer.watch(path, RecursiveMode::Recursive)`
    // on the whole repo root — that also registers (and, at startup,
    // synchronously enumerates) every file under `target`/`node_modules`/
    // etc, which is both wasted work and, on a real repo with a populated
    // build dir, a genuine CPU/latency problem (confirmed live against
    // this project's own `target/` directories: the naive version pinned a
    // CPU core and never settled). `watchable_dirs` returns the same
    // pruned directory set `scan_repo` itself walks, and each one is
    // registered `NonRecursive` — since the set already includes every
    // surviving directory at every depth, recursing further would just
    // re-discover directories already registered individually (or,
    // outside this pruning, an excluded one).
    let mut watched_count = 0;
    for dir in agentops_scanner::watchable_dirs(path) {
        if debouncer.watch(&dir, RecursiveMode::NonRecursive).is_ok() {
            watched_count += 1;
        }
    }

    println!("Watching {} directories under {} for changes (Ctrl+C to stop)...", watched_count, path.display());
    loop {
        std::thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agentops_output_paths_are_recognized_regardless_of_which_component_matches() {
        assert!(is_agentops_output_path(Path::new("/repo/.context/graph.db")));
        assert!(is_agentops_output_path(Path::new("/repo/.agentops/notes/foo.md")));
        assert!(is_agentops_output_path(Path::new("/repo/.git/index")));
        assert!(!is_agentops_output_path(Path::new("/repo/src/main.rs")));
    }
}
