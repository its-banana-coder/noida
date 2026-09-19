//! Thin wrapper over the `git` CLI: status, diffs, hunk staging/reverting,
//! commit log and file history, branches and worktrees.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

#[derive(Clone, Debug)]
pub struct Repo {
    pub root: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileState {
    Modified,
    Untracked,
    Deleted,
    /// All changes are staged ("accepted").
    Staged,
    Conflict,
}

impl FileState {
    pub fn glyph(self) -> &'static str {
        match self {
            FileState::Modified => "M",
            FileState::Untracked => "U",
            FileState::Deleted => "D",
            FileState::Staged => "✓",
            FileState::Conflict => "!",
        }
    }
}

#[derive(Default, Debug)]
pub struct Status {
    pub branch: Option<String>,
    /// Absolute path → state.
    pub files: HashMap<PathBuf, FileState>,
}

impl Status {
    pub fn changed(&self) -> usize {
        self.files.values().filter(|s| **s != FileState::Staged).count()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineMark {
    Added,
    Modified,
    DeletedBelow,
}

#[derive(Clone, Debug)]
pub struct Hunk {
    pub header: String,
    pub new_start: usize,
    pub lines: Vec<String>,
}

impl Hunk {
    pub fn counts(&self) -> (usize, usize) {
        let add = self.lines.iter().filter(|l| l.starts_with('+')).count();
        let del = self.lines.iter().filter(|l| l.starts_with('-')).count();
        (add, del)
    }
}

#[derive(Clone, Debug)]
pub struct FileDiff {
    pub path: String,
    header: Vec<String>,
    pub hunks: Vec<Hunk>,
    pub untracked: bool,
    pub binary: bool,
}

impl FileDiff {
    pub fn renamed_from(&self) -> Option<&str> {
        self.header.iter().find_map(|l| l.strip_prefix("rename from "))
    }

    pub fn counts(&self) -> (usize, usize) {
        self.hunks.iter().fold((0, 0), |(a, d), h| {
            let (ha, hd) = h.counts();
            (a + ha, d + hd)
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum HunkOp {
    Stage,
    Revert,
    /// Move a staged hunk back out of the index.
    Unstage,
}

/// One entry of `git log`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    /// Relative date, e.g. "3 days ago".
    pub date: String,
    /// Path of the file at this commit (file history only; follows renames).
    pub path: Option<String>,
}

/// A commit's metadata header and its diff.
pub struct CommitShow {
    pub meta: Vec<String>,
    pub files: Vec<FileDiff>,
}

const LOG_FORMAT: &str = "--format=%x1e%H%x1f%h%x1f%s%x1f%an%x1f%ar";
pub const LOG_LIMIT: usize = 300;

impl Repo {
    pub fn discover(path: &Path) -> Option<Repo> {
        let out = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "--show-toplevel"])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let root = String::from_utf8(out.stdout).ok()?.trim().to_string();
        Some(Repo { root: PathBuf::from(root) })
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new("git");
        c.arg("-C").arg(&self.root).args(["-c", "core.quotepath=off"]);
        c.stdin(Stdio::null());
        c
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        let out = self.cmd().args(args).output().context("failed to run git")?;
        if !out.status.success() {
            bail!("git {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn git_stdin(&self, args: &[&str], input: &str) -> Result<String> {
        let mut child = self
            .cmd()
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to run git")?;
        child.stdin.take().unwrap().write_all(input.as_bytes())?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            bail!("git {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn branch(&self) -> Option<String> {
        let b = self.git(&["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
        Some(b.trim().to_string())
    }

    pub fn status(&self) -> Result<Status> {
        let raw = self.git(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
        let mut st = Status { branch: self.branch(), files: HashMap::new() };
        let mut parts = raw.split('\0');
        while let Some(entry) = parts.next() {
            if entry.len() < 4 {
                continue;
            }
            let (x, y) = (entry.as_bytes()[0], entry.as_bytes()[1]);
            let path = &entry[3..];
            if x == b'R' || x == b'C' {
                parts.next(); // original path
            }
            let state = match (x, y) {
                (b'?', b'?') => FileState::Untracked,
                (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => FileState::Conflict,
                (_, b'M') => FileState::Modified,
                (_, b'D') => FileState::Deleted,
                (b'!', _) => continue,
                (_, b' ') => FileState::Staged,
                _ => FileState::Modified,
            };
            st.files.insert(self.root.join(path), state);
        }
        Ok(st)
    }

    /// Unstaged changes plus untracked files.
    pub fn diff(&self) -> Result<Vec<FileDiff>> {
        let raw = self.git(&["diff", "--no-color", "--no-ext-diff", "-U3"])?;
        let mut files = parse_diff(&raw);
        let others = self.git(&["ls-files", "--others", "--exclude-standard", "-z"])?;
        for path in others.split('\0').filter(|p| !p.is_empty()) {
            files.push(untracked_diff(&self.root, path));
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    /// Changes staged in the index (`git diff --cached`).
    pub fn diff_staged(&self) -> Result<Vec<FileDiff>> {
        let raw = self.git(&["diff", "--cached", "--no-color", "--no-ext-diff", "-U3"])?;
        let mut files = parse_diff(&raw);
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    pub fn apply_hunk(&self, file: &FileDiff, hunk: usize, op: HunkOp) -> Result<()> {
        if file.untracked || file.binary {
            return match op {
                HunkOp::Stage => self.stage_file(&file.path),
                HunkOp::Revert => self.revert_file(file),
                HunkOp::Unstage => self.unstage_file(&file.path),
            };
        }
        let h = file.hunks.get(hunk).context("no such hunk")?;
        let mut patch = file.header.join("\n");
        patch.push('\n');
        patch.push_str(&h.header);
        patch.push('\n');
        for l in &h.lines {
            patch.push_str(l);
            patch.push('\n');
        }
        match op {
            HunkOp::Stage => self.git_stdin(&["apply", "--cached", "--whitespace=nowarn", "-"], &patch)?,
            HunkOp::Revert => self.git_stdin(&["apply", "-R", "--whitespace=nowarn", "-"], &patch)?,
            HunkOp::Unstage => self.git_stdin(&["apply", "--cached", "-R", "--whitespace=nowarn", "-"], &patch)?,
        };
        Ok(())
    }

    pub fn stage_file(&self, path: &str) -> Result<()> {
        self.git(&["add", "--", path]).map(drop)
    }

    /// Unstage a whole file, keeping the working tree as is.
    pub fn unstage_file(&self, path: &str) -> Result<()> {
        if self.git(&["restore", "--staged", "--", path]).is_ok() {
            return Ok(());
        }
        // Older git without `restore`, or a repo without commits yet.
        self.git(&["reset", "-q", "--", path]).or_else(|_| self.git(&["rm", "-q", "--cached", "--", path])).map(drop)
    }

    pub fn revert_file(&self, file: &FileDiff) -> Result<()> {
        if file.untracked {
            std::fs::remove_file(self.root.join(&file.path)).context("failed to delete file")?;
            Ok(())
        } else {
            self.git(&["checkout", "--", &file.path]).map(drop)
        }
    }

    pub fn stage_all(&self) -> Result<()> {
        self.git(&["add", "-A"]).map(drop)
    }

    pub fn line_marks(&self, file: &Path) -> HashMap<usize, LineMark> {
        let mut marks = HashMap::new();
        let Ok(rel) = file.strip_prefix(&self.root) else { return marks };
        let rel = rel.to_string_lossy();
        let tracked = self.git(&["ls-files", "--error-unmatch", "--", &rel]).is_ok();
        if !tracked {
            if let Ok(text) = std::fs::read_to_string(file) {
                for i in 0..text.lines().count() {
                    marks.insert(i, LineMark::Added);
                }
            }
            return marks;
        }
        let Ok(raw) = self.git(&["diff", "--no-color", "--no-ext-diff", "-U0", "--", &rel]) else { return marks };
        for line in raw.lines().filter(|l| l.starts_with("@@")) {
            let Some((old_len, new_start, new_len)) = parse_hunk_header(line) else { continue };
            if new_len == 0 {
                marks.insert(new_start.saturating_sub(1), LineMark::DeletedBelow);
            } else {
                let mark = if old_len == 0 { LineMark::Added } else { LineMark::Modified };
                for i in new_start..new_start + new_len {
                    marks.insert(i - 1, mark);
                }
            }
        }
        marks
    }

    pub fn branches(&self) -> Result<Vec<(String, bool)>> {
        let raw = self.git(&["branch", "--format=%(HEAD)%(refname:short)"])?;
        Ok(raw
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| (l[1..].to_string(), l.starts_with('*')))
            .collect())
    }

    pub fn switch(&self, branch: &str) -> Result<()> {
        self.git(&["switch", branch]).map(drop)
    }

    pub fn create_branch(&self, branch: &str) -> Result<()> {
        self.git(&["switch", "-c", branch]).map(drop)
    }

    /// Most recent commits on HEAD, newest first.
    pub fn log(&self, limit: usize) -> Result<Vec<Commit>> {
        let n = format!("-n{limit}");
        Ok(parse_log(&self.git(&["log", &n, LOG_FORMAT])?))
    }

    /// Commits touching `path` (relative to the root), following renames.
    pub fn file_history(&self, path: &str, limit: usize) -> Result<Vec<Commit>> {
        let n = format!("-n{limit}");
        Ok(parse_log(&self.git(&["log", "--follow", "--name-only", &n, LOG_FORMAT, "--", path])?))
    }

    /// Metadata and diff of a commit, limited to `paths` unless empty. Pass
    /// both names of a renamed file so the rename is detected.
    pub fn show(&self, hash: &str, paths: &[String]) -> Result<CommitShow> {
        let format = "--format=commit %H%nAuthor: %an <%ae>%nDate:   %ad%n%n%w(0,4,4)%B%x1e";
        let mut args = vec!["show", "--no-color", "--no-ext-diff", "-U3", "-M", "--diff-merges=first-parent", format, hash];
        if !paths.is_empty() {
            args.push("--");
            args.extend(paths.iter().map(String::as_str));
        }
        let raw = match self.git(&args) {
            Ok(raw) => raw,
            Err(_) => {
                // git < 2.31 has no --diff-merges.
                args.retain(|a| *a != "--diff-merges=first-parent");
                self.git(&args)?
            }
        };
        let (meta, diff) = raw.split_once('\x1e').unwrap_or((raw.as_str(), ""));
        let meta = meta.trim_end().lines().map(str::to_string).collect();
        Ok(CommitShow { meta, files: parse_diff(diff) })
    }

    pub fn head(&self) -> Result<String> {
        Ok(self.git(&["rev-parse", "HEAD"])?.trim().to_string())
    }

    pub fn merge_base(&self, a: &str, b: &str) -> Result<String> {
        Ok(self.git(&["merge-base", a, b])?.trim().to_string())
    }

    /// Lines added/deleted per file between `base` and the working tree
    /// (commits, staged and unstaged work), plus untracked files as all-added.
    /// `only` restricts to those repo-relative paths. `None` counts mean binary.
    pub fn numstat(&self, base: &str, only: Option<&[String]>) -> Result<Vec<(String, Option<(usize, usize)>)>> {
        if only.is_some_and(|o| o.is_empty()) {
            return Ok(Vec::new());
        }
        let paths: Vec<&str> = only.unwrap_or_default().iter().map(String::as_str).collect();
        let mut args = vec!["diff", "--numstat", "--no-renames", base, "--"];
        args.extend(&paths);
        let mut out = Vec::new();
        for line in self.git(&args)?.lines() {
            let mut parts = line.splitn(3, '\t');
            let (Some(add), Some(del), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
            out.push((path.to_string(), add.parse().ok().zip(del.parse().ok())));
        }
        let mut args = vec!["ls-files", "--others", "--exclude-standard", "-z", "--"];
        args.extend(&paths);
        for path in self.git(&args)?.split('\0').filter(|p| !p.is_empty()) {
            let bytes = std::fs::read(self.root.join(path)).unwrap_or_default();
            let binary = bytes[..bytes.len().min(8000)].contains(&0);
            let lines = String::from_utf8_lossy(&bytes).lines().count();
            out.push((path.to_string(), (!binary).then_some((lines, 0))));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    pub fn commit(&self, message: &str) -> Result<String> {
        let out = self.git(&["commit", "-m", message])?;
        Ok(out.lines().next().unwrap_or("").to_string())
    }

    // ---- worktrees ----

    fn common_dir(&self) -> Result<PathBuf> {
        let dir = self.git(&["rev-parse", "--git-common-dir"])?;
        let p = PathBuf::from(dir.trim());
        Ok(if p.is_absolute() { p } else { self.root.join(p) })
    }

    /// Create `.git/noida-worktrees/<name>` on a new `noida/<name>` branch.
    pub fn add_worktree(&self, name: &str) -> Result<(PathBuf, String)> {
        let base = self.common_dir()?.join("noida-worktrees");
        std::fs::create_dir_all(&base)?;
        let existing: Vec<String> = self.branches().unwrap_or_default().into_iter().map(|(b, _)| b).collect();
        let mut n = 1;
        let mut slug = sanitize(name);
        while existing.contains(&format!("noida/{slug}")) || base.join(&slug).exists() {
            n += 1;
            slug = format!("{}-{n}", sanitize(name));
        }
        let path = base.join(&slug);
        let branch = format!("noida/{slug}");
        self.git(&["worktree", "add", "-b", &branch, &path.to_string_lossy(), "HEAD"])?;
        Ok((path, branch))
    }

    /// Apply everything a worktree changed (commits and uncommitted work) to this checkout.
    pub fn apply_worktree(&self, wt: &Repo) -> Result<usize> {
        wt.git(&["add", "-A"])?;
        let base = self.git(&["rev-parse", "HEAD"])?;
        let patch = wt.git(&["diff", "--cached", "--binary", base.trim()])?;
        if patch.trim().is_empty() {
            return Ok(0);
        }
        let files = patch.lines().filter(|l| l.starts_with("diff --git")).count();
        if self.git_stdin(&["apply", "--whitespace=nowarn", "-"], &patch).is_err() {
            self.git_stdin(&["apply", "--3way", "--whitespace=nowarn", "-"], &patch)?;
        }
        Ok(files)
    }

    pub fn remove_worktree(&self, path: &Path, branch: &str) -> Result<()> {
        self.git(&["worktree", "remove", "--force", &path.to_string_lossy()])?;
        let _ = self.git(&["branch", "-D", branch]);
        Ok(())
    }
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "task".into() } else { s }
}

/// Parses "@@ -a,b +c,d @@" into (old_len, new_start, new_len).
fn parse_hunk_header(line: &str) -> Option<(usize, usize, usize)> {
    let mut it = line.split_whitespace().skip(1);
    let old = it.next()?.strip_prefix('-')?;
    let new = it.next()?.strip_prefix('+')?;
    let range = |r: &str| -> Option<(usize, usize)> {
        match r.split_once(',') {
            Some((s, l)) => Some((s.parse().ok()?, l.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let (_, old_len) = range(old)?;
    let (new_start, new_len) = range(new)?;
    Some((old_len, new_start, new_len))
}

pub fn parse_diff(raw: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    for line in raw.lines() {
        if line.starts_with("diff --git ") {
            let path = line.rsplit_once(" b/").map(|(_, p)| p.to_string()).unwrap_or_default();
            files.push(FileDiff { path, header: vec![line.to_string()], hunks: Vec::new(), untracked: false, binary: false });
            continue;
        }
        let Some(file) = files.last_mut() else { continue };
        if line.starts_with("@@") {
            let new_start = parse_hunk_header(line).map_or(1, |(_, s, _)| s);
            file.hunks.push(Hunk { header: line.to_string(), new_start, lines: Vec::new() });
        } else if let Some(h) = file.hunks.last_mut() {
            h.lines.push(line.to_string());
        } else {
            if let Some(p) = line.strip_prefix("+++ b/") {
                file.path = p.to_string();
            } else if line.starts_with("Binary files") {
                file.binary = true;
            }
            file.header.push(line.to_string());
        }
    }
    files
}

/// Parses `git log` output made with `LOG_FORMAT` (optionally `--name-only`).
pub fn parse_log(raw: &str) -> Vec<Commit> {
    raw.split('\x1e')
        .filter_map(|record| {
            let mut lines = record.lines();
            let mut fields = lines.next()?.split('\x1f').map(str::to_string);
            let commit = Commit {
                hash: fields.next()?,
                short: fields.next()?,
                subject: fields.next()?,
                author: fields.next()?,
                date: fields.next()?,
                path: lines.map(str::trim).find(|l| !l.is_empty()).map(str::to_string),
            };
            (!commit.hash.is_empty()).then_some(commit)
        })
        .collect()
}

fn untracked_diff(root: &Path, path: &str) -> FileDiff {
    let bytes = std::fs::read(root.join(path)).unwrap_or_default();
    let binary = bytes[..bytes.len().min(8000)].contains(&0);
    let mut hunks = Vec::new();
    if !binary {
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<String> = text.lines().take(5000).map(|l| format!("+{l}")).collect();
        hunks.push(Hunk { header: format!("@@ -0,0 +1,{} @@ new file", lines.len()), new_start: 1, lines });
    }
    FileDiff { path: path.to_string(), header: Vec::new(), hunks, untracked: true, binary }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempdir::Dir, Repo) {
        let dir = tempdir::Dir::new();
        let run = |args: &[&str]| {
            assert!(Command::new("git").arg("-C").arg(&dir.0).args(args).output().unwrap().status.success(), "{args:?}");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        // Git for Windows rewrites line endings on checkout by default, which
        // would make these fixtures come back as CRLF.
        run(&["config", "core.autocrlf", "false"]);
        std::fs::write(dir.0.join("a.txt"), "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        let repo = Repo::discover(&dir.0).unwrap();
        (dir, repo)
    }

    mod tempdir {
        pub struct Dir(pub std::path::PathBuf);
        impl Dir {
            pub fn new() -> Self {
                static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let p = std::env::temp_dir().join(format!("noida-git-{}-{n}", std::process::id()));
                let _ = std::fs::remove_dir_all(&p);
                std::fs::create_dir_all(&p).unwrap();
                Dir(p.canonicalize().unwrap())
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn status_diff_stage_revert() {
        let (_dir, repo) = repo();
        std::fs::write(repo.root.join("a.txt"), "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nTEN\n").unwrap();
        std::fs::write(repo.root.join("new.txt"), "hello\n").unwrap();

        let st = repo.status().unwrap();
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.files[&repo.root.join("a.txt")], FileState::Modified);
        assert_eq!(st.files[&repo.root.join("new.txt")], FileState::Untracked);

        let marks = repo.line_marks(&repo.root.join("a.txt"));
        assert_eq!(marks.get(&0), Some(&LineMark::Modified));
        assert_eq!(marks.get(&9), Some(&LineMark::Modified));

        let diff = repo.diff().unwrap();
        assert_eq!(diff.len(), 2);
        let a = &diff[0];
        assert_eq!((a.path.as_str(), a.hunks.len()), ("a.txt", 2));

        // Accept first hunk, reject second.
        repo.apply_hunk(a, 0, HunkOp::Stage).unwrap();
        repo.apply_hunk(a, 1, HunkOp::Revert).unwrap();
        let text = std::fs::read_to_string(repo.root.join("a.txt")).unwrap();
        assert!(text.starts_with("ONE\n") && text.ends_with("ten\n"));
        assert_eq!(repo.status().unwrap().files[&repo.root.join("a.txt")], FileState::Staged);

        repo.revert_file(&diff[1]).unwrap();
        assert!(!repo.root.join("new.txt").exists());
    }

    fn git(dir: &Path, args: &[&str]) {
        assert!(Command::new("git").arg("-C").arg(dir).args(args).output().unwrap().status.success(), "{args:?}");
    }

    #[test]
    fn staged_diff_unstage_roundtrip() {
        let (_dir, repo) = repo();
        std::fs::write(repo.root.join("a.txt"), "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nTEN\n").unwrap();
        std::fs::write(repo.root.join("new.txt"), "hello\n").unwrap();
        repo.stage_all().unwrap();
        assert!(repo.diff().unwrap().is_empty());

        let staged = repo.diff_staged().unwrap();
        assert_eq!(staged.iter().map(|f| (f.path.as_str(), f.hunks.len())).collect::<Vec<_>>(), [("a.txt", 2), ("new.txt", 1)]);

        // Unstage the second hunk of a.txt: it moves back to the unstaged diff.
        repo.apply_hunk(&staged[0], 1, HunkOp::Unstage).unwrap();
        let staged_now = repo.diff_staged().unwrap();
        assert_eq!(staged_now[0].hunks.len(), 1);
        assert!(staged_now[0].hunks[0].lines.contains(&"+ONE".to_string()));
        let unstaged = repo.diff().unwrap();
        assert_eq!((unstaged[0].path.as_str(), unstaged[0].hunks.len()), ("a.txt", 1));
        assert!(unstaged[0].hunks[0].lines.contains(&"+TEN".to_string()));
        // The working tree is untouched.
        assert!(std::fs::read_to_string(repo.root.join("a.txt")).unwrap().ends_with("TEN\n"));

        // Unstaging the new file's only hunk makes it untracked again; whole-file unstage clears the rest.
        repo.apply_hunk(&staged_now[1], 0, HunkOp::Unstage).unwrap();
        assert_eq!(repo.status().unwrap().files[&repo.root.join("new.txt")], FileState::Untracked);
        repo.unstage_file("a.txt").unwrap();
        assert!(repo.diff_staged().unwrap().is_empty());
        assert_eq!(repo.diff().unwrap()[0].hunks.len(), 2);
    }

    #[test]
    fn parses_log_records() {
        let raw = "\x1eaaaa\x1faa\x1fFix: a|b\x1fAnn\x1f2 days ago\n\x1ebbbb\x1fbb\x1fInit\x1fBob\x1f3 weeks ago\n\nsrc/old.rs\n";
        let log = parse_log(raw);
        assert_eq!(log.len(), 2);
        assert_eq!((log[0].short.as_str(), log[0].subject.as_str(), log[0].author.as_str(), log[0].date.as_str()), ("aa", "Fix: a|b", "Ann", "2 days ago"));
        assert_eq!(log[0].path, None);
        assert_eq!((log[1].hash.as_str(), log[1].path.as_deref()), ("bbbb", Some("src/old.rs")));
        assert!(parse_log("").is_empty());
    }

    #[test]
    fn log_show_and_file_history() {
        let (dir, repo) = repo();
        std::fs::write(repo.root.join("a.txt"), "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n").unwrap();
        std::fs::write(repo.root.join("b.txt"), "b\n").unwrap();
        git(&dir.0, &["add", "."]);
        git(&dir.0, &["commit", "-q", "-m", "shout\n\nlonger body"]);
        git(&dir.0, &["mv", "a.txt", "renamed.txt"]);
        git(&dir.0, &["commit", "-q", "-m", "rename"]);

        let log = repo.log(LOG_LIMIT).unwrap();
        assert_eq!(log.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["rename", "shout", "init"]);
        assert_eq!(log[0].author, "t");

        let show = repo.show(&log[1].hash, &[]).unwrap();
        assert!(show.meta[0].starts_with("commit ") && show.meta.iter().any(|l| l.starts_with("Author: t <t@t>")));
        assert!(show.meta.iter().any(|l| l.trim() == "longer body"));
        assert_eq!(show.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["a.txt", "b.txt"]);
        assert!(show.files[0].hunks[0].lines.contains(&"+ONE".to_string()));

        let hist = repo.file_history("renamed.txt", LOG_LIMIT).unwrap();
        assert_eq!(hist.iter().map(|c| (c.subject.as_str(), c.path.as_deref())).collect::<Vec<_>>(), [
            ("rename", Some("renamed.txt")),
            ("shout", Some("a.txt")),
            ("init", Some("a.txt")),
        ]);
        let only = repo.show(&hist[1].hash, &["a.txt".into()]).unwrap();
        assert_eq!(only.files.len(), 1);
        assert_eq!(only.files[0].path, "a.txt");
        let renamed = repo.show(&hist[0].hash, &["renamed.txt".into(), "a.txt".into()]).unwrap();
        assert_eq!(renamed.files.len(), 1);
        assert_eq!((renamed.files[0].path.as_str(), renamed.files[0].renamed_from()), ("renamed.txt", Some("a.txt")));
    }

    #[test]
    fn worktree_roundtrip() {
        let (_dir, repo) = repo();
        let (path, branch) = repo.add_worktree("Fix Auth!").unwrap();
        assert_eq!(branch, "noida/fix-auth");
        std::fs::write(path.join("a.txt"), "changed\n").unwrap();
        std::fs::write(path.join("b.txt"), "new\n").unwrap();
        let wt = Repo::discover(&path).unwrap();
        let base = wt.merge_base("HEAD", &repo.head().unwrap()).unwrap();
        let stats = wt.numstat(&base, None).unwrap();
        assert_eq!(stats, vec![("a.txt".to_string(), Some((1, 10))), ("b.txt".to_string(), Some((1, 0)))]);
        assert_eq!(wt.numstat(&base, Some(&["b.txt".to_string()])).unwrap().len(), 1);
        assert!(wt.numstat(&base, Some(&[])).unwrap().is_empty());
        assert_eq!(repo.apply_worktree(&wt).unwrap(), 2);
        assert_eq!(std::fs::read_to_string(repo.root.join("b.txt")).unwrap(), "new\n");
        repo.remove_worktree(&path, &branch).unwrap();
        assert!(!path.exists());
    }
}
