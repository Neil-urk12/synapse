//! Persistent registry of indexed repos at `~/.synapse/repos.json`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("HOME is unset; cannot locate ~/.synapse/repos.json")]
    NoHome,
    #[error("registry I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("registry JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("repo path '{path}' is already registered as '{existing_name}'; pass --force-register to overwrite")]
    DuplicatePath { path: String, existing_name: String },
    #[error("repo name '{name}' is already used by '{existing_path}'")]
    DuplicateName { name: String, existing_path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoEntry {
    pub name: String,
    pub path: PathBuf,
    pub db_path: PathBuf,
    pub indexed_at: String,
    pub indexed_commit: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RepoRegistry {
    pub entries: Vec<RepoEntry>,
}

const REGISTRY_DIR: &str = ".synapse";
const REGISTRY_FILE: &str = "repos.json";

pub fn registry_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(REGISTRY_DIR).join(REGISTRY_FILE))
}

impl RepoRegistry {
    pub fn load() -> Result<Self, RegistryError> {
        let path = registry_path().ok_or(RegistryError::NoHome)?;
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> Result<Self, RegistryError> {
        match fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(RegistryError::Io(e)),
        }
    }

    pub fn save(&self) -> Result<(), RegistryError> {
        let path = registry_path().ok_or(RegistryError::NoHome)?;
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), RegistryError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let tmp_path = path.with_extension("json.tmp");
        let write_result = (|| -> Result<(), RegistryError> {
            let mut file = fs::File::create(&tmp_path)?;
            let bytes = serde_json::to_vec_pretty(self)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            Ok(())
        })();
        if let Err(e) = write_result {
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }
        fs::rename(&tmp_path, path)?;
        Ok(())
    }

    pub fn add_or_update(&mut self, entry: RepoEntry, force: bool) -> Result<(), RegistryError> {
        if let Some(position) = self.entries.iter().position(|e| e.path == entry.path) {
            if !force {
                let existing = &self.entries[position];
                return Err(RegistryError::DuplicatePath {
                    path: entry.path.to_string_lossy().into_owned(),
                    existing_name: existing.name.clone(),
                });
            }
            let mut updated = entry;
            if !self.entries[position].name.is_empty() {
                updated.name = self.entries[position].name.clone();
            }
            self.entries[position] = updated;
            return Ok(());
        }

        if let Some(existing) = self.entries.iter().find(|e| e.name == entry.name) {
            return Err(RegistryError::DuplicateName {
                name: entry.name,
                existing_path: existing.path.to_string_lossy().into_owned(),
            });
        }

        self.entries.push(entry);
        Ok(())
    }

    pub fn find_by_name(&self, name: &str) -> Option<&RepoEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    pub fn find_by_path(&self, path: &Path) -> Option<&RepoEntry> {
        self.entries.iter().find(|e| e.path == path)
    }
}

pub fn is_stale(entry: &RepoEntry) -> bool {
    if entry.indexed_commit.is_empty() {
        return false;
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(&entry.path)
        .arg("rev-parse")
        .arg("HEAD")
        .output();
    let Ok(output) = output else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    head != entry.indexed_commit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_empty_registry_returns_empty_when_file_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repos.json");
        let registry = RepoRegistry::load_from(&path).unwrap();
        assert!(registry.entries.is_empty());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repos.json");
        let entry = RepoEntry {
            name: "synapse".to_string(),
            path: PathBuf::from("/home/user/synapse"),
            db_path: PathBuf::from("/home/user/synapse/synapse.lbug"),
            indexed_at: "2026-06-17T08:30:00Z".to_string(),
            indexed_commit: "abc123".to_string(),
        };
        let mut registry = RepoRegistry::default();
        registry.add_or_update(entry.clone(), false).unwrap();
        registry.save_to(&path).unwrap();

        let loaded = RepoRegistry::load_from(&path).unwrap();
        assert_eq!(loaded.entries, vec![entry]);
    }

    #[test]
    fn add_or_update_preserves_name_on_re_add_with_force() {
        let mut registry = RepoRegistry::default();
        let first = RepoEntry {
            name: "foo".to_string(),
            path: PathBuf::from("/repo"),
            db_path: PathBuf::from("/repo/synapse.lbug"),
            indexed_at: "2026-06-17T08:30:00Z".to_string(),
            indexed_commit: "abc".to_string(),
        };
        registry.add_or_update(first, false).unwrap();

        let second = RepoEntry {
            name: "bar".to_string(),
            path: PathBuf::from("/repo"),
            db_path: PathBuf::from("/repo/synapse.lbug"),
            indexed_at: "2026-06-17T09:00:00Z".to_string(),
            indexed_commit: "def".to_string(),
        };
        registry.add_or_update(second, true).unwrap();

        assert_eq!(registry.entries.len(), 1);
        let entry = &registry.entries[0];
        assert_eq!(entry.name, "foo");
        assert_eq!(entry.indexed_at, "2026-06-17T09:00:00Z");
        assert_eq!(entry.indexed_commit, "def");
    }

    #[test]
    fn add_or_update_rejects_duplicate_path_without_force() {
        let mut registry = RepoRegistry::default();
        let first = RepoEntry {
            name: "foo".to_string(),
            path: PathBuf::from("/repo"),
            db_path: PathBuf::from("/repo/synapse.lbug"),
            indexed_at: "2026-06-17T08:30:00Z".to_string(),
            indexed_commit: "abc".to_string(),
        };
        registry.add_or_update(first, false).unwrap();

        let second = RepoEntry {
            name: "bar".to_string(),
            path: PathBuf::from("/repo"),
            db_path: PathBuf::from("/repo/synapse.lbug"),
            indexed_at: "2026-06-17T09:00:00Z".to_string(),
            indexed_commit: "def".to_string(),
        };
        let err = registry.add_or_update(second, false).unwrap_err();
        assert!(
            matches!(err, RegistryError::DuplicatePath { ref existing_name, .. } if existing_name == "foo"),
            "expected DuplicatePath with existing_name=foo, got {err:?}"
        );
    }

    #[test]
    fn is_stale_returns_false_for_non_git_path() {
        let entry_no_commit = RepoEntry {
            name: "no-commit".to_string(),
            path: PathBuf::from("/tmp"),
            db_path: PathBuf::from("/tmp/synapse.lbug"),
            indexed_at: "2026-06-17T08:30:00Z".to_string(),
            indexed_commit: String::new(),
        };
        assert!(!is_stale(&entry_no_commit));

        let entry_non_git = RepoEntry {
            name: "non-git".to_string(),
            path: PathBuf::from("/tmp"),
            db_path: PathBuf::from("/tmp/synapse.lbug"),
            indexed_at: "2026-06-17T08:30:00Z".to_string(),
            indexed_commit: "abc123".to_string(),
        };
        assert!(!is_stale(&entry_non_git));
    }
}
