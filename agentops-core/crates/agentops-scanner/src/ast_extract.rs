use std::sync::LazyLock;

use regex::Regex;
use tree_sitter_language_pack::Node;

use crate::types::{Language, Symbol};

/// Node kinds tree-sitter uses for "definition-like" constructs, per language.
/// These are the standard node kind names for each grammar (function/class/method
/// definitions); the identifier is read from each definition node's `name` field,
/// which is a convention shared across these grammars.
fn definition_kinds(lang: Language) -> &'static [&'static str] {
    match lang {
        Language::Python => &["function_definition", "class_definition"],
        Language::TypeScript | Language::JavaScript => {
            &["function_declaration", "class_declaration", "method_definition"]
        }
        Language::Go => &["function_declaration", "method_declaration"],
        // `impl_item` deliberately excluded: its grammar node has no `name`
        // field (it has `type`/`trait` fields instead), so matching it here
        // would just produce "<anonymous>" entries. Methods defined inside
        // an impl block are still found — they're `function_item` nodes,
        // and `collect_definitions` recurses into every child regardless of
        // whether the parent node matched.
        Language::Rust => &["function_item", "struct_item", "enum_item", "trait_item"],
    }
}

fn kind_label(node_kind: &str) -> &'static str {
    if node_kind.contains("class") {
        "class"
    } else if node_kind.contains("struct") {
        "struct"
    } else if node_kind.contains("enum") {
        "enum"
    } else if node_kind.contains("trait") {
        "trait"
    } else if node_kind.contains("method") {
        "method"
    } else {
        "function"
    }
}

/// Extracts symbols for `source`, preferring a real tree-sitter parse and falling
/// back to a regex-based extraction for Python/TypeScript/JavaScript if parsing
/// fails. Go has no regex fallback (see SECURITY.md / the plan) — a failed parse
/// yields zero symbols, which the chunker then treats as "no symbols found" and
/// falls back to sliding-window chunking.
///
/// Returns `(symbols, used_tree_sitter)`.
pub fn extract_symbols(language: Language, source: &str) -> (Vec<Symbol>, bool) {
    match try_tree_sitter(language, source) {
        Some(symbols) => (symbols, true),
        None => match language {
            Language::Python | Language::TypeScript | Language::JavaScript => {
                (regex_fallback(language, source), false)
            }
            // No regex fallback for Go or Rust — same tradeoff already
            // documented for Go: a failed parse yields zero symbols rather
            // than a best-effort regex guess, and the chunker treats that as
            // "no symbols found," falling back to sliding-window chunking.
            Language::Go | Language::Rust => (Vec::new(), false),
        },
    }
}

fn try_tree_sitter(language: Language, source: &str) -> Option<Vec<Symbol>> {
    let mut parser = tree_sitter_language_pack::get_parser(language.tree_sitter_name()).ok()?;
    let tree = parser.parse(source)?;
    let root = tree.root_node();

    let kinds = definition_kinds(language);
    let mut symbols = Vec::new();
    collect_definitions(&root, source, kinds, &mut symbols);
    Some(symbols)
}

fn node_text<'a>(src: &'a str, node: &Node) -> &'a str {
    let r = node.byte_range();
    &src[r.start..r.end]
}

fn collect_definitions(node: &Node, src: &str, kinds: &[&str], out: &mut Vec<Symbol>) {
    if kinds.contains(&node.kind().as_str()) {
        let name = node
            .child_by_field_name("name")
            .map(|n| node_text(src, &n).to_string())
            .unwrap_or_else(|| "<anonymous>".to_string());

        out.push(Symbol {
            name,
            kind: kind_label(&node.kind()).to_string(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            source: node_text(src, node).to_string(),
        });
    }

    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            collect_definitions(&child, src, kinds, out);
        }
    }
}

// --- Regex fallback (python / typescript / javascript only) ---

static PY_DEF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^(?P<indent>[ \t]*)(def|class)\s+(?P<name>\w+)").unwrap());

static JS_DEF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(export\s+)?(default\s+)?(async\s+)?(function\s+(?P<fname>\w+)|class\s+(?P<cname>\w+))")
        .unwrap()
});

fn regex_fallback(language: Language, source: &str) -> Vec<Symbol> {
    match language {
        Language::Python => python_regex_fallback(source),
        Language::TypeScript | Language::JavaScript => js_regex_fallback(source),
        Language::Go | Language::Rust => Vec::new(),
    }
}

