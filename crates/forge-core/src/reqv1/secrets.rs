//! Shared loading for the project-local, gitignored `.env.local` provider.

use std::collections::BTreeMap;
use std::path::Path;

pub fn load_file_secrets(root: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(root.join(".env.local")) else {
        return BTreeMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            (!key.is_empty()).then(|| {
                let value = value.trim();
                let value = serde_json::from_str::<String>(value).unwrap_or_else(|_| {
                    value
                        .strip_prefix('\'')
                        .and_then(|value| value.strip_suffix('\''))
                        .unwrap_or(value)
                        .to_string()
                });
                (key.to_string(), value)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_names_and_values_without_comments() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".env.local"),
            "# local\nTOKEN=\"secret\"\n EMPTY = ''\nBROKEN\n",
        )
        .unwrap();

        let secrets = load_file_secrets(dir.path());

        assert_eq!(secrets.get("TOKEN").map(String::as_str), Some("secret"));
        assert_eq!(secrets.get("EMPTY").map(String::as_str), Some(""));
        assert!(!secrets.contains_key("BROKEN"));
    }

    #[test]
    fn loads_json_quoted_secret_values_losslessly() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".env.local"),
            "TOKEN=\" leading \\\"quoted\\\" value \\\\ path \"\n",
        )
        .unwrap();

        let secrets = load_file_secrets(dir.path());

        assert_eq!(
            secrets.get("TOKEN").map(String::as_str),
            Some(" leading \"quoted\" value \\ path ")
        );
    }
}
