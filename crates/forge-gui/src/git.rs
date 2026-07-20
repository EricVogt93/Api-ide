//! Minimal git integration for the collections tree: per-file status (via
//! `git status --porcelain`), the current branch, and the add/restore/commit
//! actions surfaced in the tree's context menu.
//!
//! Uses the `git` CLI rather than libgit2 — no build dependency, and the
//! workspace-sized repos this runs on answer `git status` in milliseconds.
// ponytail: synchronous Command calls, refreshed at most every few seconds —
// move onto the bridge thread if huge repos ever make `git status` slow.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Working-tree state of one file, as shown in the collections tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    /// Tracked, modified (staged or not).
    Modified,
    /// Not tracked by git yet.
    Untracked,
    /// Newly added to the index.
    Added,
    /// Merge conflict.
    Conflicted,
}

/// Snapshot of the workspace's git state.
#[derive(Debug, Clone, Default)]
pub struct GitStatus {
    pub branch: Option<String>,
    /// Absolute path → status, for every changed file under the repo.
    pub files: HashMap<PathBuf, FileStatus>,
}

impl GitStatus {
    /// Status of `path`, or of any file under it when `path` is a directory
    /// (a folder shows its "most interesting" child status).
    pub fn of(&self, path: &Path) -> Option<FileStatus> {
        if let Some(s) = self.files.get(path) {
            return Some(*s);
        }
        let mut dir_status: Option<FileStatus> = None;
        for (p, s) in &self.files {
            if p.starts_with(path) {
                dir_status = Some(match (dir_status, *s) {
                    (_, FileStatus::Conflicted) | (Some(FileStatus::Conflicted), _) => {
                        FileStatus::Conflicted
                    }
                    (Some(FileStatus::Modified), _) | (_, FileStatus::Modified) => {
                        FileStatus::Modified
                    }
                    (_, other) => other,
                });
            }
        }
        dir_status
    }
}

/// Cached git snapshot with periodic refresh, owned by [`crate::state::AppState`].
#[derive(Default)]
pub struct GitState {
    pub status: Option<GitStatus>,
    last_refresh: Option<Instant>,
}

impl GitState {
    const REFRESH_EVERY: Duration = Duration::from_secs(4);

    /// Refresh the snapshot if it is stale (or `force`).
    pub fn refresh(&mut self, root: &Path, force: bool) {
        let stale = self
            .last_refresh
            .is_none_or(|t| t.elapsed() > Self::REFRESH_EVERY);
        if force || stale {
            self.status = read_status(root);
            self.last_refresh = Some(Instant::now());
        }
    }
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Read branch + per-file status. `None` when `root` is not in a git repo
/// (or git is not installed).
pub fn read_status(root: &Path) -> Option<GitStatus> {
    let top = git(root, &["rev-parse", "--show-toplevel"])?;
    let top = PathBuf::from(top.trim());
    // `rev-parse --abbrev-ref` fails on an unborn branch (fresh repo, no
    // commits yet); `symbolic-ref` still names it.
    let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .or_else(|| git(root, &["symbolic-ref", "--short", "HEAD"]))
        .map(|s| s.trim().to_string());

    let mut files = HashMap::new();
    let porcelain = git(root, &["status", "--porcelain"])?;
    for line in porcelain.lines() {
        if line.len() < 4 {
            continue;
        }
        let (code, rest) = line.split_at(2);
        // Renames: "R  old -> new" — track the new name.
        let path = rest
            .trim_start()
            .rsplit(" -> ")
            .next()
            .unwrap_or(rest)
            .trim()
            .trim_matches('"');
        let status = match code {
            "??" => FileStatus::Untracked,
            c if c.contains('U') => FileStatus::Conflicted,
            "A " | "AM" => FileStatus::Added,
            _ => FileStatus::Modified,
        };
        files.insert(top.join(path), status);
    }
    Some(GitStatus { branch, files })
}

/// `git add <path>`. Returns an error message on failure.
pub fn add(root: &Path, path: &Path) -> Result<(), String> {
    run_ok(root, &["add", "--"], path)
}

/// Discard working-tree changes to a tracked `path` (`git restore`).
pub fn restore(root: &Path, path: &Path) -> Result<(), String> {
    run_ok(root, &["restore", "--staged", "--worktree", "--"], path)
        .or_else(|_| run_ok(root, &["restore", "--"], path))
}

/// Stage and commit `path` with `message`.
pub fn commit(root: &Path, path: &Path, message: &str) -> Result<(), String> {
    add(root, path)?;
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["commit", "-m", message, "--"])
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

fn run_ok(root: &Path, args: &[&str], path: &Path) -> Result<(), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            assert!(Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "T"]);
        dir
    }

    #[test]
    fn status_reports_untracked_added_modified() {
        let dir = init_repo();
        let root = dir.path();
        std::fs::write(root.join("a.txt"), "one").unwrap();
        let st = read_status(root).expect("repo");
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(
            st.of(&root
                .join("a.txt")
                .canonicalize()
                .unwrap_or(root.join("a.txt"))),
            Some(FileStatus::Untracked)
        );

        add(root, &root.join("a.txt")).unwrap();
        let st = read_status(root).unwrap();
        assert!(matches!(
            st.files.values().next(),
            Some(FileStatus::Added | FileStatus::Modified)
        ));

        commit(root, &root.join("a.txt"), "add a").unwrap();
        let st = read_status(root).unwrap();
        assert!(st.files.is_empty(), "clean after commit: {:?}", st.files);

        std::fs::write(root.join("a.txt"), "two").unwrap();
        let st = read_status(root).unwrap();
        assert!(matches!(
            st.files.values().next(),
            Some(FileStatus::Modified)
        ));

        restore(root, &root.join("a.txt")).unwrap();
        let st = read_status(root).unwrap();
        assert!(st.files.is_empty(), "clean after restore: {:?}", st.files);
    }

    #[test]
    fn non_repo_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_status(dir.path()).is_none());
    }

    #[test]
    fn dir_status_aggregates_children() {
        let dir = init_repo();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/x.txt"), "x").unwrap();
        let st = read_status(root).unwrap();
        let canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        assert_eq!(st.of(&canon.join("sub")), Some(FileStatus::Untracked));
    }
}
