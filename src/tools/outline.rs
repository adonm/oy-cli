//! Outline tool for structural file analysis using tree-sitter.
//!
//! Extracts classes, functions, methods, and other top-level definitions
//! from source files using language-specific tree-sitter queries.

use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use tree_sitter::{Language, Parser, Query, QueryCursor};

use super::args::OutlineArgs;
use super::workspace;
use super::ToolContext;

#[derive(Debug, Clone, Serialize)]
pub(super) struct OutlineItem {
    pub kind: String,
    pub name: String,
    pub line: usize,
}

#[derive(Debug, Serialize)]
pub(super) struct OutlineOutput {
    pub path: String,
    pub items: Vec<OutlineItem>,
}

pub(super) fn tool_outline(ctx: &mut ToolContext, args: OutlineArgs) -> Result<Value> {
    let path = workspace::resolve_read_path(ctx, &args.path)?;
    if path.is_dir() {
        bail!("outline path is a directory: {}", args.path);
    }

    let content = workspace::read_file_content(ctx.root(), &path)?;
    let lang = detect_language(&path);
    let items = match lang {
        Some(lang) => parse_outline(&content, lang, args.depth)?,
        None => {
            // Fallback: return empty outline for unknown languages
            Vec::new()
        }
    };

    let output = OutlineOutput {
        path: args.path,
        items,
    };

    Ok(serde_json::to_value(output)?)
}

/// Supported language with grammar and query definitions.
struct LangDef {
    language: fn() -> Language,
    query: &'static str,
    extensions: &'static [&'static str],
}

