//! GitHub Release based desktop updater.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bridge::{Bridge, Cmd};

const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/EricVogt93/apiwright/releases/latest";

#[derive(Debug, Clone)]
pub struct UpdateRelease {
    pub tag: String,
    pub version: String,
    pub title: String,
    pub notes: String,
    pub page_url: String,
    asset: Option<UpdateAsset>,
    checksums_url: Option<String>,
}

#[derive(Debug, Clone)]
struct UpdateAsset {
    name: String,
    url: String,
    digest: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DownloadedUpdate {
    release: UpdateRelease,
    path: PathBuf,
}

#[derive(Debug, Default)]
pub struct UpdateState {
    pub checking: bool,
    open: bool,
    downloading: bool,
    release: Option<UpdateRelease>,
    downloaded: Option<DownloadedUpdate>,
    error: Option<String>,
    notice: Option<String>,
}

impl UpdateState {
    pub fn check(&mut self, bridge: &Bridge, manual: bool) {
        if self.checking || self.downloading {
            return;
        }
        self.checking = true;
        self.error = None;
        self.notice = None;
        if manual {
            self.open = true;
        }
        if let Err(error) = bridge.send(Cmd::CheckForUpdates { manual }) {
            self.checking = false;
            self.error = Some(error);
            self.open = manual;
        }
    }

    pub fn handle_check(&mut self, manual: bool, result: Result<Option<UpdateRelease>, String>) {
        self.checking = false;
        match result {
            Ok(Some(release)) if !manual && skipped_release().as_deref() == Some(&release.tag) => {}
            Ok(Some(release)) => {
                self.release = Some(release);
                self.downloaded = None;
                self.open = true;
            }
            Ok(None) if manual => {
                self.notice = Some(format!(
                    "ApiWright v{} is the latest published version.",
                    env!("CARGO_PKG_VERSION")
                ));
                self.open = true;
            }
            Ok(None) => {}
            Err(error) if manual => {
                self.error = Some(error);
                self.open = true;
            }
            Err(_) => {}
        }
    }

