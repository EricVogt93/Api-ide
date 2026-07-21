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
    /// Tracked, modified in the working tree but not staged.
    Modified,
    /// Not tracked by git yet.
    Untracked,
    /// Newly added to the index.
    Added,
    /// Tracked change staged in the index.
    Staged,
    /// Change staged in the index and modified again in the working tree.
    StagedModified,
    /// Merge conflict.
    Conflicted,
}

impl FileStatus {
    fn priority(self) -> u8 {
        match self {
            Self::Untracked => 1,
            Self::Added | Self::Staged => 2,
            Self::Modified => 3,
            Self::StagedModified => 4,
            Self::Conflicted => 5,
        }
    }
}

/// Snapshot of the workspace's git state.
#[derive(Debug, Clone, Default)]
pub struct GitStatus {
    pub branch: Option<String>,
    repo_root: PathBuf,
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
                dir_status = Some(
                    dir_status
                        .filter(|current| current.priority() >= s.priority())
                        .unwrap_or(*s),
                );
            }
        }
        dir_status
    }
}

/// Cached git snapshot with periodic refresh, owned by [`crate::state::AppState`].
#[derive(Default)]
pub struct GitState {
    pub status: Option<GitStatus>,
    pub worktree_open: bool,
    pub worktree_branch: String,
    pub worktree_path: String,
    root: Option<PathBuf>,
    last_refresh: Option<Instant>,
}

impl GitState {
    const REFRESH_EVERY: Duration = Duration::from_secs(4);

    /// Refresh the snapshot if it is stale (or `force`).
    pub fn refresh(&mut self, root: &Path, force: bool) {
        let requested_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let same_repo = self
            .status
            .as_ref()
            .is_some_and(|status| requested_root.starts_with(&status.repo_root));
        let root_changed = self.root.as_deref() != Some(root) && !same_repo;
        let stale = self
            .last_refresh
            .is_none_or(|t| t.elapsed() > Self::REFRESH_EVERY);
        if force || root_changed || stale {
            self.status = read_status(root);
            self.root = Some(root.to_path_buf());
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
        let status = parse_status(code);
        files.insert(top.join(path), status);
    }
    Some(GitStatus {
        branch,
        repo_root: top,
        files,
    })
}

fn parse_status(code: &str) -> FileStatus {
    let bytes = code.as_bytes();
    let (index, worktree) = (bytes[0], bytes[1]);
    if code == "??" {
        FileStatus::Untracked
    } else if code.contains('U') || matches!(code, "AA" | "DD") {
        FileStatus::Conflicted
    } else if index == b'A' && worktree == b' ' {
        FileStatus::Added
    } else if index != b' ' && worktree != b' ' {
        FileStatus::StagedModified
    } else if index != b' ' {
        FileStatus::Staged
    } else {
        FileStatus::Modified
    }
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

pub fn branches(root: &Path) -> Result<Vec<String>, String> {
    let output = git_result(
        root,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
    )?;
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .map(str::to_string)
        .collect())
}

pub fn switch_branch(root: &Path, branch: &str) -> Result<(), String> {
    git_result(root, &["switch", branch.trim()]).map(|_| ())
}

pub fn create_worktree(root: &Path, path: &Path, branch: &str) -> Result<(), String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("branch name is required".to_string());
    }
    if path.as_os_str().is_empty() || path.exists() {
        return Err("worktree path must not exist yet".to_string());
    }
    let existing = branches(root)?.iter().any(|candidate| candidate == branch);
    let path = path
        .to_str()
        .ok_or_else(|| "worktree path is not valid UTF-8".to_string())?;
    if existing {
        git_result(root, &["worktree", "add", path, branch])?;
    } else {
        git_result(root, &["worktree", "add", "-b", branch, path])?;
    }
    Ok(())
}

fn git_result(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
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
    fn porcelain_columns_separate_index_and_worktree_changes() {
        assert_eq!(parse_status(" M"), FileStatus::Modified);
        assert_eq!(parse_status("M "), FileStatus::Staged);
        assert_eq!(parse_status("MM"), FileStatus::StagedModified);
        assert_eq!(parse_status("A "), FileStatus::Added);
        assert_eq!(parse_status("??"), FileStatus::Untracked);
        assert_eq!(parse_status("UU"), FileStatus::Conflicted);
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

    #[test]
    fn creates_worktree_for_a_new_branch() {
        let parent = tempfile::tempdir().unwrap();
        let repo = parent.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| {
            assert!(Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "T"]);
        std::fs::write(repo.join("tracked.txt"), "one").unwrap();
        run(&["add", "tracked.txt"]);
        run(&["commit", "-q", "-m", "initial"]);

        let worktree = parent.path().join("feature-worktree");
        create_worktree(&repo, &worktree, "feature").unwrap();

        assert!(worktree.join("tracked.txt").is_file());
        assert!(branches(&repo)
            .unwrap()
            .iter()
            .any(|branch| branch == "feature"));
        assert_eq!(
            read_status(&worktree).unwrap().branch.as_deref(),
            Some("feature")
        );
    }
}