/// All supported languages with their tree-sitter queries.
///
/// Each query captures `@kind` for the definition type and `@name` for the identifier.
/// Queries are written to match common definition patterns in each language.
static LANGUAGES: &[LangDef] = &[
    // Rust
    LangDef {
        language: || tree_sitter_rust::LANGUAGE.into(),
        extensions: &["rs"],
        query: r#"
(function_item name: (identifier) @name) @kind
(impl_item type: (type_identifier) @name) @kind
(struct_item name: (type_identifier) @name) @kind
(enum_item name: (type_identifier) @name) @kind
(trait_item name: (type_identifier) @name) @kind
(mod_item name: (identifier) @name) @kind
(const_item name: (identifier) @name) @kind
(static_item name: (identifier) @name) @kind
(type_item name: (type_identifier) @name) @kind
(macro_definition name: (identifier) @name) @kind
"#,
    },
    // Python
    LangDef {
        language: || tree_sitter_python::LANGUAGE.into(),
        extensions: &["py", "pyi"],
        query: r#"
(function_definition name: (identifier) @name) @kind
(class_definition name: (identifier) @name) @kind
(decorated_definition definition: (function_definition name: (identifier) @name)) @kind
(decorated_definition definition: (class_definition name: (identifier) @name)) @kind
"#,
    },
    // JavaScript
    LangDef {
        language: || tree_sitter_javascript::LANGUAGE.into(),
        extensions: &["js", "jsx", "mjs", "cjs"],
        query: r#"
(function_declaration name: (identifier) @name) @kind
(class_declaration name: (identifier) @name) @kind
(method_definition name: (property_identifier) @name) @kind
(generator_function_declaration name: (identifier) @name) @kind
"#,
    },
    // TypeScript
    LangDef {
        language: || tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        extensions: &["ts", "mts", "cts"],
        query: r#"
(function_declaration name: (identifier) @name) @kind
(class_declaration name: (type_identifier) @name) @kind
(method_definition name: (property_identifier) @name) @kind
(interface_declaration name: (type_identifier) @name) @kind
(type_alias_declaration name: (type_identifier) @name) @kind
(enum_declaration name: (identifier) @name) @kind
"#,
    },
    // TSX
    LangDef {
        language: || tree_sitter_typescript::LANGUAGE_TSX.into(),
        extensions: &["tsx"],
        query: r#"
(function_declaration name: (identifier) @name) @kind
(class_declaration name: (type_identifier) @name) @kind
(method_definition name: (property_identifier) @name) @kind
(interface_declaration name: (type_identifier) @name) @kind
(type_alias_declaration name: (type_identifier) @name) @kind
(enum_declaration name: (identifier) @name) @kind
"#,
    },
    // Go
    LangDef {
        language: || tree_sitter_go::LANGUAGE.into(),
        extensions: &["go"],
        query: r#"
(function_declaration name: (identifier) @name) @kind
(method_declaration name: (field_identifier) @name) @kind
(type_declaration (type_spec name: (type_identifier) @name)) @kind
"#,
    },
    // Java
    LangDef {
        language: || tree_sitter_java::LANGUAGE.into(),
        extensions: &["java"],
        query: r#"
(class_declaration name: (identifier) @name) @kind
(interface_declaration name: (identifier) @name) @kind
(method_declaration name: (identifier) @name) @kind
(enum_declaration name: (identifier) @name) @kind
(record_declaration name: (identifier) @name) @kind
"#,
    },
    // C
    LangDef {
        language: || tree_sitter_c::LANGUAGE.into(),
        extensions: &["c", "h"],
        query: r#"
(function_definition declarator: (function_declarator declarator: (identifier) @name)) @kind
(struct_specifier name: (type_identifier) @name) @kind
(enum_specifier name: (type_identifier) @name) @kind
(type_definition declarator: (type_identifier) @name) @kind
"#,
    },
    // C++
    LangDef {
        language: || tree_sitter_cpp::LANGUAGE.into(),
        extensions: &["cpp", "cc", "cxx", "hpp", "hxx", "hh"],
        query: r#"
(function_definition declarator: (function_declarator declarator: (identifier) @name)) @kind
(function_definition declarator: (function_declarator declarator: (qualified_identifier name: (identifier) @name))) @kind
(function_definition declarator: (pointer_declarator declarator: (function_declarator declarator: (identifier) @name))) @kind
(struct_specifier name: (type_identifier) @name) @kind
(class_specifier name: (type_identifier) @name) @kind
(enum_specifier name: (type_identifier) @name) @kind
(namespace_definition name: (identifier) @name) @kind
"#,
    },
    // C#
    LangDef {
        language: || tree_sitter_c_sharp::LANGUAGE.into(),
        extensions: &["cs"],
        query: r#"
(class_declaration name: (identifier) @name) @kind
(interface_declaration name: (identifier) @name) @kind
(method_declaration name: (identifier) @name) @kind
(struct_declaration name: (identifier) @name) @kind
(enum_declaration name: (identifier) @name) @kind
(record_declaration name: (identifier) @name) @kind
(namespace_declaration name: (identifier) @name) @kind
"#,
    },
    // Ruby
    LangDef {
        language: || tree_sitter_ruby::LANGUAGE.into(),
        extensions: &["rb"],
        query: r#"
(class name: (constant) @name) @kind
(module name: (constant) @name) @kind
(method name: (identifier) @name) @kind
(singleton_method name: (identifier) @name) @kind
"#,
    },
    // PHP
    LangDef {
        language: || tree_sitter_php::LANGUAGE_PHP.into(),
        extensions: &["php"],
        query: r#"
(function_definition name: (name) @name) @kind
(class_declaration name: (name) @name) @kind
(interface_declaration name: (name) @name) @kind
(method_declaration name: (name) @name) @kind
(trait_declaration name: (name) @name) @kind
(enum_declaration name: (name) @name) @kind
"#,
    },
    // Swift
    LangDef {
        language: || tree_sitter_swift::LANGUAGE.into(),
        extensions: &["swift"],
        query: r#"
(class_declaration name: (type_identifier) @name) @kind
(struct_declaration name: (type_identifier) @name) @kind
(enum_declaration name: (type_identifier) @name) @kind
(protocol_declaration name: (type_identifier) @name) @kind
(function_declaration name: (simple_identifier) @name) @kind
"#,
    },
    // Kotlin
    LangDef {
        language: || tree_sitter_kotlin_ng::LANGUAGE.into(),
        extensions: &["kt", "kts"],
        query: r#"
(class_declaration (simple_identifier) @name) @kind
(function_declaration (simple_identifier) @name) @kind
(object_declaration (simple_identifier) @name) @kind
(interface_declaration (simple_identifier) @name) @kind
"#,
    },
    // Bash
    LangDef {
        language: || tree_sitter_bash::LANGUAGE.into(),
        extensions: &["sh", "bash", "zsh"],
        query: r#"
(function_definition name: (word) @name) @kind
"#,
    },
    // Lua
    LangDef {
        language: || tree_sitter_lua::LANGUAGE.into(),
        extensions: &["lua"],
        query: r#"
(function_declaration name: (_) @name) @kind
"#,
    },
    // Dart
    LangDef {
        language: || tree_sitter_dart::LANGUAGE.into(),
        extensions: &["dart"],
        query: r#"
(class_definition name: (identifier) @name) @kind
(function_signature name: (identifier) @name) @kind
(method_signature name: (identifier) @name) @kind
(enum_declaration name: (identifier) @name) @kind
(mixin_declaration name: (identifier) @name) @kind
(extension_declaration name: (identifier) @name) @kind
"#,
    },
];

