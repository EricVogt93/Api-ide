//! Git-friendly Jira links for project folders and individual test files.

use std::path::{Path, PathBuf};

pub const FOLDER_TICKET_FILE: &str = ".forge-jira";
pub const FILE_TICKET_SUFFIX: &str = ".forge-jira";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketLink {
    pub value: String,
    pub source: PathBuf,
}

fn ticket_file(node: &Path) -> PathBuf {
    if node.is_dir() {
        node.join(FOLDER_TICKET_FILE)
    } else {
        let name = node
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();
        node.with_file_name(format!(".{name}{FILE_TICKET_SUFFIX}"))
    }
}

pub fn own_ticket(node: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(ticket_file(node)) {
        Ok(value) => Ok((!value.trim().is_empty()).then(|| value.trim().to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "cannot read Jira link for {}: {error}",
            node.display()
        )),
    }
}

pub fn effective_ticket(root: &Path, node: &Path) -> Result<Option<TicketLink>, String> {
    if !node.starts_with(root) {
        return Err(format!("{} is outside the project", node.display()));
    }
    let mut current = Some(node);
    while let Some(candidate) = current {
        if let Some(value) = own_ticket(candidate)? {
            return Ok(Some(TicketLink {
                value,
                source: candidate.to_path_buf(),
            }));
        }
        if candidate == root {
            break;
        }
        current = candidate.parent();
    }
    Ok(None)
}

pub fn set_ticket(node: &Path, value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Jira ticket must not be empty".to_string());
    }
    let path = ticket_file(node);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    std::fs::write(&path, format!("{value}\n"))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

pub fn remove_ticket(node: &Path) -> Result<(), String> {
    let path = ticket_file(node);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot remove {}: {error}", path.display())),
    }
}

pub fn ticket_label(value: &str) -> &str {
    value
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_ticket_overrides_and_then_falls_back_to_parent() {
        let root = tempfile::tempdir().unwrap();
        let story = root.path().join("requests/checkout");
        std::fs::create_dir_all(&story).unwrap();
        let test = story.join("submit.request.json");
        std::fs::write(&test, "{}").unwrap();

        set_ticket(&story, "SHOP-100").unwrap();
        assert_eq!(
            effective_ticket(root.path(), &test).unwrap().unwrap(),
            TicketLink {
                value: "SHOP-100".to_string(),
                source: story.clone(),
            }
        );

        set_ticket(&test, "https://jira.example/browse/SHOP-101").unwrap();
        assert_eq!(
            effective_ticket(root.path(), &test).unwrap().unwrap().value,
            "https://jira.example/browse/SHOP-101"
        );

        remove_ticket(&test).unwrap();
        assert_eq!(
            effective_ticket(root.path(), &test).unwrap().unwrap().value,
            "SHOP-100"
        );
    }
}
