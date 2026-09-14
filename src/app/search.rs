//! Workspace search and replace (results grouped by file, preview before
//! replacing, per-match exclusion).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use regex::Regex;

use super::views::ViewResult;
use crate::events::Bg;
use crate::theme;

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Opts {
    pub case: bool,
    pub word: bool,
    pub regex: bool,
}

#[derive(Clone, Debug)]
pub struct Hit {
    /// 0-based line; char columns.
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct FileHits {
    pub path: PathBuf,
    pub hits: Vec<Hit>,
}

pub struct SearchResults {
    pub generation: u64,
    pub files: Vec<FileHits>,
    pub truncated: bool,
    pub error: Option<String>,
}

pub fn build_regex(query: &str, opts: Opts) -> Result<Regex, String> {
    let mut pat = if opts.regex { query.to_string() } else { regex::escape(query) };
    if opts.word {
        pat = format!(r"\b(?:{pat})\b");
    }
    if !opts.case && !query.chars().any(char::is_uppercase) {
        pat = format!("(?i){pat}");
    }
    Regex::new(&pat).map_err(|e| e.to_string().lines().last().unwrap_or("invalid regex").to_string())
}

pub fn run(root: &Path, query: &str, opts: Opts, include: &str, limit: usize) -> (Vec<FileHits>, bool, Option<String>) {
    let re = match build_regex(query, opts) {
        Ok(re) => re,
        Err(e) => return (Vec::new(), false, Some(e)),
    };
    let mut walk = WalkBuilder::new(root);
    walk.hidden(false).require_git(false).filter_entry(|e| e.file_name() != ".git");
    let globs: Vec<&str> = include.split(',').map(str::trim).filter(|g| !g.is_empty()).collect();
    if !globs.is_empty() {
        let mut ob = OverrideBuilder::new(root);
        for g in globs {
            if ob.add(g).is_err() {
                return (Vec::new(), false, Some(format!("bad glob: {g}")));
            }
        }
        if let Ok(o) = ob.build() {
            walk.overrides(o);
        }
    }
    let mut files = Vec::new();
    let mut total = 0;
    for entry in walk.build().filter_map(Result::ok) {
        if !entry.file_type().is_some_and(|t| t.is_file()) || entry.metadata().map_or(true, |m| m.len() > 4_000_000) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
        let mut hits = Vec::new();
        for (i, line) in text.lines().enumerate() {
            for m in re.find_iter(line) {
                if m.start() == m.end() {
                    continue;
                }
                let start = line[..m.start()].chars().count();
                hits.push(Hit { line: i, start, end: start + line[m.start()..m.end()].chars().count(), text: line.to_string() });
                total += 1;
                if total >= limit {
                    files.push(FileHits { path: entry.path().to_path_buf(), hits });
                    return (files, true, None);
                }
            }
        }
        if !hits.is_empty() {
            files.push(FileHits { path: entry.path().to_path_buf(), hits });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    (files, false, None)
}

/// Replace all non-excluded hits in one file. Returns the number replaced.
pub fn replace_in_file(path: &Path, hits: &[&Hit], re: &Regex, replacement: &str, regex_mode: bool) -> std::io::Result<usize> {
    let text = std::fs::read_to_string(path)?;
    let ends_newline = text.ends_with('\n');
    let crlf = text.contains("\r\n");
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let mut count = 0;
    let mut sorted: Vec<&&Hit> = hits.iter().collect();
    sorted.sort_by(|a, b| (b.line, b.start).cmp(&(a.line, a.start)));
    for h in sorted {
        let Some(line) = lines.get_mut(h.line) else { continue };
        // Skip hits whose text changed on disk since the search.
        if *line != h.text {
            continue;
        }
        let sb = line.char_indices().nth(h.start).map_or(line.len(), |(b, _)| b);
        let eb = line.char_indices().nth(h.end).map_or(line.len(), |(b, _)| b);
        let matched = &line[sb..eb];
        let new = if regex_mode { re.replace(matched, replacement).into_owned() } else { replacement.to_string() };
        line.replace_range(sb..eb, &new);
        count += 1;
    }
    let mut out = lines.join(if crlf { "\r\n" } else { "\n" });
    if ends_newline {
        out.push_str(if crlf { "\r\n" } else { "\n" });
    }
    std::fs::write(path, out)?;
    Ok(count)
}

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Query,
    Replace,
    Include,
    Results,
}

enum Row {
    File(usize),
    Hit(usize, usize),
}

pub struct SearchView {
    root: PathBuf,
    tx: Sender<Bg>,
    pub query: String,
    replace: String,
    include: String,
    opts: Opts,
    field: Field,
    files: Vec<FileHits>,
    excluded: HashSet<(usize, usize)>,
    rows: Vec<Row>,
    selected: usize,
    scroll: usize,
    height: usize,
    generation: u64,
    searching: bool,
    truncated: bool,
    error: Option<String>,
    dirty_at: Option<Instant>,
    confirm: Option<Instant>,
    area: Rect,
}

impl SearchView {
    pub fn new(root: PathBuf, tx: Sender<Bg>, query: String, replace_mode: bool) -> Self {
        let mut v = Self {
            root,
            tx,
            query,
            replace: String::new(),
            include: String::new(),
            opts: Opts::default(),
            field: if replace_mode { Field::Replace } else { Field::Query },
            files: Vec::new(),
            excluded: HashSet::new(),
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            height: 10,
            generation: 0,
            searching: false,
            truncated: false,
            error: None,
            dirty_at: None,
            confirm: None,
            area: Rect::default(),
        };
        if !v.query.is_empty() {
            v.start();
        }
        v
    }

    fn start(&mut self) {
        self.generation += 1;
        self.dirty_at = None;
        if self.query.is_empty() {
            self.files.clear();
            self.rebuild_rows();
            return;
        }
        self.searching = true;
        let (root, query, opts, include, tx, generation) = (self.root.clone(), self.query.clone(), self.opts, self.include.clone(), self.tx.clone(), self.generation);
        std::thread::spawn(move || {
            let (files, truncated, error) = run(&root, &query, opts, &include, 20_000);
            let _ = tx.send(Bg::Search(SearchResults { generation, files, truncated, error }));
        });
    }

    /// Debounced re-search while typing.
    pub fn tick(&mut self) -> bool {
        if self.dirty_at.is_some_and(|t| t.elapsed() > Duration::from_millis(250)) {
            self.start();
            return true;
        }
        false
    }

    pub fn on_results(&mut self, r: SearchResults) {
        if r.generation != self.generation {
            return;
        }
        self.searching = false;
        self.files = r.files;
        self.truncated = r.truncated;
        self.error = r.error;
        self.excluded.clear();
        self.selected = 0;
        self.scroll = 0;
        self.rebuild_rows();
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        for (fi, f) in self.files.iter().enumerate() {
            self.rows.push(Row::File(fi));
            for hi in 0..f.hits.len() {
                self.rows.push(Row::Hit(fi, hi));
            }
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    fn total(&self) -> usize {
        self.files.iter().map(|f| f.hits.len()).sum()
    }

    fn edit(&mut self) {
        self.dirty_at = Some(Instant::now());
        self.confirm = None;
    }

    fn replace_all(&mut self) -> ViewResult {
        let re = match build_regex(&self.query, self.opts) {
            Ok(r) => r,
            Err(e) => return ViewResult::Message(e, true),
        };
        let included = self.total() - self.excluded.len();
        if included == 0 {
            return ViewResult::Message("nothing to replace".into(), true);
        }
        if !self.confirm.is_some_and(|t| t.elapsed() < Duration::from_secs(3)) {
            self.confirm = Some(Instant::now());
            let files = self.files.iter().enumerate().filter(|(fi, f)| (0..f.hits.len()).any(|hi| !self.excluded.contains(&(*fi, hi)))).count();
            return ViewResult::Message(format!("replace {included} matches in {files} files with \"{}\"? press Alt+a again", self.replace), true);
        }
        self.confirm = None;
        let (mut n, mut errors) = (0, 0);
        for (fi, f) in self.files.iter().enumerate() {
            let hits: Vec<&Hit> = f.hits.iter().enumerate().filter(|(hi, _)| !self.excluded.contains(&(fi, *hi))).map(|(_, h)| h).collect();
            if hits.is_empty() {
                continue;
            }
            match replace_in_file(&f.path, &hits, &re, &self.replace, self.opts.regex) {
                Ok(c) => n += c,
                Err(_) => errors += 1,
            }
        }
        self.start();
        let msg = if errors > 0 { format!("replaced {n} matches ({errors} files failed)") } else { format!("replaced {n} matches") };
        ViewResult::Changed(msg)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ViewResult {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return ViewResult::Close,
            KeyCode::Char('c') if alt => {
                self.opts.case = !self.opts.case;
                self.start();
            }
            KeyCode::Char('w') if alt => {
                self.opts.word = !self.opts.word;
                self.start();
            }
            KeyCode::Char('r') if alt => {
                self.opts.regex = !self.opts.regex;
                self.start();
            }
            KeyCode::Char('a') if alt => return self.replace_all(),
            KeyCode::Tab => {
                self.field = match self.field {
                    Field::Query => Field::Replace,
                    Field::Replace => Field::Include,
                    Field::Include => Field::Results,
                    Field::Results => Field::Query,
                }
            }
            KeyCode::BackTab => {
                self.field = match self.field {
                    Field::Query => Field::Results,
                    Field::Replace => Field::Query,
                    Field::Include => Field::Replace,
                    Field::Results => Field::Include,
                }
            }
            KeyCode::Down if self.field != Field::Results => self.field = Field::Results,
            KeyCode::Down | KeyCode::Char('j') if self.field == Field::Results => self.selected = (self.selected + 1).min(self.rows.len().saturating_sub(1)),
            KeyCode::Up | KeyCode::Char('k') if self.field == Field::Results => {
                if self.selected == 0 {
                    self.field = Field::Include;
                }
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::PageDown => self.selected = (self.selected + self.height).min(self.rows.len().saturating_sub(1)),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(self.height),
            KeyCode::Char('x') | KeyCode::Delete if self.field == Field::Results => match self.rows.get(self.selected) {
                Some(Row::Hit(fi, hi)) => {
                    let k = (*fi, *hi);
                    if !self.excluded.remove(&k) {
                        self.excluded.insert(k);
                    }
                    self.selected = (self.selected + 1).min(self.rows.len().saturating_sub(1));
                }
                Some(Row::File(fi)) => {
                    let fi = *fi;
                    let n = self.files[fi].hits.len();
                    let all = (0..n).all(|hi| self.excluded.contains(&(fi, hi)));
                    for hi in 0..n {
                        if all {
                            self.excluded.remove(&(fi, hi));
                        } else {
                            self.excluded.insert((fi, hi));
                        }
                    }
                }
                None => {}
            },
            KeyCode::Enter => {
                if self.field != Field::Results {
                    self.start();
                    self.field = Field::Results;
                } else {
                    match self.rows.get(self.selected) {
                        Some(Row::Hit(fi, hi)) => {
                            let h = &self.files[*fi].hits[*hi];
                            return ViewResult::OpenAt(self.files[*fi].path.clone(), h.line + 1, h.start + 1);
                        }
                        Some(Row::File(fi)) => return ViewResult::Open(self.files[*fi].path.clone(), 1),
                        None => {}
                    }
                }
            }
            KeyCode::Backspace if self.field != Field::Results => {
                self.current_field().pop();
                self.edit();
            }
            KeyCode::Char('u') if ctrl && self.field != Field::Results => {
                self.current_field().clear();
                self.edit();
            }
            KeyCode::Char(c) if !ctrl && !alt && self.field != Field::Results => {
                self.current_field().push(c);
                self.edit();
            }
            _ => {}
        }
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + self.height {
            self.scroll = self.selected + 1 - self.height;
        }
        ViewResult::None
    }

    fn current_field(&mut self) -> &mut String {
        match self.field {
            Field::Replace => &mut self.replace,
            Field::Include => &mut self.include,
            _ => &mut self.query,
        }
    }

    pub fn paste(&mut self, text: &str) {
        if self.field != Field::Results {
            self.current_field().push_str(text.lines().next().unwrap_or(""));
            self.edit();
        }
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = (self.scroll as isize + delta).clamp(0, self.rows.len().saturating_sub(1) as isize) as usize;
    }

    pub fn click(&mut self, y: u16) -> ViewResult {
        let top = self.area.y + 4;
        if y < top {
            self.field = match y.saturating_sub(self.area.y) {
                0 => Field::Query,
                1 => Field::Replace,
                _ => Field::Include,
            };
            return ViewResult::None;
        }
        let row = self.scroll + (y - top) as usize;
        if row < self.rows.len() {
            let again = row == self.selected && self.field == Field::Results;
            self.selected = row;
            self.field = Field::Results;
            if again {
                return self.handle_key(KeyEvent::from(KeyCode::Enter));
            }
        }
        ViewResult::None
    }

    pub fn status(&self) -> String {
        "Tab next field · Enter search/open · x exclude · Alt+a replace all · Alt+c case · Alt+w word · Alt+r regex · Esc close".into()
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
        self.area = area;
        if area.height < 6 || area.width < 20 {
            return None;
        }
        let label_w = 9u16;
        let fields = [(Field::Query, "search", &self.query), (Field::Replace, "replace", &self.replace), (Field::Include, "include", &self.include)];
        let mut cursor = None;
        for (i, (f, label, value)) in fields.iter().enumerate() {
            let y = area.y + i as u16;
            let active = self.field == *f;
            let bg = Style::default().bg(if active { theme::SELECT() } else { theme::STATUS_BG() });
            buf.set_style(Rect::new(area.x, y, area.width, 1), bg);
            buf.set_string(area.x + 1, y, format!("{label:>7}"), bg.fg(theme::DIM()));
            let w = area.width.saturating_sub(label_w + 18) as usize;
            let n = value.chars().count();
            let shown: String = value.chars().skip(n.saturating_sub(w)).collect();
            buf.set_string(area.x + label_w, y, &shown, bg.fg(theme::FG()));
            if value.is_empty() && *f == Field::Include {
                buf.set_string(area.x + label_w, y, "e.g. src/**/*.rs, !*.lock", bg.fg(theme::BORDER()));
            }
            if active {
                cursor = Some((area.x + label_w + shown.chars().count() as u16, y));
            }
        }
        let toggles = [("Aa", self.opts.case), ("W", self.opts.word), (".*", self.opts.regex)];
        let mut x = area.x + area.width.saturating_sub(10);
        for (t, on) in toggles {
            let style = if on { Style::default().fg(theme::HINT_FG()).bg(theme::BORDER_FOCUS()) } else { Style::default().fg(theme::DIM()).bg(theme::STATUS_BG()) };
            buf.set_string(x, area.y, t, style);
            x += t.chars().count() as u16 + 1;
        }
        let summary = if let Some(e) = &self.error {
            (e.clone(), theme::ERROR())
        } else if self.searching {
            ("searching…".into(), theme::DIM())
        } else if self.query.is_empty() {
            ("type to search the workspace".into(), theme::DIM())
        } else {
            let excluded = if self.excluded.is_empty() { String::new() } else { format!(" · {} excluded", self.excluded.len()) };
            let more = if self.truncated { " (first 20000)" } else { "" };
            (format!("{} results in {} files{more}{excluded}", self.total(), self.files.len()), theme::ACCENT())
        };
        buf.set_stringn(area.x + 1, area.y + 3, &summary.0, area.width as usize - 2, Style::default().fg(summary.1));

        let top = area.y + 4;
        self.height = (area.height - 4) as usize;
        let preview = !self.replace.is_empty();
        let re = if preview && self.opts.regex { build_regex(&self.query, self.opts).ok() } else { None };
        for (i, row) in self.rows.iter().enumerate().skip(self.scroll).take(self.height) {
            let y = top + (i - self.scroll) as u16;
            let selected = i == self.selected && self.field == Field::Results;
            if selected {
                buf.set_style(Rect::new(area.x, y, area.width, 1), Style::default().bg(theme::SELECT()));
            }
            match row {
                Row::File(fi) => {
                    let f = &self.files[*fi];
                    let rel = crate::refs::relative(&self.root, &f.path).into_owned();
                    let (x, _) = buf.set_stringn(area.x + 1, y, &rel, area.width as usize - 8, Style::default().fg(theme::FG()).add_modifier(Modifier::BOLD));
                    buf.set_string(x + 1, y, format!("{}", f.hits.len()), Style::default().fg(theme::DIM()));
                }
                Row::Hit(fi, hi) => {
                    let h = &self.files[*fi].hits[*hi];
                    let excluded = self.excluded.contains(&(*fi, *hi));
                    let num = format!("{:>5} ", h.line + 1);
                    buf.set_string(area.x + 1, y, &num, Style::default().fg(theme::DIM()));
                    let chars: Vec<char> = h.text.chars().collect();
                    // Show some context before the match, trimmed of indentation.
                    let lead = chars.iter().take_while(|c| c.is_whitespace()).count();
                    let ctx_start = lead.max(h.start.saturating_sub(30));
                    let mut x = area.x + 7;
                    let end_x = area.x + area.width;
                    let plain = Style::default().fg(if excluded { theme::BORDER() } else { theme::FG() });
                    let before: String = chars[ctx_start..h.start.min(chars.len())].iter().collect();
                    x = buf.set_stringn(x, y, &before, end_x.saturating_sub(x) as usize, plain).0;
                    let matched: String = chars[h.start.min(chars.len())..h.end.min(chars.len())].iter().collect();
                    let mut m_style = Style::default().fg(theme::HINT_FG()).bg(theme::ACCENT());
                    if preview || excluded {
                        m_style = Style::default().fg(theme::ERROR()).add_modifier(Modifier::CROSSED_OUT);
                    }
                    x = buf.set_stringn(x, y, &matched, end_x.saturating_sub(x) as usize, m_style).0;
                    if preview && !excluded {
                        let new = match &re {
                            Some(re) => re.replace(&matched, self.replace.as_str()).into_owned(),
                            None => self.replace.clone(),
                        };
                        x = buf.set_stringn(x, y, &new, end_x.saturating_sub(x) as usize, Style::default().fg(theme::HINT_FG()).bg(theme::ADDED())).0;
                    }
                    let after: String = chars[h.end.min(chars.len())..].iter().collect();
                    buf.set_stringn(x, y, &after, end_x.saturating_sub(x) as usize, plain);
                }
            }
        }
        if self.field == Field::Results { None } else { cursor }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_and_replace_files() {
        let dir = std::env::temp_dir().join(format!("noida-wsearch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "let foo = 1;\nfoo(foo);\n").unwrap();
        std::fs::write(dir.join("b.txt"), "Foo food\n").unwrap();
        let (files, truncated, err) = run(&dir, "foo", Opts { word: true, ..Default::default() }, "", 100);
        assert!(!truncated && err.is_none());
        let counts: Vec<usize> = files.iter().map(|f| f.hits.len()).collect();
        assert_eq!(counts, vec![1, 3]);
        let (files, _, _) = run(&dir, "foo", Opts::default(), "src/**", 100);
        assert_eq!(files.len(), 1);
        // Replace all but the second hit in a.rs.
        let a = &files[0];
        let hits: Vec<&Hit> = a.hits.iter().enumerate().filter(|(i, _)| *i != 1).map(|(_, h)| h).collect();
        let re = build_regex("foo", Opts::default()).unwrap();
        assert_eq!(replace_in_file(&a.path, &hits, &re, "bar", false).unwrap(), 2);
        assert_eq!(std::fs::read_to_string(dir.join("src/a.rs")).unwrap(), "let bar = 1;\nfoo(bar);\n");
        std::fs::remove_dir_all(&dir).ok();
    }
}
