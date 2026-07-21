//! Local configuration and OpenAI-compatible transport for the API advisor.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const CONFIG_FILE: &str = "advisor.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdvisorConfig {
    pub endpoint: String,
    pub model: String,
    /// Optional variable name resolved from `.env.local` or the process.
    pub api_key_env: String,
}

fn path_for(root: &Path) -> PathBuf {
    root.join(forge_core::store::LOCAL_DIR).join(CONFIG_FILE)
}

pub fn load(root: &Path) -> Result<AdvisorConfig, String> {
    let path = path_for(root);
    if !path.exists() {
        return Ok(AdvisorConfig::default());
    }
    forge_core::store::load_json(&path).map_err(|error| error.to_string())
}

pub fn save(root: &Path, config: &AdvisorConfig) -> Result<(), String> {
    forge_core::store::ensure_gitignore(root).map_err(|error| error.to_string())?;
    forge_core::store::save_json(&path_for(root), config).map_err(|error| error.to_string())
}

pub fn resolve_api_key(root: &Path, variable: &str) -> Result<Option<String>, String> {
    let variable = variable.trim();
    if variable.is_empty() {
        return Ok(None);
    }
    if variable.contains(['=', '\0']) {
        return Err("API key variable name is invalid".to_string());
    }
    forge_core::reqv1::load_file_secrets(root)
        .get(variable)
        .cloned()
        .or_else(|| std::env::var(variable).ok())
        .map(Some)
        .ok_or_else(|| {
            format!("secret {variable} was not found in .env.local or the process environment")
        })
}

pub async fn ask(
    config: &AdvisorConfig,
    api_key: Option<&str>,
    question: &str,
    context: &str,
) -> Result<String, String> {
    let endpoint = chat_completions_url(&config.endpoint)?;
    let model = config.model.trim();
    if model.is_empty() {
        return Err("configure an advisor model first".to_string());
    }
    if question.trim().is_empty() {
        return Err("enter a question for the advisor".to_string());
    }
    let body = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": "You are a concise API testing advisor. Review requests against the supplied OpenAPI context, identify concrete defects, and suggest maintainable assertions or fixes. Never invent response data."
            },
            {
                "role": "user",
                "content": format!("{}\n\nAPI context:\n{}", question.trim(), context)
            }
        ]
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|error| error.to_string())?;
    let mut request = client.post(endpoint).json(&body);
    if let Some(api_key) = api_key {
        request = request.bearer_auth(api_key);
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    let status = response.status();
    let value: serde_json::Value = response.json().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        let message = value
            .pointer("/error/message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("provider returned an error");
        return Err(format!("advisor HTTP {status}: {message}"));
    }
    response_text(&value).ok_or_else(|| "advisor response contained no text".to_string())
}

fn chat_completions_url(endpoint: &str) -> Result<url::Url, String> {
    let mut url = url::Url::parse(endpoint.trim()).map_err(|error| error.to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("advisor endpoint must use http or https".to_string());
    }
    let path = url.path().trim_end_matches('/');
    if !path.ends_with("/chat/completions") {
        let base = if path.is_empty() { "/v1" } else { path };
        url.set_path(&format!("{base}/chat/completions"));
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn response_text(value: &serde_json::Value) -> Option<String> {
    let content = value.pointer("/choices/0/message/content")?;
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    let blocks = content.as_array()?;
    let text = blocks
        .iter()
        .filter_map(|block| block.get("text").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_accepts_base_or_full_chat_url() {
        assert_eq!(
            chat_completions_url("http://localhost:11434/v1")
                .unwrap()
                .as_str(),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://ai.example/chat/completions")
                .unwrap()
                .as_str(),
            "https://ai.example/chat/completions"
        );
        assert!(chat_completions_url("file:///tmp/model").is_err());
    }

    #[test]
    fn reads_string_and_block_response_content() {
        assert_eq!(
            response_text(&serde_json::json!({
                "choices": [{"message": {"content": "Use a status assertion."}}]
            }))
            .as_deref(),
            Some("Use a status assertion.")
        );
        assert_eq!(
            response_text(&serde_json::json!({
                "choices": [{"message": {"content": [
                    {"type": "text", "text": "First"},
                    {"type": "text", "text": "Second"}
                ]}}]
            }))
            .as_deref(),
            Some("First\nSecond")
        );
    }

    #[test]
    fn config_is_local_and_round_trips_without_a_secret_value() {
        let root = tempfile::tempdir().unwrap();
        let config = AdvisorConfig {
            endpoint: "https://ai.example/v1".to_string(),
            model: "api-model".to_string(),
            api_key_env: "AI_KEY".to_string(),
        };
        save(root.path(), &config).unwrap();
        assert_eq!(load(root.path()).unwrap(), config);
        assert!(path_for(root.path()).ends_with(".forge-local/advisor.json"));
        assert!(std::fs::read_to_string(root.path().join(".gitignore"))
            .unwrap()
            .lines()
            .any(|line| line == ".forge-local/"));
    }
}