/// Detect language from file extension.
fn detect_language(path: &Path) -> Option<&'static LangDef> {
    let ext = path.extension()?.to_str()?;
    LANGUAGES.iter().find(|lang| lang.extensions.contains(&ext))
}

/// Extract the kind label from a tree-sitter node kind string.
fn node_kind_to_label(ts_kind: &str) -> &'static str {
    match ts_kind {
        // Rust
        "function_item" => "function",
        "impl_item" => "impl",
        "struct_item" => "struct",
        "enum_item" => "enum",
        "trait_item" => "trait",
        "mod_item" => "module",
        "const_item" => "const",
        "static_item" => "static",
        "type_item" => "type",
        "macro_definition" => "macro",
        // Python
        "function_definition" => "function",
        "class_definition" => "class",
        "decorated_definition" => "definition",
        // JavaScript/TypeScript
        "function_declaration" => "function",
        "class_declaration" => "class",
        "method_definition" => "method",
        "generator_function_declaration" => "function",
        "interface_declaration" => "interface",
        "type_alias_declaration" => "type",
        "enum_declaration" => "enum",
        // Go
        "method_declaration" => "function",
        "type_declaration" => "type",
        "type_spec" => "type",
        // C/C++
        "struct_specifier" => "struct",
        "class_specifier" => "class",
        "enum_specifier" => "enum",
        "namespace_definition" => "namespace",
        // C#
        "struct_declaration" => "struct",
        "record_declaration" => "record",
        "namespace_declaration" => "namespace",
        // Ruby
        "class" => "class",
        "module" => "module",
        "method" => "function",
        "singleton_method" => "function",
        // PHP
        "trait_declaration" => "trait",
        // Swift
        "protocol_declaration" => "protocol",
        // Kotlin
        "object_declaration" => "object",
        // Dart
        "function_signature" => "function",
        "method_signature" => "method",
        "mixin_declaration" => "mixin",
        "extension_declaration" => "extension",
        // Fallback
        _ => {
            if ts_kind.contains("function") {
                "function"
            } else if ts_kind.contains("class") {
                "class"
            } else if ts_kind.contains("method") {
                "method"
            } else if ts_kind.contains("struct") {
                "struct"
            } else if ts_kind.contains("enum") {
                "enum"
            } else if ts_kind.contains("interface") {
                "interface"
            } else if ts_kind.contains("trait") {
                "trait"
            } else if ts_kind.contains("module") || ts_kind.contains("namespace") {
                "module"
            } else {
                "definition"
            }
        }
    }
}

/// Parse a source file and extract definitions using tree-sitter.
///
/// The `_depth` parameter is reserved for future recursive expansion but currently unused
/// as the parser only extracts top-level definitions.
fn parse_outline(source: &str, lang: &LangDef, _depth: usize) -> Result<Vec<OutlineItem>> {
    let language = (lang.language)();
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .map_err(|e| anyhow::anyhow!("failed to set language: {e}"))?;

    let tree = match parser.parse(source, None) {
        Some(tree) => tree,
        None => return Ok(Vec::new()),
    };

    let query = match Query::new(&language, lang.query) {
        Ok(q) => q,
        Err(_) => {
            // If the query has issues, return empty rather than failing
            return Ok(Vec::new());
        }
    };

    let kind_idx = query.capture_index_for_name("kind").unwrap();
    let name_idx = query.capture_index_for_name("name").unwrap();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();

    use streaming_iterator::StreamingIterator;
    while let Some(m) = matches.next() {
        let mut kind_node = None;
        let mut name_text = None;

        for capture in m.captures {
            if capture.index == kind_idx {
                kind_node = Some(capture.node);
            }
            if capture.index == name_idx {
                let node = capture.node;
                name_text = Some(
                    source[node.byte_range()]
                        .lines()
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string(),
                );
            }
        }

        if let (Some(kn), Some(name)) = (kind_node, name_text) {
            if name.is_empty() || !seen.insert((name.clone(), kn.start_position().row)) {
                continue;
            }
            items.push(OutlineItem {
                kind: node_kind_to_label(kn.kind()).to_string(),
                name,
                line: kn.start_position().row + 1, // 1-based
            });
        }
    }

    // Sort by line number
    items.sort_by_key(|item| item.line);
    Ok(items)
}