    pub fn handle_download(&mut self, result: Result<DownloadedUpdate, String>) {
        self.downloading = false;
        self.open = true;
        match result {
            Ok(downloaded) => {
                self.downloaded = Some(downloaded);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiRelease {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Debug, Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum PackageKind {
    LinuxAppImage,
    WindowsExe,
    MacDmg,
}

impl PackageKind {
    fn current() -> Option<Self> {
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Some(Self::LinuxAppImage)
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            Some(Self::WindowsExe)
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Some(Self::MacDmg)
        } else {
            None
        }
    }

    fn matches(self, name: &str) -> bool {
        let name = name.to_ascii_lowercase();
        match self {
            Self::LinuxAppImage => name.ends_with("-linux-x86_64.appimage"),
            Self::WindowsExe => name.ends_with("-windows-x86_64.exe"),
            Self::MacDmg => name.ends_with("-macos-arm64.dmg"),
        }
    }
}

pub async fn check_for_update() -> Result<Option<UpdateRelease>, String> {
    check_for_update_at(
        LATEST_RELEASE_URL,
        env!("CARGO_PKG_VERSION"),
        PackageKind::current(),
    )
    .await
}

async fn check_for_update_at(
    url: &str,
    current: &str,
    package: Option<PackageKind>,
) -> Result<Option<UpdateRelease>, String> {
    let client = update_client(std::time::Duration::from_secs(15))?;
    let response = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|error| format!("Update check failed: {error}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let response = response
        .error_for_status()
        .map_err(|error| format!("Update check failed: {error}"))?;
    let release = response
        .json::<ApiRelease>()
        .await
        .map_err(|error| format!("Invalid release metadata: {error}"))?;
    release_from_api(release, current, package)
}

fn release_from_api(
    release: ApiRelease,
    current: &str,
    package: Option<PackageKind>,
) -> Result<Option<UpdateRelease>, String> {
    let version_text = release.tag_name.trim_start_matches(['v', 'V']);
    let version = semver::Version::parse(version_text)
        .map_err(|error| format!("Invalid release version {}: {error}", release.tag_name))?;
    let current = semver::Version::parse(current)
        .map_err(|error| format!("Invalid installed version {current}: {error}"))?;
    if version <= current {
        return Ok(None);
    }

    let checksums_url = release
        .assets
        .iter()
        .find(|asset| asset.name == "SHA256SUMS.txt")
        .map(|asset| asset.browser_download_url.clone());
    let asset = package.and_then(|kind| {
        release
            .assets
            .iter()
            .find(|asset| kind.matches(&asset.name))
            .map(|asset| UpdateAsset {
                name: asset.name.clone(),
                url: asset.browser_download_url.clone(),
                digest: asset.digest.clone(),
            })
    });
    Ok(Some(UpdateRelease {
        title: release
            .name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| format!("ApiWright {}", release.tag_name)),
        notes: release
            .body
            .filter(|notes| !notes.trim().is_empty())
            .unwrap_or_else(|| "No changelog was provided for this release.".to_string()),
        page_url: release.html_url,
        tag: release.tag_name,
        version: version.to_string(),
        asset,
        checksums_url,
    }))
}

pub async fn download_update(release: UpdateRelease) -> Result<DownloadedUpdate, String> {
    let cache = update_cache_dir()?;
    download_update_to(release, &cache).await
}

async fn download_update_to(
    release: UpdateRelease,
    cache: &Path,
) -> Result<DownloadedUpdate, String> {
    let asset = release.asset.as_ref().ok_or_else(|| {
        "No package is available for this operating system and architecture.".to_string()
    })?;
    let client = update_client(std::time::Duration::from_secs(5 * 60))?;
    let expected = expected_digest(&client, &release, asset).await?;
    let response = client
        .get(&asset.url)
        .send()
        .await
        .map_err(|error| format!("Update download failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Update download failed: {error}"))?;
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("Update download failed: {error}"))?;
    let actual = sha256_hex(&bytes);
    if !actual.eq_ignore_ascii_case(&expected) {
        return Err("The downloaded update failed SHA-256 verification.".to_string());
    }

    let directory = cache.join(&release.version);
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|error| format!("Cannot create update cache: {error}"))?;
    let path = directory.join(safe_file_name(&asset.name)?);
    let partial = path.with_extension("download");
    tokio::fs::write(&partial, &bytes)
        .await
        .map_err(|error| format!("Cannot save update: {error}"))?;
    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        tokio::fs::remove_file(&path)
            .await
            .map_err(|error| format!("Cannot replace cached update: {error}"))?;
    }
    tokio::fs::rename(&partial, &path)
        .await
        .map_err(|error| format!("Cannot finish update download: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o755);
        tokio::fs::set_permissions(&path, permissions)
            .await
            .map_err(|error| format!("Cannot make update executable: {error}"))?;
    }
    Ok(DownloadedUpdate { release, path })
}

async fn expected_digest(
    client: &reqwest::Client,
    release: &UpdateRelease,
    asset: &UpdateAsset,
) -> Result<String, String> {
    if let Some(digest) = asset
        .digest
        .as_deref()
        .and_then(|value| value.strip_prefix("sha256:"))
    {
        return Ok(digest.to_string());
    }
    let url = release
        .checksums_url
        .as_deref()
        .ok_or_else(|| "The release does not provide a SHA-256 checksum.".to_string())?;
    let checksums = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("Checksum download failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Checksum download failed: {error}"))?
        .text()
        .await
        .map_err(|error| format!("Checksum download failed: {error}"))?;
    checksum_for(&checksums, &asset.name)
        .ok_or_else(|| format!("No checksum was published for {}.", asset.name))
}

fn checksum_for(checksums: &str, file_name: &str) -> Option<String> {
    checksums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let digest = fields.next()?;
        let name = fields.next()?.trim_start_matches('*');
        (name == file_name && digest.len() == 64).then(|| digest.to_string())
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        })
}

fn update_client(timeout: std::time::Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(format!("ApiWright/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(timeout)
        .build()
        .map_err(|error| format!("Cannot initialize update client: {error}"))
}

fn update_cache_dir() -> Result<PathBuf, String> {
    std::env::var_os("XDG_CACHE_HOME")
        .or_else(|| std::env::var_os("LOCALAPPDATA"))
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".cache"))
        })
        .map(|root| root.join("forge").join("updates"))
        .ok_or_else(|| "Cannot determine the update cache directory.".to_string())
}

fn safe_file_name(name: &str) -> Result<&str, String> {
    Path::new(name)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|safe| *safe == name && !safe.is_empty())
        .ok_or_else(|| "The release contains an unsafe package name.".to_string())
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct UpdatePreferences {
    #[serde(default)]
    skipped_release: Option<String>,
}

fn preferences_file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("forge")
            .join("updates.json"),
    )
}

