//! Editor groups: the editor area can be split into two side-by-side groups.
//!
//! Documents stay shared in `App::docs`; a group only remembers which doc it
//! shows. A `Doc` owns its cursors, scroll and last render area, so the same
//! doc shown in both groups shares that state. Splitting therefore prefers to
//! show a different (most recently used) doc in the new group, and falls back
//! to the same doc only when nothing else is open. In that case the inactive
//! group is rendered first and the focused group last, so the doc's stored
//! layout (used for mouse mapping and the cursor) always belongs to the
//! focused group; a click in the other group then only moves focus.

use std::path::PathBuf;

use super::{App, Focus};

pub const MAX_GROUPS: usize = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Group {
    /// Index into `App::docs` (meaningless while no doc is open).
    pub active: usize,
}

/// Fix up group indices after `docs.remove(removed)`; `len` is the new doc count.
/// Mirrors the single-editor rule: the next tab to the right takes the closed
/// one's place, or the previous one when the last tab closed.
pub fn on_doc_removed(groups: &mut Vec<Group>, active_group: &mut usize, removed: usize, len: usize) {
    for g in groups.iter_mut() {
        if g.active > removed || g.active >= len {
            g.active = g.active.saturating_sub(1);
        }
        g.active = g.active.min(len.saturating_sub(1));
    }
    if len == 0 {
        groups.truncate(1);
        *active_group = 0;
    }
}

/// Fix up group indices after a doc moved from `from` to `to` (remove + insert).
pub fn on_doc_moved(groups: &mut [Group], from: usize, to: usize) {
    for g in groups.iter_mut() {
        let i = g.active;
        g.active = if i == from {
            to
        } else if from < to && i > from && i <= to {
            i - 1
        } else if to < from && i >= to && i < from {
            i + 1
        } else {
            i
        };
    }
}

/// The most recently used doc other than `current`, by index.
pub fn recent_other(docs: &[PathBuf], mru: &[PathBuf], current: usize) -> Option<usize> {
    let cur = docs.get(current);
    mru.iter()
        .rev()
        .filter(|p| Some(*p) != cur)
        .find_map(|p| docs.iter().position(|d| d == p))
        .or_else(|| (0..docs.len()).rev().find(|&i| i != current))
}

impl App {
    pub(super) fn active_doc(&self) -> usize {
        self.groups[self.active_group].active
    }

    pub(super) fn set_active_doc(&mut self, i: usize) {
        let g = self.active_group;
        self.groups[g].active = i;
    }

    /// Whether doc `i` is visible in a group other than the focused one.
    pub(super) fn shown_elsewhere(&self, i: usize) -> bool {
        self.groups.iter().enumerate().any(|(g, grp)| g != self.active_group && grp.active == i)
    }

    fn doc_paths(&self) -> Vec<PathBuf> {
        self.docs.iter().map(|d| d.path.clone()).collect()
    }

    fn after_group_change(&mut self) {
        self.view = None;
        self.popup = None;
        self.focus = Focus::Editor;
        if let Some(p) = self.doc().map(|d| d.path.clone()) {
            self.tree.reveal(&p);
        }
        self.refresh_marks();
    }

    pub(super) fn split_editor(&mut self) {
        if self.groups.len() >= MAX_GROUPS {
            self.active_group = 1 - self.active_group;
            self.after_group_change();
            return self.info("the editor is already split (at most two groups)");
        }
        let current = self.active_doc();
        let shown = recent_other(&self.doc_paths(), &self.mru, current).unwrap_or(current);
        self.groups.push(Group { active: shown });
        self.active_group = self.groups.len() - 1;
        self.after_group_change();
    }

    pub(super) fn focus_other_group(&mut self) {
        if self.groups.len() < 2 {
            return self.info("the editor is not split");
        }
        self.active_group = 1 - self.active_group;
        self.after_group_change();
    }

    pub(super) fn close_group(&mut self) {
        if self.groups.len() < 2 {
            return self.info("only one editor group");
        }
        self.groups.remove(self.active_group);
        self.active_group = 0;
        self.after_group_change();
    }

    /// Show the focused tab in the other group (splitting first if needed) and
    /// let the source group fall back to its most recently used other doc.
    pub(super) fn move_tab_to_other_group(&mut self) {
        if self.docs.is_empty() {
            return self.info("open a file first");
        }
        let doc = self.active_doc();
        if self.groups.len() < 2 {
            self.groups.push(Group { active: doc });
        }
        let from = self.active_group;
        if let Some(other) = recent_other(&self.doc_paths(), &self.mru, doc) {
            self.groups[from].active = other;
        }
        self.active_group = 1 - from;
        self.groups[self.active_group].active = doc;
        self.after_group_change();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(active: &[usize]) -> Vec<Group> {
        active.iter().map(|&a| Group { active: a }).collect()
    }

    #[test]
    fn remove_before_both_groups_shifts() {
        let mut groups = g(&[2, 3]);
        let mut ag = 1;
        on_doc_removed(&mut groups, &mut ag, 0, 3);
        assert_eq!(groups, g(&[1, 2]));
        assert_eq!(ag, 1);
    }

    #[test]
    fn remove_shown_doc_takes_right_neighbour_or_last() {
        let mut groups = g(&[1, 3]);
        let mut ag = 0;
        // docs were [0,1,2,3]; remove 1 → group 0 shows the old doc 2 (now 1).
        on_doc_removed(&mut groups, &mut ag, 1, 3);
        assert_eq!(groups, g(&[1, 2]));
        // remove the last doc (index 2) → group 1 falls back to index 1.
        on_doc_removed(&mut groups, &mut ag, 2, 2);
        assert_eq!(groups, g(&[1, 1]));
        assert_eq!(ag, 0);
    }

    #[test]
    fn remove_after_is_untouched() {
        let mut groups = g(&[0, 1]);
        let mut ag = 1;
        on_doc_removed(&mut groups, &mut ag, 3, 3);
        assert_eq!(groups, g(&[0, 1]));
    }

    #[test]
    fn removing_everything_collapses_groups() {
        let mut groups = g(&[0, 0]);
        let mut ag = 1;
        on_doc_removed(&mut groups, &mut ag, 0, 0);
        assert_eq!(groups, g(&[0]));
        assert_eq!(ag, 0);
    }

    #[test]
    fn move_fixes_both_groups() {
        // Pinning doc 3 moves it to the front: [a,b,c,d] → [d,a,b,c].
        let mut groups = g(&[3, 1]);
        on_doc_moved(&mut groups, 3, 0);
        assert_eq!(groups, g(&[0, 2]));
        // Unpinning doc 0 moves it back to 2: [d,a,b,c] → [a,b,d,c].
        let mut groups = g(&[0, 2]);
        on_doc_moved(&mut groups, 0, 2);
        assert_eq!(groups, g(&[2, 1]));
        let mut groups = g(&[3]);
        on_doc_moved(&mut groups, 0, 2);
        assert_eq!(groups, g(&[3]));
    }

    #[test]
    fn recent_other_prefers_mru() {
        let docs: Vec<PathBuf> = ["a", "b", "c"].iter().map(PathBuf::from).collect();
        let mru: Vec<PathBuf> = ["c", "a", "b"].iter().map(PathBuf::from).collect();
        assert_eq!(recent_other(&docs, &mru, 1), Some(0));
        assert_eq!(recent_other(&docs, &mru, 0), Some(1));
        assert_eq!(recent_other(&docs, &[], 2), Some(1));
        assert_eq!(recent_other(&docs[..1], &mru, 0), None);
        assert_eq!(recent_other(&[], &mru, 0), None);
    }
}
