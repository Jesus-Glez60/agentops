use std::collections::HashSet;
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
        // `function_definition` here is a free function or an in-class
        // member definition — both land on this same node kind in
        // tree-sitter-cpp's grammar. Its name isn't in a `name` field
        // (see `cpp_function_name`'s doc comment) but it's still the
        // right node kind to match on.
        Language::Cpp => &["class_specifier", "struct_specifier", "function_definition"],
        Language::CSharp => &["class_declaration", "struct_declaration", "enum_declaration", "method_declaration"],
    }
}

/// Node kinds tree-sitter uses for plain identifier references, per
/// language -- used to collect a symbol's `references` set (see
/// `collect_identifiers`). Comments and string literals are never these
/// kinds in any of these grammars, so walking for them naturally excludes
/// comment/string text with no separate skip-list required. Mostly just
/// `"identifier"`; a few grammars split out a type- or field-specific
/// identifier kind for qualified/member access (`Foo.bar`, `pkg.Type`)
/// that would otherwise be missed.
fn reference_kinds(lang: Language) -> &'static [&'static str] {
    match lang {
        Language::Python => &["identifier"],
        Language::TypeScript | Language::JavaScript => &["identifier", "property_identifier", "type_identifier"],
        Language::Go => &["identifier", "type_identifier", "field_identifier"],
        Language::Rust => &["identifier", "type_identifier", "field_identifier"],
        Language::Cpp => &["identifier", "field_identifier", "type_identifier"],
        Language::CSharp => &["identifier"],
    }
}

/// Collects the text of every descendant node (including `node` itself)
/// whose kind is in `kinds` -- same recursion shape as `collect_definitions`,
/// just gathering identifier text instead of definition symbols.
fn collect_identifiers(node: &Node, src: &str, kinds: &[&str], out: &mut HashSet<String>) {
    if kinds.contains(&node.kind().as_str()) {
        out.insert(node_text(src, node).to_string());
    }
    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            collect_identifiers(&child, src, kinds, out);
        }
    }
}

/// C++'s `function_definition` node has no `name` field of its own — the
/// identifier is nested inside its `declarator` field, which for a plain
/// function is a `function_declarator` (whose own `declarator` field is
/// the `identifier`/`field_identifier`), or a `pointer_declarator`
/// wrapping a `function_declarator` for a pointer return type (`int*
/// foo()`) — confirmed via a standalone tree-sitter-cpp probe, not
/// guessed: `function_definition.declarator` is one of `function_declarator`
/// (common case) or `pointer_declarator` (needs one more `declarator`
/// unwrap), and `function_declarator.declarator` is the actual name node.
/// Recurses through `declarator` fields until landing on a node kind that
/// is actually a name (also handles `qualified_identifier` for
/// `Foo::bar()` out-of-class definitions, and `destructor_name`/
/// `operator_name` for the two other identifier-shaped leaf kinds this
/// grammar uses).
fn cpp_function_name(node: &Node, src: &str) -> Option<String> {
    match node.kind().as_str() {
        "identifier" | "field_identifier" | "qualified_identifier" | "destructor_name" | "operator_name" => Some(node_text(src, node).to_string()),
        _ => node.child_by_field_name("declarator").and_then(|d| cpp_function_name(&d, src)),
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
    collect_definitions(language, &root, source, kinds, None, &mut symbols);
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
        "trait_item" | "class_declaration" | "class_definition" | "class_specifier" | "struct_specifier" | "struct_declaration" => Some("name"),
        _ => None,
    }
}