fn skipped_release() -> Option<String> {
    let file = preferences_file()?;
    let text = std::fs::read_to_string(file).ok()?;
    serde_json::from_str::<UpdatePreferences>(&text)
        .ok()?
        .skipped_release
}

fn skip_release(tag: String) -> Result<(), String> {
    let file = preferences_file()
        .ok_or_else(|| "Cannot determine the update preferences path.".to_string())?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create update preferences: {error}"))?;
    }
    let text = serde_json::to_string_pretty(&UpdatePreferences {
        skipped_release: Some(tag),
    })
    .map_err(|error| format!("Cannot serialize update preferences: {error}"))?;
    std::fs::write(file, text).map_err(|error| format!("Cannot save update preferences: {error}"))
}

enum DialogAction {
    Close,
    Download(UpdateRelease),
    Skip(String),
    OpenRelease(String),
    Install(DownloadedUpdate),
}

pub fn show(ctx: &egui::Context, state: &mut UpdateState, bridge: &Bridge) {
    if !state.open {
        return;
    }
    let mut open = state.open;
    let mut action = None;
    egui::Window::new("ApiWright update")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(540.0)
        .min_width(420.0)
        .show(ctx, |ui| {
            if state.checking {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking published releases…");
                });
                return;
            }
            if let Some(error) = &state.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
                ui.add_space(8.0);
            }
            if let Some(notice) = &state.notice {
                ui.label(notice);
                if ui.button("Close").clicked() {
                    action = Some(DialogAction::Close);
                }
                return;
            }
            let Some(release) = state.release.as_ref() else {
                return;
            };
            ui.heading(&release.title);
            ui.label(format!(
                "Installed v{}  →  available v{}",
                env!("CARGO_PKG_VERSION"),
                release.version
            ));
            ui.add_space(10.0);
            ui.label(egui::RichText::new("CHANGELOG").small().strong().weak());
            egui::ScrollArea::vertical()
                .id_salt("forge-update-changelog")
                .max_height(280.0)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(&release.notes);
                });
            ui.add_space(12.0);

            if state.downloading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Downloading and verifying update…");
                });
            } else if let Some(downloaded) = state.downloaded.as_ref() {
                ui.horizontal(|ui| {
                    if ui.button(install_button_label()).clicked() {
                        action = Some(DialogAction::Install(downloaded.clone()));
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · SHA-256 verified",
                            downloaded.release.tag
                        ))
                        .weak(),
                    );
                });
            } else {
                ui.horizontal(|ui| {
                    if release.asset.is_some() && ui.button("Download update").clicked() {
                        action = Some(DialogAction::Download(release.clone()));
                    }
                    if ui.button("Open release page").clicked() {
                        action = Some(DialogAction::OpenRelease(release.page_url.clone()));
                    }
                    if ui.button("Skip this version").clicked() {
                        action = Some(DialogAction::Skip(release.tag.clone()));
                    }
                });
            }
        });
    state.open = open;

    match action {
        Some(DialogAction::Close) => state.open = false,
        Some(DialogAction::Download(release)) => {
            state.downloading = true;
            state.error = None;
            if let Err(error) = bridge.send(Cmd::DownloadUpdate { release }) {
                state.downloading = false;
                state.error = Some(error);
            }
        }
        Some(DialogAction::Skip(tag)) => match skip_release(tag) {
            Ok(()) => state.open = false,
            Err(error) => state.error = Some(error),
        },
        Some(DialogAction::OpenRelease(url)) => {
            if let Err(error) = open::that(url) {
                state.error = Some(format!("Cannot open release page: {error}"));
            }
        }
        Some(DialogAction::Install(downloaded)) => {
            match install_downloaded(&downloaded) {
                Ok(true) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Ok(false) => {
                    state.notice = Some("The installer is open. Finish the operating-system installation when ready.".to_string());
                    state.release = None;
                    state.downloaded = None;
                }
                Err(error) => state.error = Some(error),
            }
        }
        None => {}
    }
}

fn install_button_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Open installer"
    } else {
        "Restart and update"
    }
}

#[cfg(target_os = "linux")]
fn install_downloaded(downloaded: &DownloadedUpdate) -> Result<bool, String> {
    use std::process::Command;

    let target = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .ok_or_else(|| {
            "Automatic replacement is available when ApiWright runs as an AppImage. Open the release page for other installations.".to_string()
        })?;
    let script = r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done
chmod +x "$2" || exec "$3"
mv -f "$2" "$3"
exec "$3""#;
    Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg("forge-updater")
        .arg(std::process::id().to_string())
        .arg(&downloaded.path)
        .arg(target)
        .spawn()
        .map_err(|error| format!("Cannot start updater: {error}"))?;
    Ok(true)
}

