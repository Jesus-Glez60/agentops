use tree_sitter_language_pack::Node;

use crate::types::Language;

/// Node kinds that represent an executable function/method body (as opposed
/// to a struct/class/trait's field or member list, which is the useful
/// payload of that kind of definition, not implementation detail to hide) —
/// a function-only subset of `ast_extract::definition_kinds`.
fn compressible_kinds(lang: Language) -> &'static [&'static str] {
    match lang {
        Language::Python => &["function_definition"],
        Language::TypeScript | Language::JavaScript => &["function_declaration", "method_definition"],
        Language::Go => &["function_declaration", "method_declaration"],
        Language::Rust => &["function_item"],
        Language::Cpp => &["function_definition"],
        // A bare re-parsed method snippet (no enclosing `class`, since
        // `Symbol::source` for a C# method is just the method's own text)
        // doesn't parse as `method_declaration` at all -- the grammar reads
        // it as a C# 7+ local function statement instead (confirmed live:
        // dumping the parse tree for a standalone method body showed
        // `local_function_statement`, not `method_declaration`). Both kinds
        // are covered so real extracted symbols (always missing their
        // class context) still compress.
        Language::CSharp => &["method_declaration", "local_function_statement"],
    }
}

/// Per-language placeholder for an elided function body. Brace languages'
/// `body` field includes the surrounding `{`/`}` in its byte range, so the
/// marker supplies its own; Python's `body` field is an indented block with
/// no braces, so a bare ellipsis reads naturally in its place instead.
fn elision_marker(lang: Language) -> &'static str {
    match lang {
        Language::Python => "...",
        _ => "{ /* ... */ }",
    }
}

fn find_first(node: &Node, kinds: &[&str]) -> Option<Node> {
    if kinds.contains(&node.kind().as_str()) {
        return Some(node.clone());
    }
    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            if let Some(found) = find_first(&child, kinds) {
                return Some(found);
            }
        }
    }
    None
}

/// Compresses `source` — a single already-extracted symbol's own source text
/// (`Symbol::source`, not a whole file) — by keeping its signature and
/// eliding its body, for use as a fallback when a symbol is too large to fit
/// an LLM prompt's token budget unabridged. Re-parses `source` as its own
/// standalone mini-source (never the original file) via the same
/// `tree_sitter_language_pack` parser `ast_extract` uses, so this never
/// needs the caller to hold onto a tree-sitter tree past extraction time.
///
/// Falls back to returning `source` unchanged whenever compression isn't
/// possible or wouldn't apply: the language has no parser available, the
/// mini-source fails to parse (e.g. a regex-fallback-extracted symbol whose
/// text isn't valid standalone syntax on its own), no compressible
/// definition node is found inside it (e.g. a struct/class/trait, whose
/// body is its useful payload, not detail worth hiding), or that node has no
/// `body` field. Callers should compare token counts of the result against
/// the original and only use the compressed version if it's actually
/// shorter — a short body with a long leading doc-comment can compress to
/// something no smaller than the original.
pub fn compress_symbol_source(source: &str, language: Language) -> String {
    let Ok(mut parser) = tree_sitter_language_pack::get_parser(language.tree_sitter_name()) else {
        return source.to_string();
    };
    let Some(tree) = parser.parse(source) else {
        return source.to_string();
    };

    let Some(def_node) = find_first(&tree.root_node(), compressible_kinds(language)) else {
        return source.to_string();
    };
    let Some(body) = def_node.child_by_field_name("body") else {
        return source.to_string();
    };

    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..body.start_byte()]);
    out.push_str(elision_marker(language));
    out.push_str(&source[body.end_byte()..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compresses_python_function_body() {
        let src = "def foo(x: int) -> int:\n    y = x + 1\n    return y\n";
        let out = compress_symbol_source(src, Language::Python);
        assert!(out.starts_with("def foo(x: int) -> int:"));
        assert!(out.contains("..."));
        assert!(!out.contains("return y"));
    }

    #[test]
    fn compresses_python_decorated_function_body_and_keeps_decorator() {
        let src = "@property\ndef foo(self) -> int:\n    return self._x\n";
        let out = compress_symbol_source(src, Language::Python);
        assert!(out.starts_with("@property\ndef foo(self) -> int:"));
        assert!(!out.contains("self._x"));
    }

    #[test]
    fn compresses_rust_function_body_and_keeps_generics() {
        let src = "pub fn foo<T: Clone>(x: T) -> T {\n    let y = x.clone();\n    y\n}\n";
        let out = compress_symbol_source(src, Language::Rust);
        assert!(out.starts_with("pub fn foo<T: Clone>(x: T) -> T {"));
        assert!(out.contains("{ /* ... */ }"));
        assert!(!out.contains("y.clone"));
    }

    #[test]
    fn compresses_typescript_function_body() {
        let src = "function foo(x: number): number {\n  const y = x + 1;\n  return y;\n}\n";
        let out = compress_symbol_source(src, Language::TypeScript);
        assert!(out.starts_with("function foo(x: number): number "));
        assert!(!out.contains("return y"));
    }

    #[test]
    fn compresses_go_function_body() {
        let src = "func foo(x int) int {\n\ty := x + 1\n\treturn y\n}\n";
        let out = compress_symbol_source(src, Language::Go);
        assert!(out.starts_with("func foo(x int) int "));
        assert!(!out.contains("return y"));
    }

    #[test]
    fn compresses_cpp_function_body() {
        let src = "int foo(int x) {\n    int y = x + 1;\n    return y;\n}\n";
        let out = compress_symbol_source(src, Language::Cpp);
        assert!(out.starts_with("int foo(int x) "));
        assert!(!out.contains("return y"));
    }

    #[test]
    fn compresses_csharp_method_body() {
        let src = "public int Foo(int x) {\n    int y = x + 1;\n    return y;\n}\n";
        let out = compress_symbol_source(src, Language::CSharp);
        assert!(out.starts_with("public int Foo(int x) "));
        assert!(!out.contains("return y"));
    }

    #[test]
    fn leaves_struct_definitions_unchanged() {
        let src = "pub struct Foo {\n    pub x: i32,\n    pub y: i32,\n}\n";
        let out = compress_symbol_source(src, Language::Rust);
        assert_eq!(out, src);
    }

    #[test]
    fn leaves_unparseable_source_unchanged() {
        let src = "this is not valid rust at all {{{ ]]]";
        let out = compress_symbol_source(src, Language::Rust);
        assert_eq!(out, src);
    }
}
