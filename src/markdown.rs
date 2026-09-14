//! Renders Markdown into styled, wrapped terminal lines for the preview.

use std::sync::LazyLock;

use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use regex::Regex;
use unicode_width::UnicodeWidthStr;

use crate::theme;

pub type Span = (String, Style);
pub type MdLine = Vec<Span>;

static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());

struct Quote {
    kind: Option<BlockQuoteKind>,
}

struct Table {
    align: Vec<Alignment>,
    rows: Vec<Vec<MdLine>>,
    head_rows: usize,
    in_head: bool,
}

struct Renderer {
    width: usize,
    out: Vec<MdLine>,
    inline: MdLine,
    bold: usize,
    italic: usize,
    strike: usize,
    link: usize,
    image: usize,
    heading: Option<HeadingLevel>,
    lists: Vec<Option<u64>>,
    /// Pending bullet for the first line of the current list item.
    item_marker: Option<String>,
    quotes: Vec<Quote>,
    code: Option<(String, Vec<String>)>,
    table: Option<Table>,
    summary: bool,
}

impl Renderer {
    fn style(&self) -> Style {
        let mut s = Style::default().fg(theme::FG());
        if self.heading.is_some() || self.bold > 0 || self.summary {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.italic > 0 {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if self.strike > 0 {
            s = s.add_modifier(Modifier::CROSSED_OUT);
        }
        if self.link > 0 {
            s = s.fg(theme::LINK()).add_modifier(Modifier::UNDERLINED);
        }
        if self.image > 0 {
            s = s.fg(theme::DIM());
        }
        match self.heading {
            Some(HeadingLevel::H1) => s = s.fg(theme::ACCENT()),
            Some(HeadingLevel::H2) => s = s.fg(theme::BORDER_FOCUS()),
            Some(HeadingLevel::H3) => s = s.fg(theme::DIR()),
            Some(_) => s = s.fg(theme::LINK()),
            None => {}
        }
        s
    }

    fn push(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        if let Some(t) = &mut self.table {
            if let Some(row) = t.rows.last_mut() {
                if let Some(cell) = row.last_mut() {
                    cell.push((text.to_string(), style));
                }
            }
            return;
        }
        self.inline.push((text.to_string(), style));
    }

    /// Prefix for block quotes (and list indentation) on every line.
    fn prefixes(&self) -> (MdLine, MdLine) {
        let mut first: MdLine = Vec::new();
        let mut rest: MdLine = Vec::new();
        for q in &self.quotes {
            let color = match q.kind {
                Some(BlockQuoteKind::Warning | BlockQuoteKind::Caution) => theme::ERROR(),
                Some(BlockQuoteKind::Important) => theme::BORDER_FOCUS(),
                Some(BlockQuoteKind::Tip) => theme::ADDED(),
                Some(BlockQuoteKind::Note) => theme::DIR(),
                None => theme::BORDER(),
            };
            first.push(("▌ ".into(), Style::default().fg(color)));
            rest.push(("▌ ".into(), Style::default().fg(color)));
        }
        let depth = self.lists.len();
        if depth > 0 {
            let indent = "  ".repeat(depth - 1);
            match &self.item_marker {
                Some(m) => {
                    first.push((format!("{indent}{m}"), Style::default().fg(theme::ACCENT())));
                    rest.push((" ".repeat(indent.len() + m.width()), Style::default()));
                }
                None => {
                    let pad = " ".repeat(indent.len() + 2);
                    first.push((pad.clone(), Style::default()));
                    rest.push((pad, Style::default()));
                }
            }
        }
        (first, rest)
    }

    fn flush(&mut self) {
        let spans = std::mem::take(&mut self.inline);
        if spans.iter().all(|(t, _)| t.trim().is_empty()) {
            return;
        }
        let (first, rest) = self.prefixes();
        self.item_marker = None;
        let lines = wrap(&spans, self.width, &first, &rest);
        self.out.extend(lines);
    }

    fn blank(&mut self) {
        if self.out.last().is_some_and(|l| !l.is_empty()) {
            if self.quotes.is_empty() {
                self.out.push(Vec::new());
            } else {
                let (_, rest) = self.prefixes();
                self.out.push(rest.into_iter().filter(|(t, _)| t.contains('▌')).collect());
            }
        }
    }

    fn event(&mut self, ev: Event) {
        if let Some((_, lines)) = &mut self.code {
            match ev {
                Event::Text(t) => {
                    for (i, part) in t.split('\n').enumerate() {
                        if i > 0 || lines.is_empty() {
                            lines.push(String::new());
                        }
                        lines.last_mut().unwrap().push_str(part);
                    }
                }
                Event::End(TagEnd::CodeBlock) => self.end_code(),
                _ => {}
            }
            return;
        }
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => {
                let s = self.style();
                self.push(&t, s);
            }
            Event::Code(t) => {
                let style = Style::default().fg(theme::ACCENT()).bg(theme::STATUS_BG());
                self.push(&format!(" {t} "), style);
            }
            Event::InlineMath(t) | Event::DisplayMath(t) => {
                let s = self.style().add_modifier(Modifier::ITALIC);
                self.push(&t, s);
            }
            Event::Html(h) | Event::InlineHtml(h) => self.html(&h),
            Event::SoftBreak => {
                let s = self.style();
                self.push(" ", s);
            }
            Event::HardBreak => {
                self.flush();
            }
            Event::Rule => {
                self.flush();
                self.blank();
                self.out.push(vec![("─".repeat(self.width), Style::default().fg(theme::BORDER()))]);
                self.out.push(Vec::new());
            }
            Event::TaskListMarker(done) => {
                let (mark, color) = if done { ("☑ ", theme::ADDED()) } else { ("☐ ", theme::DIM()) };
                self.push(mark, Style::default().fg(color));
            }
            Event::FootnoteReference(r) => {
                let s = Style::default().fg(theme::DIM());
                self.push(&format!("[{r}]"), s);
            }
        }
    }

    fn html(&mut self, html: &str) {
        let lower = html.to_ascii_lowercase();
        if lower.contains("<summary") {
            self.flush();
            self.summary = true;
            self.push("▸ ", Style::default().fg(theme::ACCENT()));
        }
        let text = TAG_RE.replace_all(html, "");
        let text = text.trim();
        if !text.is_empty() {
            let s = self.style();
            self.push(text, s);
        }
        if lower.contains("</summary") {
            self.flush();
            self.summary = false;
        }
        if lower.contains("<br") || lower.contains("</div") || lower.contains("</p") {
            self.flush();
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                self.flush();
                self.blank();
                self.heading = Some(level);
                if level == HeadingLevel::H1 {
                    self.push("█ ", Style::default().fg(theme::ACCENT()));
                } else if level == HeadingLevel::H2 {
                    self.push("▍", Style::default().fg(theme::BORDER_FOCUS()));
                }
            }
            Tag::BlockQuote(kind) => {
                self.flush();
                self.quotes.push(Quote { kind });
                if let Some(k) = kind {
                    let (label, color) = match k {
                        BlockQuoteKind::Note => ("ℹ Note", theme::DIR()),
                        BlockQuoteKind::Tip => ("✦ Tip", theme::ADDED()),
                        BlockQuoteKind::Important => ("❖ Important", theme::BORDER_FOCUS()),
                        BlockQuoteKind::Warning => ("⚠ Warning", theme::ERROR()),
                        BlockQuoteKind::Caution => ("⛔ Caution", theme::ERROR()),
                    };
                    self.push(label, Style::default().fg(color).add_modifier(Modifier::BOLD));
                    self.flush();
                }
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                self.blank();
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, Vec::new()));
            }
            Tag::List(start) => {
                self.flush();
                if self.lists.is_empty() {
                    self.blank();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush();
                let depth = self.lists.len();
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => match depth {
                        1 => "• ".into(),
                        2 => "◦ ".into(),
                        _ => "▪ ".into(),
                    },
                };
                self.item_marker = Some(marker);
            }
            Tag::Table(align) => {
                self.flush();
                self.blank();
                self.table = Some(Table { align, rows: Vec::new(), head_rows: 0, in_head: false });
            }
            Tag::TableHead => {
                if let Some(t) = &mut self.table {
                    t.in_head = true;
                    t.rows.push(Vec::new());
                }
            }
            Tag::TableRow => {
                if let Some(t) = &mut self.table {
                    t.rows.push(Vec::new());
                }
            }
            Tag::TableCell => {
                if let Some(t) = &mut self.table {
                    if let Some(row) = t.rows.last_mut() {
                        row.push(Vec::new());
                    }
                }
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { .. } => self.link += 1,
            Tag::Image { .. } => {
                self.image += 1;
                self.push("[", Style::default().fg(theme::DIM()));
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Heading(level) => {
                self.flush();
                self.heading = None;
                if matches!(level, HeadingLevel::H1 | HeadingLevel::H2) {
                    let ch = if level == HeadingLevel::H1 { "━" } else { "─" };
                    self.out.push(vec![(ch.repeat(self.width), Style::default().fg(theme::BORDER()))]);
                }
                self.out.push(Vec::new());
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quotes.pop();
                self.blank();
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Item => self.flush(),
            TagEnd::HtmlBlock => self.flush(),
            TagEnd::Table => self.end_table(),
            TagEnd::TableHead => {
                if let Some(t) = &mut self.table {
                    t.in_head = false;
                    t.head_rows = t.rows.len();
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => self.link = self.link.saturating_sub(1),
            TagEnd::Image => {
                self.push("]", Style::default().fg(theme::DIM()));
                self.image = self.image.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn end_code(&mut self) {
        let Some((lang, mut lines)) = self.code.take() else { return };
        if lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        let bg = Style::default().bg(theme::STATUS_BG());
        let (_, rest) = self.prefixes();
        let prefix_w: usize = rest.iter().map(|(t, _)| t.width()).sum();
        let inner = self.width.saturating_sub(prefix_w);
        let label = if lang.is_empty() { String::new() } else { format!(" {lang} ") };
        let mut top = rest.clone();
        top.push((format!("{:>inner$}", label), bg.fg(theme::DIM())));
        self.out.push(top);
        for l in lines {
            let text: String = l.replace('\t', "    ");
            let mut row = rest.clone();
            let shown: String = truncate(&format!("  {text}"), inner);
            let pad = inner.saturating_sub(shown.width());
            row.push((shown, bg.fg(theme::FG())));
            row.push((" ".repeat(pad), bg));
            self.out.push(row);
        }
        let mut bottom = rest;
        bottom.push((" ".repeat(inner), bg));
        self.out.push(bottom);
        self.out.push(Vec::new());
    }

    fn end_table(&mut self) {
        let Some(t) = self.table.take() else { return };
        let cols = t.rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let text_of = |cell: &MdLine| cell.iter().map(|(s, _)| s.as_str()).collect::<String>();
        let mut widths = vec![0usize; cols];
        for row in &t.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(text_of(cell).width());
            }
        }
        // Shrink the widest columns until the table fits.
        let budget = self.width.saturating_sub(cols * 3 + 1);
        while widths.iter().sum::<usize>() > budget && widths.iter().any(|&w| w > 6) {
            let i = (0..cols).max_by_key(|&i| widths[i]).unwrap();
            widths[i] -= 1;
        }
        let border = Style::default().fg(theme::BORDER());
        let line = |l: &str, m: &str, r: &str| -> MdLine {
            let body: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
            vec![(format!("{l}{}{r}", body.join(m)), border)]
        };
        self.out.push(line("┌", "┬", "┐"));
        for (ri, row) in t.rows.iter().enumerate() {
            let mut out: MdLine = vec![("│".into(), border)];
            for (ci, w) in widths.iter().enumerate() {
                let empty = Vec::new();
                let cell = row.get(ci).unwrap_or(&empty);
                let text = truncate(&text_of(cell), *w);
                let pad = w - text.width();
                let (left, right) = match t.align.get(ci) {
                    Some(Alignment::Right) => (pad, 0),
                    Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                    _ => (0, pad),
                };
                let mut style = cell.first().map_or(Style::default().fg(theme::FG()), |(_, s)| *s);
                if ri < t.head_rows {
                    style = style.add_modifier(Modifier::BOLD).fg(theme::ACCENT());
                }
                out.push((format!(" {}{}{} ", " ".repeat(left), text, " ".repeat(right)), style));
                out.push(("│".into(), border));
            }
            self.out.push(out);
            if ri + 1 == t.head_rows {
                self.out.push(line("├", "┼", "┤"));
            }
        }
        self.out.push(line("└", "┴", "┘"));
        self.out.push(Vec::new());
    }
}

fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        if out.width() + unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) + 1 > width {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

/// Word-wrap styled spans to `width`, with prefixes for the first and later lines.
fn wrap(spans: &[Span], width: usize, first: &MdLine, rest: &MdLine) -> Vec<MdLine> {
    let mut lines: Vec<MdLine> = Vec::new();
    let mut line: MdLine = first.clone();
    let prefix_w = |l: &MdLine| l.iter().map(|(t, _)| t.width()).sum::<usize>();
    let mut col = prefix_w(first);
    let mut at_start = true;
    for (text, style) in spans {
        // Split into words and whitespace runs so styles survive wrapping.
        let mut token = String::new();
        let mut tokens: Vec<String> = Vec::new();
        for c in text.chars() {
            if c.is_whitespace() != token.chars().last().is_some_and(char::is_whitespace) && !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
            token.push(if c == '\n' { ' ' } else { c });
        }
        if !token.is_empty() {
            tokens.push(token);
        }
        for tok in tokens {
            let space = tok.chars().all(char::is_whitespace);
            let w = tok.width();
            if space {
                if at_start {
                    continue;
                }
                if col + 1 > width {
                    continue;
                }
                line.push((" ".into(), *style));
                col += 1;
                continue;
            }
            if col + w > width && !at_start {
                while line.last().is_some_and(|(t, _)| t == " ") {
                    line.pop();
                }
                lines.push(std::mem::replace(&mut line, rest.clone()));
                col = prefix_w(rest);
            }
            // Hard-break words longer than the line.
            let mut tok = tok;
            while col + tok.width() > width && tok.chars().count() > 1 && width > col + 1 {
                let fit = width - col;
                let head: String = tok.chars().take(fit).collect();
                let tail: String = tok.chars().skip(fit).collect();
                line.push((head, *style));
                lines.push(std::mem::replace(&mut line, rest.clone()));
                col = prefix_w(rest);
                tok = tail;
            }
            col += tok.width();
            line.push((tok, *style));
            at_start = false;
        }
    }
    if !at_start {
        lines.push(line);
    }
    lines
}

pub fn render(text: &str, width: usize) -> Vec<MdLine> {
    let width = width.max(20);
    let mut r = Renderer {
        width,
        out: Vec::new(),
        inline: Vec::new(),
        bold: 0,
        italic: 0,
        strike: 0,
        link: 0,
        image: 0,
        heading: None,
        lists: Vec::new(),
        item_marker: None,
        quotes: Vec::new(),
        code: None,
        table: None,
        summary: false,
    };
    let opts = Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_GFM | Options::ENABLE_FOOTNOTES;
    for ev in Parser::new_ext(text, opts) {
        r.event(ev);
    }
    r.flush();
    while r.out.last().is_some_and(Vec::is_empty) {
        r.out.pop();
    }
    r.out
}

pub fn is_markdown(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "md" | "markdown" | "mdx"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[MdLine]) -> Vec<String> {
        lines.iter().map(|l| l.iter().map(|(t, _)| t.as_str()).collect::<String>().trim_end().to_string()).collect()
    }

    #[test]
    fn renders_common_markdown() {
        let md = "# Title\n\nSome **bold** and `code` text.\n\n- one\n- [x] done\n  1. nested\n\n> [!WARNING]\n> careful\n\n```rust\nfn main() {}\n```\n\n| A | B |\n|---|--:|\n| x | 10 |\n\n<div align=\"center\">\n\n**Centered**\n\n</div>\n";
        let out = plain(&render(md, 40));
        assert_eq!(out[0], "█ Title");
        assert!(out[1].starts_with("━━━"));
        assert!(out.contains(&"Some bold and  code  text.".to_string()), "{out:#?}");
        assert!(out.contains(&"• one".to_string()));
        assert!(out.contains(&"• ☑ done".to_string()));
        assert!(out.contains(&"    1. nested".to_string()) || out.contains(&"  1. nested".to_string()), "{out:#?}");
        assert!(out.contains(&"▌ ⚠ Warning".to_string()));
        assert!(out.contains(&"▌ careful".to_string()));
        assert!(out.iter().any(|l| l.contains("fn main() {}")));
        assert!(out.iter().any(|l| l.starts_with("┌")));
        assert!(out.contains(&"│ x │ 10 │".to_string()), "{out:#?}");
        assert!(out.contains(&"Centered".to_string()));
        assert!(!out.iter().any(|l| l.contains("<div")));
    }

    #[test]
    fn wraps_long_paragraphs() {
        let md = "word ".repeat(30);
        let out = plain(&render(&md, 24));
        assert!(out.len() > 5);
        assert!(out.iter().all(|l| l.width() <= 24));
    }
}
