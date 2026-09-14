//! Per-language editing conventions: comment tokens, auto-closing pairs, tags.

use std::path::Path;

pub enum Comment {
    Line(&'static str),
    Block(&'static str, &'static str),
}

pub fn comment_for(path: &Path) -> Option<Comment> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    Some(match ext.as_str() {
        "rs" | "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "go" | "java" | "c" | "h" | "cc" | "cpp" | "hpp" | "cs" | "swift" | "kt" | "kts"
        | "scala" | "php" | "dart" | "zig" | "proto" | "groovy" | "gradle" | "jsonc" | "sol" | "v" => Comment::Line("//"),
        "py" | "sh" | "bash" | "zsh" | "fish" | "rb" | "yaml" | "yml" | "toml" | "pl" | "r" | "ex" | "exs" | "nim" | "cmake" | "conf" | "ini"
        | "dockerfile" | "tf" | "hcl" | "nix" | "ps1" => Comment::Line("#"),
        "sql" | "lua" | "hs" | "elm" => Comment::Line("--"),
        "tex" | "erl" => Comment::Line("%"),
        "clj" | "cljs" | "lisp" | "el" | "scm" => Comment::Line(";;"),
        "vim" => Comment::Line("\""),
        "html" | "htm" | "xml" | "svg" | "md" | "markdown" | "vue" | "svelte" => Comment::Block("<!--", "-->"),
        "css" | "scss" | "less" => Comment::Block("/*", "*/"),
        _ if name == "Makefile" || name == "Dockerfile" || name.starts_with(".env") => Comment::Line("#"),
        _ => return None,
    })
}

/// Languages where typing `>` after an opening tag inserts the closing tag.
pub fn closes_tags(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).unwrap_or(""),
        "html" | "htm" | "xml" | "svg" | "jsx" | "tsx" | "vue" | "svelte"
    )
}

pub fn closing(c: char) -> Option<char> {
    Some(match c {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '"' => '"',
        '\'' => '\'',
        '`' => '`',
        _ => return None,
    })
}

pub fn is_closer(c: char) -> bool {
    matches!(c, ')' | ']' | '}' | '"' | '\'' | '`')
}

pub fn bracket_pair(c: char) -> Option<(char, char, bool)> {
    Some(match c {
        '(' => ('(', ')', true),
        '[' => ('[', ']', true),
        '{' => ('{', '}', true),
        ')' => ('(', ')', false),
        ']' => ('[', ']', false),
        '}' => ('{', '}', false),
        _ => return None,
    })
}

/// If `before` (text left of the cursor, including the just-typed `>`) ends an
/// opening tag, return its name.
pub fn open_tag_name(before: &str) -> Option<String> {
    let body = before.strip_suffix('>')?;
    if body.ends_with('/') || body.ends_with('-') {
        return None;
    }
    let lt = body.rfind('<')?;
    let tag = &body[lt + 1..];
    if tag.contains('>') {
        return None;
    }
    let name: String = tag.chars().take_while(|c| c.is_alphanumeric() || matches!(c, '-' | '.' | ':' | '_')).collect();
    let first = name.chars().next()?;
    if !first.is_alphabetic() {
        return None;
    }
    const VOID: [&str; 14] = ["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];
    (!VOID.contains(&name.to_ascii_lowercase().as_str())).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags() {
        assert_eq!(open_tag_name("<div class=\"a\">").as_deref(), Some("div"));
        assert_eq!(open_tag_name("return <Foo.Bar x={1}>").as_deref(), Some("Foo.Bar"));
        assert_eq!(open_tag_name("<br>"), None);
        assert_eq!(open_tag_name("<img />"), None);
        assert_eq!(open_tag_name("a -> b >"), None);
        assert_eq!(open_tag_name("if (a <b>"), Some("b".into()));
    }
}
