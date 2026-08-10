use std::sync::LazyLock;

use regex::Regex;
use tree_sitter_language_pack::Node;

use crate::types::{Language, Symbol};

/// Node kinds tree-sitter uses for "definition-like" constructs, per
/// language. The identifier is read from each definition node's `name`
/// field, a convention shared across these grammars.
fn definition_kinds(lang: Language) -> &'static [&'static str] {
    match lang {
        Language::Python => &["function_definition", "class_definition"],
        Language::TypeScript | Language::JavaScript => &["function_declaration", "class_declaration", "method_definition"],
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

/// Extracts symbols for `source`, preferring a real tree-sitter parse and
/// falling back to a regex-based extraction — for all five languages
/// uniformly. `main` only had a regex fallback for Python/TS/JS; Go and
/// Rust silently produced zero symbols on a failed parse, and only Go's gap
/// was even tracked. Both are brace-delimited, so both get the same
/// brace-depth-counting technique already used for JS/TS.
///
/// Returns `(symbols, used_tree_sitter)`.
pub fn extract_symbols(language: Language, source: &str) -> (Vec<Symbol>, bool) {
    match try_tree_sitter(language, source) {
        Some(symbols) => (symbols, true),
        None => (regex_fallback(language, source), false),
    }
}

fn try_tree_sitter(language: Language, source: &str) -> Option<Vec<Symbol>> {
    let mut parser = tree_sitter_language_pack::get_parser(language.tree_sitter_name()).ok()?;
    let tree = parser.parse(source)?;
    let root = tree.root_node();

    let kinds = definition_kinds(language);
    let mut symbols = Vec::new();
    collect_definitions(&root, source, kinds, None, &mut symbols);
    Some(symbols)
}

fn node_text<'a>(src: &'a str, node: &Node) -> &'a str {
    let r = node.byte_range();
    &src[r.start..r.end]
}

/// Node kinds that introduce a naming scope for symbols found inside them —
/// e.g. `impl Foo { fn new() }`'s `new` must be qualified by `Foo`, since a
/// bare `new` collides across every other `impl` block in the same file
/// (confirmed via live testing: `agentops-graph/src/lib.rs` alone has three
/// unrelated `as_db_str` methods, one per `impl` block). Each entry names
/// the tree-sitter node kind and which of its fields holds the scope's own
/// identifying name — `impl_item`'s Self type is its `type` field, not a
/// `name` field, since it isn't itself a named declaration.
fn container_scope_field(node_kind: &str) -> Option<&'static str> {
    match node_kind {
        "impl_item" => Some("type"),
        "trait_item" | "class_declaration" | "class_definition" => Some("name"),
        _ => None,
    }
}

fn collect_definitions(node: &Node, src: &str, kinds: &[&str], scope: Option<&str>, out: &mut Vec<Symbol>) {
    if kinds.contains(&node.kind().as_str()) {
        let name = node.child_by_field_name("name").map(|n| node_text(src, &n).to_string()).unwrap_or_else(|| "<anonymous>".to_string());

        out.push(Symbol {
            name,
            container: scope.map(str::to_string),
            kind: kind_label(&node.kind()).to_string(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            source: node_text(src, node).to_string(),
        });
    }

    let child_scope = match container_scope_field(&node.kind()) {
        Some(field) => node.child_by_field_name(field).map(|n| node_text(src, &n).to_string()),
        None => None,
    };
    let child_scope = child_scope.as_deref().or(scope);

    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            collect_definitions(&child, src, kinds, child_scope, out);
        }
    }
}

// --- Regex fallback, all five languages ---

static PY_DEF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^(?P<indent>[ \t]*)(def|class)\s+(?P<name>\w+)").unwrap());

static JS_DEF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(export\s+)?(default\s+)?(async\s+)?(function\s+(?P<fname>\w+)|class\s+(?P<cname>\w+))").unwrap());

static GO_DEF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*func\s+(?:\([^)]*\)\s+)?(?P<name>\w+)\s*\(").unwrap());

static RUST_DEF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:fn\s+(?P<fname>\w+)|struct\s+(?P<sname>\w+)|enum\s+(?P<ename>\w+)|trait\s+(?P<tname>\w+))")
        .unwrap()
});

fn regex_fallback(language: Language, source: &str) -> Vec<Symbol> {
    match language {
        Language::Python => python_regex_fallback(source),
        Language::TypeScript | Language::JavaScript => js_regex_fallback(source),
        Language::Go => go_regex_fallback(source),
        Language::Rust => rust_regex_fallback(source),
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

        symbols.push(Symbol { name, container: None, kind: kind.to_string(), start_line: i + 1, end_line: end + 1, source: lines[i..=end].join("\n") });
    }

    symbols
}

/// Brace-depth counting: a function/class ends where its opening `{` finds
/// its matching `}`.
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

        let (end, _) = brace_depth_end(&lines, i);
        symbols.push(Symbol { name, container: None, kind: kind.to_string(), start_line: i + 1, end_line: end + 1, source: lines[i..=end].join("\n") });
    }

    symbols
}

/// Same brace-depth technique as `js_regex_fallback`, applied to Go's
/// `func Name(...)`/`func (recv Type) Name(...)` forms — Go is
/// brace-delimited exactly like JS/TS, so a failed tree-sitter parse no
/// longer means zero symbols.
fn go_regex_fallback(source: &str) -> Vec<Symbol> {
    let lines: Vec<&str> = source.lines().collect();
    let mut symbols = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let Some(caps) = GO_DEF.captures(line) else { continue };
        let name = caps.name("name").unwrap().as_str().to_string();
        let kind = if line.trim_start().starts_with("func (") { "method" } else { "function" };

        let (end, _) = brace_depth_end(&lines, i);
        symbols.push(Symbol { name, container: None, kind: kind.to_string(), start_line: i + 1, end_line: end + 1, source: lines[i..=end].join("\n") });
    }

    symbols
}

