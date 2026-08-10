use crate::types::{Chunk, ChunkKind, Symbol};

const FILE_HEADER_LINES: usize = 30;
const MAX_SYMBOL_LINES: usize = 150;
const WINDOW_SIZE: usize = 150;
const WINDOW_OVERLAP: usize = 20;

/// Produces chunks for a file: always a file-header chunk (first 30 lines),
/// plus either one chunk per symbol (truncated at 150 lines) if any symbols
/// were found, or a sliding-window fallback (150-line window, 20-line
/// overlap) if none were.
pub fn chunk_file(source: &str, symbols: &[Symbol]) -> Vec<Chunk> {
    let lines: Vec<&str> = source.lines().collect();
    let mut chunks = Vec::new();

    let header_end = lines.len().min(FILE_HEADER_LINES);
    chunks.push(Chunk { kind: ChunkKind::FileHeader, name: None, start_line: 1, end_line: header_end, text: lines[..header_end].join("\n") });

    if symbols.is_empty() {
        chunks.extend(sliding_window(&lines));
    } else {
        for symbol in symbols {
            chunks.push(symbol_chunk(symbol));
        }
    }

    chunks
}

fn symbol_chunk(symbol: &Symbol) -> Chunk {
    let lines: Vec<&str> = symbol.source.lines().collect();
    let truncated = lines.len() > MAX_SYMBOL_LINES;
    let text = if truncated {
        let mut t = lines[..MAX_SYMBOL_LINES].join("\n");
        t.push_str("\n... [truncated]");
        t
    } else {
        symbol.source.clone()
    };

    Chunk { kind: ChunkKind::Symbol, name: Some(symbol.name.clone()), start_line: symbol.start_line, end_line: symbol.end_line, text }
}

fn sliding_window(lines: &[&str]) -> Vec<Chunk> {
    if lines.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let step = WINDOW_SIZE - WINDOW_OVERLAP;
    let mut start = 0usize;

    loop {
        let end = (start + WINDOW_SIZE).min(lines.len());
        chunks.push(Chunk { kind: ChunkKind::Window, name: None, start_line: start + 1, end_line: end, text: lines[start..end].join("\n") });

        if end == lines.len() {
            break;
        }
        start += step;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_one_chunk_per_symbol_plus_header() {
        let source = "def a():\n    pass\n\ndef b():\n    pass\n";
        let symbols = vec![
            Symbol { name: "a".into(), container: None, kind: "function".into(), start_line: 1, end_line: 2, source: "def a():\n    pass".into() },
            Symbol { name: "b".into(), container: None, kind: "function".into(), start_line: 4, end_line: 5, source: "def b():\n    pass".into() },
        ];
        let chunks = chunk_file(source, &symbols);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].kind, ChunkKind::FileHeader);
        assert_eq!(chunks[1].kind, ChunkKind::Symbol);
        assert_eq!(chunks[1].name.as_deref(), Some("a"));
    }

    #[test]
    fn truncates_long_symbols_at_150_lines() {
        let long_source = (0..300).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let symbol = Symbol { name: "big".into(), container: None, kind: "function".into(), start_line: 1, end_line: 300, source: long_source };
        let chunk = symbol_chunk(&symbol);
        assert!(chunk.text.contains("[truncated]"));
        assert_eq!(chunk.text.lines().count(), MAX_SYMBOL_LINES + 1);
    }

    #[test]
    fn falls_back_to_sliding_window_when_no_symbols_found() {
        let source = (0..400).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let chunks = chunk_file(&source, &[]);
        assert!(chunks.len() >= 3);
        assert!(chunks[1..].iter().all(|c| c.kind == ChunkKind::Window));

        let first_end = chunks[1].end_line;
        let second_start = chunks[2].start_line;
        assert!(second_start < first_end, "windows should overlap");
    }
}