#[cfg(target_os = "windows")]
fn install_downloaded(downloaded: &DownloadedUpdate) -> Result<bool, String> {
    use std::process::Command;

    let target = std::env::current_exe()
        .map_err(|error| format!("Cannot locate the installed executable: {error}"))?;
    let script = r#"param([int]$processId,[string]$source,[string]$target)
Wait-Process -Id $processId -ErrorAction SilentlyContinue
Move-Item -LiteralPath $source -Destination $target -Force
Start-Process -FilePath $target"#;
    Command::new("powershell.exe")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(script)
        .arg(std::process::id().to_string())
        .arg(&downloaded.path)
        .arg(target)
        .spawn()
        .map_err(|error| format!("Cannot start updater: {error}"))?;
    Ok(true)
}

#[cfg(target_os = "macos")]
fn install_downloaded(downloaded: &DownloadedUpdate) -> Result<bool, String> {
    open::that(&downloaded.path).map_err(|error| format!("Cannot open installer: {error}"))?;
    Ok(false)
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn install_downloaded(downloaded: &DownloadedUpdate) -> Result<bool, String> {
    open::that(&downloaded.release.page_url)
        .map_err(|error| format!("Cannot open release page: {error}"))?;
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn release_selection_is_semantic_and_platform_specific() {
        let release = serde_json::from_value::<ApiRelease>(serde_json::json!({
            "tag_name": "v0.10.0",
            "name": "ApiWright v0.10.0",
            "body": "Changes",
            "html_url": "https://example.test/release",
            "assets": [
                {"name": "ApiWright-0.10.0-linux-x86_64.AppImage", "browser_download_url": "https://example.test/linux", "digest": "sha256:aa"},
                {"name": "ApiWright-0.10.0-windows-x86_64.exe", "browser_download_url": "https://example.test/windows", "digest": "sha256:bb"},
                {"name": "SHA256SUMS.txt", "browser_download_url": "https://example.test/sums", "digest": null}
            ]
        }))
        .unwrap();

        let selected = release_from_api(release, "0.9.9", Some(PackageKind::LinuxAppImage))
            .unwrap()
            .unwrap();
        assert_eq!(selected.version, "0.10.0");
        assert_eq!(selected.asset.unwrap().url, "https://example.test/linux");
    }

    #[test]
    fn checksum_parser_requires_the_exact_asset_name() {
        let digest = "a".repeat(64);
        let text = format!(
            "{digest}  ApiWright.AppImage\n{}  Other.AppImage\n",
            "b".repeat(64)
        );
        assert_eq!(checksum_for(&text, "ApiWright.AppImage"), Some(digest));
        assert_eq!(checksum_for(&text, "Missing.AppImage"), None);
    }

    #[tokio::test]
    async fn update_check_uses_release_api_and_returns_newer_release() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/latest"))
            .and(header("accept", "application/vnd.github+json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tag_name": "v1.2.0",
                "name": "ApiWright v1.2.0",
                "body": "A useful change",
                "html_url": "https://example.test/release",
                "assets": []
            })))
            .mount(&server)
            .await;

        let release = check_for_update_at(
            &format!("{}/latest", server.uri()),
            "1.1.9",
            Some(PackageKind::LinuxAppImage),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(release.version, "1.2.0");
        assert_eq!(release.notes, "A useful change");
    }

    #[tokio::test]
    async fn downloaded_update_is_verified_before_it_is_staged() {
        let server = MockServer::start().await;
        let bytes = b"verified executable";
        Mock::given(method("GET"))
            .and(path("/ApiWright.AppImage"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .mount(&server)
            .await;
        let release = UpdateRelease {
            tag: "v1.2.0".to_string(),
            version: "1.2.0".to_string(),
            title: "ApiWright v1.2.0".to_string(),
            notes: "Changes".to_string(),
            page_url: "https://example.test/release".to_string(),
            asset: Some(UpdateAsset {
                name: "ApiWright.AppImage".to_string(),
                url: format!("{}/ApiWright.AppImage", server.uri()),
                digest: Some(format!("sha256:{}", sha256_hex(bytes))),
            }),
            checksums_url: None,
        };
        let cache = tempfile::tempdir().unwrap();

        let downloaded = download_update_to(release, cache.path()).await.unwrap();
        assert_eq!(std::fs::read(downloaded.path).unwrap(), bytes);
    }
}
