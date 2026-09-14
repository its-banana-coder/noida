//! Lazily-loaded project file tree that respects .gitignore.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::theme;

#[derive(Clone)]
struct Entry {
    path: PathBuf,
    name: String,
    is_dir: bool,
}

pub struct Row {
    pub path: PathBuf,
    name: String,
    depth: usize,
    is_dir: bool,
}

pub enum Activate {
    None,
    Open(PathBuf),
}

pub struct Tree {
    root: PathBuf,
    expanded: HashSet<PathBuf>,
    cache: HashMap<PathBuf, Vec<Entry>>,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub offset: usize,
    height: usize,
}

impl Tree {
    pub fn new(root: PathBuf) -> Self {
        let mut t = Self {
            expanded: HashSet::from([root.clone()]),
            root,
            cache: HashMap::new(),
            rows: Vec::new(),
            selected: 0,
            offset: 0,
            height: 20,
        };
        t.rebuild();
        t
    }

    fn list(&mut self, dir: &Path) -> Vec<Entry> {
        if let Some(e) = self.cache.get(dir) {
            return e.clone();
        }
        let mut entries: Vec<Entry> = WalkBuilder::new(dir)
            .max_depth(Some(1))
            .hidden(false)
            .require_git(false)
            .filter_entry(|e| e.file_name() != ".git")
            .build()
            .filter_map(Result::ok)
            .filter(|e| e.depth() == 1)
            .map(|e| Entry {
                is_dir: e.file_type().is_some_and(|t| t.is_dir()),
                name: e.file_name().to_string_lossy().into_owned(),
                path: e.into_path(),
            })
            .collect();
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.cache.insert(dir.to_path_buf(), entries.clone());
        entries
    }

    pub fn rebuild(&mut self) {
        let keep = self.rows.get(self.selected).map(|r| r.path.clone());
        let mut rows = Vec::new();
        let root = self.root.clone();
        self.push_children(&root, 0, &mut rows);
        self.rows = rows;
        if let Some(p) = keep {
            self.select_path(&p);
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    fn push_children(&mut self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        for e in self.list(dir) {
            let expand = e.is_dir && self.expanded.contains(&e.path);
            rows.push(Row {
                path: e.path.clone(),
                name: e.name,
                depth,
                is_dir: e.is_dir,
            });
            if expand {
                self.push_children(&e.path, depth + 1, rows);
            }
        }
    }

    /// Re-read directories from disk (agents create and delete files).
    pub fn refresh(&mut self) {
        self.cache.clear();
        self.expanded.retain(|p| p.is_dir());
        self.rebuild();
    }

    fn select_path(&mut self, path: &Path) -> bool {
        if let Some(i) = self.rows.iter().position(|r| r.path == path) {
            self.selected = i;
            true
        } else {
            false
        }
    }

    /// Expand every ancestor of `path` and select it.
    pub fn reveal(&mut self, path: &Path) {
        let Ok(rel) = path.strip_prefix(&self.root) else { return };
        let mut cur = self.root.clone();
        let comps: Vec<_> = rel.components().collect();
        for c in &comps[..comps.len().saturating_sub(1)] {
            cur.push(c);
            self.expanded.insert(cur.clone());
        }
        self.cache.clear();
        self.rebuild();
        if self.select_path(path) {
            self.center();
        }
    }

    fn center(&mut self) {
        if self.selected < self.offset || self.selected >= self.offset + self.height {
            self.offset = self.selected.saturating_sub(self.height / 2);
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        let max = self.rows.len().saturating_sub(1) as isize;
        self.selected = (self.selected as isize + delta).clamp(0, max) as usize;
    }

    pub fn activate(&mut self) -> Activate {
        let Some(row) = self.rows.get(self.selected) else { return Activate::None };
        if row.is_dir {
            let p = row.path.clone();
            if !self.expanded.remove(&p) {
                self.expanded.insert(p);
            }
            self.rebuild();
            Activate::None
        } else {
            Activate::Open(row.path.clone())
        }
    }

    pub fn expand(&mut self) -> Activate {
        match self.rows.get(self.selected) {
            Some(r) if r.is_dir && self.expanded.contains(&r.path) => {
                self.move_by(1);
                Activate::None
            }
            _ => self.activate(),
        }
    }

    pub fn collapse(&mut self) {
        let Some(row) = self.rows.get(self.selected) else { return };
        if row.is_dir && self.expanded.contains(&row.path) {
            let p = row.path.clone();
            self.expanded.remove(&p);
            self.rebuild();
        } else if let Some(parent) = row.path.parent().map(Path::to_path_buf) {
            if parent != self.root {
                self.select_path(&parent);
            }
        }
    }

    pub fn scroll(&mut self, delta: isize) {
        let max = self.rows.len().saturating_sub(self.height) as isize;
        self.offset = (self.offset as isize + delta).clamp(0, max.max(0)) as usize;
    }

    /// Row index at a screen y inside the tree area.
    pub fn row_at(&self, area: Rect, y: u16) -> Option<usize> {
        let i = self.offset + (y.checked_sub(area.y)? as usize);
        (i < self.rows.len()).then_some(i)
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, focused: bool, open: Option<&Path>) {
        self.height = area.height as usize;
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + self.height {
            self.offset = self.selected + 1 - self.height;
        }
        for (i, row) in self.rows.iter().enumerate().skip(self.offset).take(self.height) {
            let y = area.y + (i - self.offset) as u16;
            let mut style = Style::default().fg(if row.is_dir { theme::DIR } else { theme::FG });
            if Some(row.path.as_path()) == open {
                style = style.fg(theme::ACCENT).add_modifier(Modifier::BOLD);
            }
            if i == self.selected {
                style = style.bg(if focused { theme::SELECT } else { theme::SELECT_DIM });
                buf.set_style(Rect::new(area.x, y, area.width, 1), style);
            }
            let icon = match (row.is_dir, self.expanded.contains(&row.path)) {
                (true, true) => "▾ ",
                (true, false) => "▸ ",
                _ => "  ",
            };
            let line = format!("{}{}{}", "  ".repeat(row.depth), icon, row.name);
            buf.set_stringn(area.x, y, line, area.width as usize, style);
        }
        if self.rows.is_empty() {
            buf.set_string(area.x, area.y, "(empty)", Style::default().fg(Color::DarkGray));
        }
    }
}