fn collect_definitions(language: Language, node: &Node, src: &str, kinds: &[&str], scope: Option<&str>, out: &mut Vec<Symbol>) {
    if kinds.contains(&node.kind().as_str()) {
        // `"function_definition"` is also Python's node kind name for
        // `def foo():` (which *does* have a real `name` field) — this
        // must be scoped to C++ specifically, not matched by node-kind
        // string alone, or Python symbols silently come back
        // `"<anonymous>"` (a real regression this exact confusion caused
        // once already, caught immediately by the existing Python test).
        let name = if language == Language::Cpp && node.kind().as_str() == "function_definition" {
            cpp_function_name(node, src)
        } else {
            node.child_by_field_name("name").map(|n| node_text(src, &n).to_string())
        }
        .unwrap_or_else(|| "<anonymous>".to_string());

        let mut references = HashSet::new();
        collect_identifiers(node, src, reference_kinds(language), &mut references);
        references.remove(&name);

        out.push(Symbol {
            name,
            container: scope.map(str::to_string),
            kind: kind_label(&node.kind()).to_string(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            source: node_text(src, node).to_string(),
            references: references.into_iter().collect(),
        });
    }

    let child_scope = match container_scope_field(&node.kind()) {
        Some(field) => node.child_by_field_name(field).map(|n| node_text(src, &n).to_string()),
        None => None,
    };
    let child_scope = child_scope.as_deref().or(scope);

    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            collect_definitions(language, &child, src, kinds, child_scope, out);
        }
    }
}

// --- Regex fallback, all seven languages ---

static PY_DEF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^(?P<indent>[ \t]*)(def|class)\s+(?P<name>\w+)").unwrap());

static JS_DEF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(export\s+)?(default\s+)?(async\s+)?(function\s+(?P<fname>\w+)|class\s+(?P<cname>\w+))").unwrap());

static GO_DEF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*func\s+(?:\([^)]*\)\s+)?(?P<name>\w+)\s*\(").unwrap());

static RUST_DEF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:fn\s+(?P<fname>\w+)|struct\s+(?P<sname>\w+)|enum\s+(?P<ename>\w+)|trait\s+(?P<tname>\w+))")
        .unwrap()
});

/// Deliberately loose — C++ signatures (templates, pointers/references,
/// namespaces, trailing return types) are one of the hardest things to
/// match with a regex, and this only ever runs when a real tree-sitter
/// parse already failed outright. `(?P<fname>...)` matches a free/member
/// function definition line (`ReturnType name(args) {`); `(?P<tname>...)`
/// matches `class`/`struct` declarations directly.
static CPP_DEF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?:class|struct)\s+(?P<tname>\w+)|^\s*(?:[\w:*&<>,\s]+?)\s+(?P<fname>\w+)\s*\([^;{}]*\)\s*(?:const\s*)?\{").unwrap()
});

static CSHARP_DEF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^\s*(?:public|private|protected|internal|static|sealed|abstract|partial|\s)*(?:class|struct|enum)\s+(?P<tname>\w+)|^\s*(?:public|private|protected|internal|static|virtual|override|async|\s)*[\w<>\[\],\s]+?\s+(?P<mname>\w+)\s*\([^;{}]*\)\s*\{",
    )
    .unwrap()
});

fn regex_fallback(language: Language, source: &str) -> Vec<Symbol> {
    match language {
        Language::Python => python_regex_fallback(source),
        Language::TypeScript | Language::JavaScript => js_regex_fallback(source),
        Language::Go => go_regex_fallback(source),
        Language::Rust => rust_regex_fallback(source),
        Language::Cpp => cpp_regex_fallback(source),
        Language::CSharp => csharp_regex_fallback(source),
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

        symbols.push(Symbol { name, container: None, kind: kind.to_string(), start_line: i + 1, end_line: end + 1, source: lines[i..=end].join("\n"), references: Vec::new() });
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
        symbols.push(Symbol { name, container: None, kind: kind.to_string(), start_line: i + 1, end_line: end + 1, source: lines[i..=end].join("\n"), references: Vec::new() });
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
        symbols.push(Symbol { name, container: None, kind: kind.to_string(), start_line: i + 1, end_line: end + 1, source: lines[i..=end].join("\n"), references: Vec::new() });
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
        symbols.push(Symbol { name, container: None, kind: kind.to_string(), start_line: i + 1, end_line: end + 1, source: lines[i..=end].join("\n"), references: Vec::new() });
    }

    symbols
}

