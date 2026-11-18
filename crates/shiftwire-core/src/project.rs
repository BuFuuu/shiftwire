//! Projects, one folder each (like ShiftGrid): `project.json` marks a folder as
//! a project and names it, `flows.db` holds everything captured in it. Folders
//! can live anywhere; the projects dir is only where new ones go by default,
//! and a recent list remembers the ones opened from elsewhere.

use std::path::{Path, PathBuf};
use std::{fs, io};

use serde::{Deserialize, Serialize};
use serde_json::json;

/// The file that makes a folder a project.
pub const MARKER: &str = "project.json";
/// The project's database inside its folder.
pub const DB: &str = "flows.db";
const RECENT_LIMIT: usize = 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    /// Unix milliseconds.
    pub created_at: u64,
    /// The project folder (not stored in `project.json`: folders can be moved).
    #[serde(default)]
    pub path: String,
}

/// A project as listed on the Projects page.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectInfo {
    #[serde(flatten)]
    pub project: Project,
    pub loaded: bool,
    pub flows: Option<u64>,
    /// Size of the folder's files in bytes.
    pub size: u64,
}

/// Creates `base/<slug>-<6 chars>/project.json`.
pub fn create(base: &Path, name: &str) -> io::Result<Project> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(crate::invalid(
            "a project name needs 1-80 characters".into(),
        ));
    }
    let suffix = crate::new_id()[20..].to_ascii_lowercase();
    let id = format!("{}-{suffix}", slug(name));
    let folder = base.join(&id);
    fs::create_dir_all(&folder)?;
    let meta = json!({ "id": id, "name": name, "created_at": crate::now_ms() });
    let tmp = folder.join(format!("{MARKER}.tmp"));
    fs::write(&tmp, serde_json::to_vec_pretty(&meta)?)?;
    fs::rename(tmp, folder.join(MARKER))?;
    load(&folder)
}

/// Reads the project in `folder`, which must contain `project.json`.
pub fn load(folder: &Path) -> io::Result<Project> {
    let folder = folder
        .canonicalize()
        .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", folder.display())))?;
    let text = fs::read(folder.join(MARKER)).map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "{} is not a shiftWire project (no {MARKER})",
                folder.display()
            ),
        )
    })?;
    let mut project: Project = serde_json::from_slice(&text).map_err(io::Error::other)?;
    project.path = folder.display().to_string();
    Ok(project)
}

/// Every project in `base` plus the recently opened ones elsewhere, newest first.
pub fn discover(base: &Path, recent: &[String]) -> Vec<Project> {
    let mut folders: Vec<PathBuf> = recent.iter().map(PathBuf::from).collect();
    if let Ok(entries) = fs::read_dir(base) {
        folders.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    let mut projects: Vec<Project> = Vec::new();
    for folder in folders {
        if let Ok(p) = load(&folder)
            && !projects.iter().any(|q| q.path == p.path)
        {
            projects.push(p);
        }
    }
    projects.sort_by_key(|p| std::cmp::Reverse(p.created_at));
    projects
}

/// Deletes a project folder. Refuses folders that are not projects.
pub fn delete(folder: &Path) -> io::Result<()> {
    let project = load(folder)?;
    fs::remove_dir_all(project.path)
}

/// Total size of the files directly in `folder`.
pub fn size(folder: &Path) -> u64 {
    fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// Recently opened project folders, newest first (`recent.json` in the data dir).
pub struct Recent(pub PathBuf);

impl Recent {
    pub fn read(&self) -> Vec<String> {
        fs::read(&self.0)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn record(&self, path: &str) {
        let mut list = self.read();
        list.retain(|p| p != path);
        list.insert(0, path.to_string());
        list.truncate(RECENT_LIMIT);
        self.write(&list);
    }

    pub fn forget(&self, path: &str) {
        let mut list = self.read();
        list.retain(|p| p != path);
        self.write(&list);
    }

    fn write(&self, list: &[String]) {
        if let Ok(bytes) = serde_json::to_vec_pretty(list) {
            let _ = fs::write(&self.0, bytes);
        }
    }
}

/// `"ACME Web App"` -> `"acme-web-app"`.
fn slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let s = s
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if s.is_empty() { "project".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("shiftwire-project-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn slugs_are_folder_safe() {
        assert_eq!(slug("ACME Web App"), "acme-web-app");
        assert_eq!(slug("../../etc"), "etc");
        assert_eq!(slug("äöü"), "project");
    }

    #[test]
    fn create_load_discover_delete() {
        let base = temp("crud");
        let p = create(&base, "Shop Test").unwrap();
        assert!(p.id.starts_with("shop-test-"));
        assert_eq!(load(Path::new(&p.path)).unwrap(), p);
        assert!(create(&base, "  ").is_err());

        // A project outside the base dir is found through the recent list.
        let elsewhere = temp("crud-elsewhere");
        let q = create(&elsewhere, "Other").unwrap();
        let found = discover(&base, std::slice::from_ref(&q.path));
        assert_eq!(found.len(), 2);

        let plain = base.join("not-a-project");
        fs::create_dir_all(&plain).unwrap();
        assert!(delete(&plain).is_err(), "only project folders are deleted");
        delete(Path::new(&p.path)).unwrap();
        assert_eq!(discover(&base, &[]).len(), 0);
    }

    #[test]
    fn recent_keeps_newest_first_without_duplicates() {
        let recent = Recent(temp("recent").join("recent.json"));
        recent.record("/a");
        recent.record("/b");
        recent.record("/a");
        assert_eq!(recent.read(), ["/a", "/b"]);
        recent.forget("/a");
        assert_eq!(recent.read(), ["/b"]);
    }
}
