//! Per-project workspace state restored on the next launch: open files,
//! cursors, layout and agent tabs (with their conversation ids).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct DocState {
    pub path: String,
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AgentState {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub worktree_branch: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub renamed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Workspace {
    pub docs: Vec<DocState>,
    pub active_doc: usize,
    /// Doc shown by each editor group (indices into `docs`); empty means one group.
    #[serde(default)]
    pub groups: Vec<usize>,
    #[serde(default)]
    pub active_group: usize,
    pub show_tree: bool,
    pub tree_width: u16,
    pub agent_pct: u16,
    pub split: bool,
    #[serde(default)]
    pub split_pct: Option<u16>,
    pub agents: Vec<AgentState>,
    pub slots: [usize; 2],
    #[serde(default)]
    pub expanded: Vec<String>,
}

pub fn path_for(root: &Path) -> Option<PathBuf> {
    let home = crate::settings::home()?;
    let escaped: String = root.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    Some(home.join(".local/share/noida/workspaces").join(format!("{escaped}.json")))
}

pub fn load(root: &Path) -> Option<Workspace> {
    let text = std::fs::read_to_string(path_for(root)?).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(root: &Path, ws: &Workspace) {
    let Some(path) = path_for(root) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(ws) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

/// Random RFC 4122 v4 UUID for new Claude sessions.
pub fn uuid_v4() -> String {
    let mut b = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut b);
    } else {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        b = (t ^ ((std::process::id() as u128) << 64)).to_le_bytes();
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_shape() {
        let u = uuid_v4();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert_ne!(u, uuid_v4());
    }

    #[test]
    fn roundtrip() {
        let ws = Workspace {
            docs: vec![DocState { path: "src/a.rs".into(), line: 10, col: 2 }],
            active_doc: 0,
            groups: vec![0, 0],
            active_group: 1,
            show_tree: true,
            tree_width: 30,
            agent_pct: 45,
            split: false,
            split_pct: None,
            agents: vec![AgentState { name: "claude".into(), command: "claude".into(), cwd: None, session_id: Some(uuid_v4()), worktree_branch: None, title: None, renamed: false }],
            slots: [0, 0],
            expanded: vec![],
        };
        let text = serde_json::to_string(&ws).unwrap();
        assert_eq!(serde_json::from_str::<Workspace>(&text).unwrap(), ws);
    }

    #[test]
    fn loads_without_groups() {
        let text = r#"{"docs":[],"active_doc":0,"show_tree":true,"tree_width":30,"agent_pct":45,"split":false,"agents":[],"slots":[0,0]}"#;
        let ws: Workspace = serde_json::from_str(text).unwrap();
        assert!(ws.groups.is_empty());
        assert_eq!(ws.active_group, 0);
    }
}