fn cpp_regex_fallback(source: &str) -> Vec<Symbol> {
    let lines: Vec<&str> = source.lines().collect();
    let mut symbols = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let Some(caps) = CPP_DEF.captures(line) else { continue };
        let (name, kind) = if let Some(m) = caps.name("tname") {
            (m.as_str().to_string(), if line.trim_start().starts_with("class") { "class" } else { "struct" })
        } else if let Some(m) = caps.name("fname") {
            (m.as_str().to_string(), "function")
        } else {
            continue;
        };

        let (end, _) = brace_depth_end(&lines, i);
        symbols.push(Symbol { name, container: None, kind: kind.to_string(), start_line: i + 1, end_line: end + 1, source: lines[i..=end].join("\n"), references: Vec::new() });
    }

    symbols
}

fn csharp_regex_fallback(source: &str) -> Vec<Symbol> {
    let lines: Vec<&str> = source.lines().collect();
    let mut symbols = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let Some(caps) = CSHARP_DEF.captures(line) else { continue };
        let (name, kind) = if let Some(m) = caps.name("tname") {
            let trimmed = line.trim_start();
            let kind = if trimmed.contains("class") { "class" } else if trimmed.contains("struct") { "struct" } else { "enum" };
            (m.as_str().to_string(), kind)
        } else if let Some(m) = caps.name("mname") {
            (m.as_str().to_string(), "method")
        } else {
            continue;
        };

        let (end, _) = brace_depth_end(&lines, i);
        symbols.push(Symbol { name, container: None, kind: kind.to_string(), start_line: i + 1, end_line: end + 1, source: lines[i..=end].join("\n"), references: Vec::new() });
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

    #[test]
    fn a_function_calling_a_sibling_gets_it_in_references() {
        let src = "fn helper() -> i32 {\n    1\n}\n\nfn main() {\n    let x = helper();\n    println!(\"{x}\");\n}\n";
        let (symbols, used_ts) = extract_symbols(Language::Rust, src);
        assert!(used_ts);
        let main_fn = symbols.iter().find(|s| s.name == "main").unwrap();
        assert!(main_fn.references.iter().any(|r| r == "helper"), "references: {:?}", main_fn.references);
    }

    #[test]
    fn a_symbol_name_mentioned_only_in_a_comment_is_not_a_reference() {
        // The whole reason AST identifier-node walking was chosen over
        // full-text regex matching: a comment mentioning another symbol's
        // name must never produce a false reference edge.
        let src = "fn helper() -> i32 {\n    1\n}\n\n// This does NOT call helper, just mentions it in prose.\nfn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n";
        let (symbols, used_ts) = extract_symbols(Language::Rust, src);
        assert!(used_ts);
        let main_fn = symbols.iter().find(|s| s.name == "main").unwrap();
        assert!(!main_fn.references.iter().any(|r| r == "helper"), "references: {:?}", main_fn.references);
    }

    #[test]
    fn a_symbol_name_mentioned_only_in_a_string_literal_is_not_a_reference() {
        let src = "fn helper() -> i32 {\n    1\n}\n\nfn main() {\n    let s = \"helper\";\n    println!(\"{s}\");\n}\n";
        let (symbols, used_ts) = extract_symbols(Language::Rust, src);
        assert!(used_ts);
        let main_fn = symbols.iter().find(|s| s.name == "main").unwrap();
        assert!(!main_fn.references.iter().any(|r| r == "helper"), "references: {:?}", main_fn.references);
    }

    #[test]
    fn a_symbol_never_references_itself() {
        let src = "fn factorial(n: u64) -> u64 {\n    if n == 0 { 1 } else { n * factorial(n - 1) }\n}\n";
        let (symbols, used_ts) = extract_symbols(Language::Rust, src);
        assert!(used_ts);
        let factorial = symbols.iter().find(|s| s.name == "factorial").unwrap();
        assert!(!factorial.references.iter().any(|r| r == "factorial"), "references: {:?}", factorial.references);
    }

    #[test]
    fn python_reference_detection_also_excludes_comments() {
        let src = "def helper():\n    return 1\n\n\n# does not call helper\ndef main():\n    return 2\n";
        let (symbols, used_ts) = extract_symbols(Language::Python, src);
        assert!(used_ts);
        let main_fn = symbols.iter().find(|s| s.name == "main").unwrap();
        assert!(!main_fn.references.iter().any(|r| r == "helper"), "references: {:?}", main_fn.references);
    }

    #[test]
    fn typescript_reference_detection_finds_a_call_to_a_sibling_function() {
        let src = "function helper(): number {\n  return 1;\n}\n\nfunction main(): number {\n  return helper();\n}\n";
        let (symbols, used_ts) = extract_symbols(Language::TypeScript, src);
        assert!(used_ts);
        let main_fn = symbols.iter().find(|s| s.name == "main").unwrap();
        assert!(main_fn.references.iter().any(|r| r == "helper"), "references: {:?}", main_fn.references);
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

    #[test]
    fn extracts_cpp_class_struct_and_member_function_via_tree_sitter() {
        let src = "class Foo {\npublic:\n  int bar(int x) { return x; }\n};\n\nint baz(int y) {\n  return y;\n}\n\nstruct Point {\n  int x;\n};\n";
        let (symbols, used_ts) = extract_symbols(Language::Cpp, src);
        assert!(used_ts, "tree-sitter should be available for cpp");
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Foo"), "found: {names:?}");
        assert!(names.contains(&"bar"), "found: {names:?}");
        assert!(names.contains(&"baz"), "found: {names:?}");
        assert!(names.contains(&"Point"), "found: {names:?}");
        assert!(!names.contains(&"<anonymous>"), "no C++ definition here should come back unnamed: {names:?}");

        let bar = symbols.iter().find(|s| s.name == "bar").unwrap();
        assert_eq!(bar.container.as_deref(), Some("Foo"), "a member function must be qualified by its enclosing class: {symbols:?}");

        let baz = symbols.iter().find(|s| s.name == "baz").unwrap();
        assert_eq!(baz.container, None, "a free function has no enclosing container");
    }

    #[test]
    fn extracts_csharp_class_struct_enum_and_method_via_tree_sitter() {
        let src = "public class Foo {\n  public int Bar(int x) {\n    return x;\n  }\n}\n\nstruct Point {\n  public int X;\n}\n\nenum Color {\n  Red,\n  Green,\n}\n";
        let (symbols, used_ts) = extract_symbols(Language::CSharp, src);
        assert!(used_ts, "tree-sitter should be available for csharp");
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Foo"), "found: {names:?}");
        assert!(names.contains(&"Bar"), "found: {names:?}");
        assert!(names.contains(&"Point"), "found: {names:?}");
        assert!(names.contains(&"Color"), "found: {names:?}");

        let bar = symbols.iter().find(|s| s.name == "Bar").unwrap();
        assert_eq!(bar.container.as_deref(), Some("Foo"), "a method must be qualified by its enclosing class: {symbols:?}");
    }

    #[test]
    fn cpp_regex_fallback_finds_class_struct_and_function() {
        let src = "class Foo {\n  int bar() { return 1; }\n};\n\nint baz(int y) {\n  return y;\n}\n\nstruct Point {\n  int x;\n};\n";
        let symbols = cpp_regex_fallback(src);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Foo"), "found: {names:?}");
        assert!(names.contains(&"baz"), "found: {names:?}");
        assert!(names.contains(&"Point"), "found: {names:?}");
    }

    #[test]
    fn csharp_regex_fallback_finds_class_and_method() {
        let src = "public class Foo {\n  public int Bar(int x) {\n    return x;\n  }\n}\n";
        let symbols = csharp_regex_fallback(src);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Foo"), "found: {names:?}");
        assert!(names.contains(&"Bar"), "found: {names:?}");
    }
}