/// Indentation-based block detection: a def/class ends where the next
/// same-or-lower-indentation, non-blank line begins.
fn python_regex_fallback(source: &str) -> Vec<Symbol> {
    let lines: Vec<&str> = source.lines().collect();
    let mut symbols = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let Some(caps) = PY_DEF.captures(line) else { continue };
        let indent = caps.name("indent").unwrap().as_str().len();
        let name = caps.name("name").unwrap().as_str().to_string();
        let kind = if line.trim_start().starts_with("class") { "class" } else { "function" };

        let mut end = i;
        for (j, later) in lines.iter().enumerate().skip(i + 1) {
            if later.trim().is_empty() {
                end = j;
                continue;
            }
            let later_indent = later.len() - later.trim_start().len();
            if later_indent <= indent {
                break;
            }
            end = j;
        }

        symbols.push(Symbol {
            name,
            kind: kind.to_string(),
            start_line: i + 1,
            end_line: end + 1,
            source: lines[i..=end].join("\n"),
        });
    }

    symbols
}

/// Brace-depth counting: a function/class ends where its opening `{` finds its
/// matching `}`.
fn js_regex_fallback(source: &str) -> Vec<Symbol> {
    let lines: Vec<&str> = source.lines().collect();
    let mut symbols = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let Some(caps) = JS_DEF.captures(line) else { continue };
        let (name, kind) = if let Some(m) = caps.name("fname") {
            (m.as_str().to_string(), "function")
        } else if let Some(m) = caps.name("cname") {
            (m.as_str().to_string(), "class")
        } else {
            continue;
        };

        // Find matching closing brace starting from this line.
        let mut depth = 0i32;
        let mut seen_open = false;
        let mut end = i;
        'outer: for (j, later) in lines.iter().enumerate().skip(i) {
            for ch in later.chars() {
                match ch {
                    '{' => {
                        depth += 1;
                        seen_open = true;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            end = j;
            if seen_open && depth <= 0 {
                break 'outer;
            }
        }

        symbols.push(Symbol {
            name,
            kind: kind.to_string(),
            start_line: i + 1,
            end_line: end + 1,
            source: lines[i..=end].join("\n"),
        });
    }

    symbols
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_python_function_and_class_via_tree_sitter() {
        let src = "def greet(name):\n    return f\"hi {name}\"\n\n\nclass Greeter:\n    def hello(self):\n        return 1\n";
        let (symbols, used_ts) = extract_symbols(Language::Python, src);
        assert!(used_ts, "tree-sitter should be available for python");
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"greet"), "found: {names:?}");
        assert!(names.contains(&"Greeter"), "found: {names:?}");
        assert!(names.contains(&"hello"), "found: {names:?}");
    }

    #[test]
    fn extracts_typescript_function_and_class_via_tree_sitter() {
        let src = "export function add(a: number, b: number): number {\n  return a + b;\n}\n\nclass Widget {\n  render() {\n    return null;\n  }\n}\n";
        let (symbols, used_ts) = extract_symbols(Language::TypeScript, src);
        assert!(used_ts, "tree-sitter should be available for typescript");
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"add"), "found: {names:?}");
        assert!(names.contains(&"Widget"), "found: {names:?}");
    }

    #[test]
    fn extracts_go_function_via_tree_sitter() {
        let src = "package main\n\nfunc Add(a int, b int) int {\n\treturn a + b\n}\n";
        let (symbols, used_ts) = extract_symbols(Language::Go, src);
        assert!(used_ts, "tree-sitter should be available for go");
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Add"), "found: {names:?}");
    }

    #[test]
    fn extracts_rust_function_struct_and_impl_method_via_tree_sitter() {
        let src = "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub struct Widget {\n    name: String,\n}\n\nimpl Widget {\n    pub fn render(&self) -> String {\n        self.name.clone()\n    }\n}\n";
        let (symbols, used_ts) = extract_symbols(Language::Rust, src);
        assert!(used_ts, "tree-sitter should be available for rust");
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"add"), "found: {names:?}");
        assert!(names.contains(&"Widget"), "found: {names:?}");
        // `render` lives inside an `impl` block (not matched directly, since
        // impl_item has no `name` field) but should still surface, since
        // collect_definitions recurses into every child regardless.
        assert!(names.contains(&"render"), "found: {names:?}");

        let widget = symbols.iter().find(|s| s.name == "Widget").unwrap();
        assert_eq!(widget.kind, "struct");
    }

    #[test]
    fn python_regex_fallback_finds_def_and_class() {
        let src = "def greet(name):\n    return name\n\nclass Greeter:\n    def hello(self):\n        return 1\n";
        let symbols = python_regex_fallback(src);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"greet"));
        assert!(names.contains(&"Greeter"));
    }

    #[test]
    fn js_regex_fallback_finds_function_and_class() {
        let src = "function add(a, b) {\n  return a + b;\n}\n\nclass Widget {\n  render() {\n    return null;\n  }\n}\n";
        let symbols = js_regex_fallback(src);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"add"));
        assert!(names.contains(&"Widget"));
    }
}