/// Same technique again for Rust's `fn`/`struct`/`enum`/`trait` — with one
/// addition brace-delimited Go/JS don't need: a unit/tuple struct
/// (`struct Foo;`, `struct Foo(i32);`) has no body at all, so the scan also
/// stops at a top-level `;` seen before any `{`.
fn rust_regex_fallback(source: &str) -> Vec<Symbol> {
    let lines: Vec<&str> = source.lines().collect();
    let mut symbols = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let Some(caps) = RUST_DEF.captures(line) else { continue };
        let (name, kind) = if let Some(m) = caps.name("fname") {
            (m.as_str().to_string(), "function")
        } else if let Some(m) = caps.name("sname") {
            (m.as_str().to_string(), "struct")
        } else if let Some(m) = caps.name("ename") {
            (m.as_str().to_string(), "enum")
        } else if let Some(m) = caps.name("tname") {
            (m.as_str().to_string(), "trait")
        } else {
            continue;
        };

        let (end, _) = brace_or_semicolon_end(&lines, i);
        symbols.push(Symbol { name, container: None, kind: kind.to_string(), start_line: i + 1, end_line: end + 1, source: lines[i..=end].join("\n") });
    }

    symbols
}

/// Scans forward from `start` counting `{`/`}` depth, returning the line
/// index where depth returns to zero after having opened at least once.
fn brace_depth_end(lines: &[&str], start: usize) -> (usize, bool) {
    let mut depth = 0i32;
    let mut seen_open = false;
    let mut end = start;
    for (j, later) in lines.iter().enumerate().skip(start) {
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
            break;
        }
    }
    (end, seen_open)
}

/// Like `brace_depth_end`, but also stops at a top-level `;` seen before
/// any `{` — for declarations with no body (`struct Foo;`).
fn brace_or_semicolon_end(lines: &[&str], start: usize) -> (usize, bool) {
    let mut depth = 0i32;
    let mut seen_open = false;
    let mut end = start;
    'outer: for (j, later) in lines.iter().enumerate().skip(start) {
        for ch in later.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    seen_open = true;
                }
                '}' => depth -= 1,
                ';' if !seen_open => {
                    end = j;
                    break 'outer;
                }
                _ => {}
            }
        }
        end = j;
        if seen_open && depth <= 0 {
            break 'outer;
        }
    }
    (end, seen_open)
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
        assert!(names.contains(&"render"), "found: {names:?}");

        let widget = symbols.iter().find(|s| s.name == "Widget").unwrap();
        assert_eq!(widget.kind, "struct");

        let render = symbols.iter().find(|s| s.name == "render").unwrap();
        assert_eq!(render.container.as_deref(), Some("Widget"), "a method inside impl Widget must be qualified by Widget");
        assert_eq!(widget.container, None, "a top-level struct has no enclosing container");
    }

    /// Regression test for a confirmed real bug found via live testing
    /// against this actual repo: `agentops-graph/src/lib.rs` defines
    /// `as_db_str` in three unrelated `impl` blocks. Without `container`,
    /// all three would carry the identical bare name `as_db_str` and
    /// collide under the graph's `(repo, kind, path, name)` natural key.
    #[test]
    fn methods_in_different_impl_blocks_get_distinct_containers() {
        let src = "impl Foo {\n    fn new() -> Self { Foo }\n}\n\nimpl Bar {\n    fn new() -> Self { Bar }\n}\n";
        let (symbols, used_ts) = extract_symbols(Language::Rust, src);
        assert!(used_ts);

        let news: Vec<&Symbol> = symbols.iter().filter(|s| s.name == "new").collect();
        assert_eq!(news.len(), 2, "found: {symbols:?}");
        let containers: std::collections::HashSet<_> = news.iter().map(|s| s.container.as_deref()).collect();
        assert_eq!(containers, std::collections::HashSet::from([Some("Foo"), Some("Bar")]), "each impl block's method must carry its own type as container");
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

    /// Regression test for the confirmed gap: `main` produced zero symbols
    /// here (no Go regex fallback existed at all).
    #[test]
    fn go_regex_fallback_finds_function_and_method() {
        let src = "package main\n\nfunc Add(a int, b int) int {\n\treturn a + b\n}\n\nfunc (w *Widget) Render() string {\n\treturn w.Name\n}\n";
        let symbols = go_regex_fallback(src);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Add"), "found: {names:?}");
        assert!(names.contains(&"Render"), "found: {names:?}");
        let render = symbols.iter().find(|s| s.name == "Render").unwrap();
        assert_eq!(render.kind, "method");
    }

    /// Regression test for the confirmed gap: `main` produced zero symbols
    /// here (no Rust regex fallback existed at all — an identical gap to
    /// Go's, but not even tracked).
    #[test]
    fn rust_regex_fallback_finds_fn_struct_enum_trait() {
        let src = "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub struct Widget {\n    name: String,\n}\n\nstruct Unit;\n\nenum Color {\n    Red,\n    Blue,\n}\n\ntrait Shape {\n    fn area(&self) -> f64;\n}\n";
        let symbols = rust_regex_fallback(src);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"add"), "found: {names:?}");
        assert!(names.contains(&"Widget"), "found: {names:?}");
        assert!(names.contains(&"Unit"), "found: {names:?}");
        assert!(names.contains(&"Color"), "found: {names:?}");
        assert!(names.contains(&"Shape"), "found: {names:?}");

        let unit = symbols.iter().find(|s| s.name == "Unit").unwrap();
        assert_eq!(unit.source, "struct Unit;", "a body-less struct must terminate at its semicolon, not run to end of file");
    }
}
