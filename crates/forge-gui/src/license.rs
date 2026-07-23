//! License & Billing: the Free/Pro/Enterprise tier model, license-key
//! activation against a license server and the License & Billing dialog.
//!
//! Free needs no key and is limited to noncommercial use by the LICENSE
//! terms. Pro keys validate against the hosted ApiWright license server;
//! Enterprise keys validate against the customer's own license server
//! (same protocol, different base URL). Either way the server's answer is
//! only trusted if it carries a license payload signed with the ApiWright
//! product signing key — the Ed25519 public half is embedded below, so
//! neither a spoofed license server nor a hand-edited cache file can mint
//! an entitlement. The last verified payload is cached in
//! `~/.config/forge/license.json` (and re-verified on every load), so the
//! app stays licensed offline until the paid period (`valid_until`) ends.

use std::path::PathBuf;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use chrono::{DateTime, Duration, Utc};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::bridge::{Bridge, Cmd};
use crate::state::AppState;

/// The license server Pro subscriptions validate against.
pub const HOSTED_LICENSE_SERVER: &str = "https://license.ericvogt.com";
/// Where "Buy ApiWright Pro" leads.
pub const BUY_URL: &str = "https://ericvogt.com/apiwright/pricing";
/// Where "Contact" for Enterprise leads.
pub const ENTERPRISE_CONTACT_URL: &str = "https://github.com/EricVogt93";
/// Display price of a Pro seat.
pub const PRO_PRICE: &str = "12 € per user / month";

/// Ed25519 public key of the "forge" product on the license server. Licenses
/// are only honored when their payload verifies against this key.
const PRODUCT_PUBLIC_KEY_B64: &str = "bnbhgDVqqIw8ja7pYOXkWL6WQP5HNeWDs5gWMhDQE/8=";
/// The product slug this build accepts licenses for.
const PRODUCT: &str = "forge";

/// How long a cached validation is trusted before the app re-checks in the
/// background on startup.
const RECHECK_AFTER_HOURS: i64 = 24;

/// Length of the free commercial evaluation available on the Free plan.
pub const TRIAL_DAYS: i64 = 60;

const LICENSE_FILE: &str = "license.json";
const TRIAL_FILE: &str = "trial.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Free,
    Paid,
    Enterprise,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Free => "Free",
            Tier::Paid => "Pro",
            Tier::Enterprise => "Enterprise",
        }
    }
}

/// The signed entitlement issued by the license server.
#[derive(Debug, Clone, Deserialize)]
pub struct LicensePayload {
    pub key: String,
    pub product: String,
    pub tier: Tier,
    pub licensed_to: String,
    /// End of the paid period. Beyond this the cached license no longer
    /// counts and the app falls back to Free.
    pub valid_until: DateTime<Utc>,
}

/// A verified license as held in memory: the raw signed blob (kept for
/// re-saving) plus its parsed payload.
#[derive(Debug, Clone)]
pub struct StoredLicense {
    blob: String,
    signature: String,
    /// Enterprise license-server base URL; `None` means the hosted server.
    pub server: Option<String>,
    pub last_check: DateTime<Utc>,
    pub payload: LicensePayload,
}

impl StoredLicense {
    pub fn server_url(&self) -> &str {
        self.server.as_deref().unwrap_or(HOSTED_LICENSE_SERVER)
    }

    fn active(&self, now: DateTime<Utc>) -> bool {
        now < self.payload.valid_until
    }
}

/// What `license.json` holds on disk. The entitlement itself lives inside
/// the signed `license` blob and is re-verified on every load.
#[derive(Serialize, Deserialize)]
struct LicenseFile {
    license: String,
    signature: String,
    #[serde(default)]
    server: Option<String>,
    last_check: DateTime<Utc>,
}

/// The tier the app runs under right now: the stored license's tier while
/// its paid period lasts, Free otherwise.
pub fn effective_tier(license: Option<&StoredLicense>, now: DateTime<Utc>) -> Tier {
    match license {
        Some(license) if license.active(now) => license.payload.tier,
        _ => Tier::Free,
    }
}

// ---------------------------------------------------------------------
// Signature verification
// ---------------------------------------------------------------------

fn product_public_key() -> VerifyingKey {
    // Both are compile-time constants; a failure here is a build defect,
    // not a runtime condition.
    let bytes: [u8; 32] = BASE64
        .decode(PRODUCT_PUBLIC_KEY_B64)
        .expect("embedded product key is valid base64")
        .try_into()
        .expect("embedded product key is 32 bytes");
    VerifyingKey::from_bytes(&bytes).expect("embedded product key is a valid Ed25519 point")
}

/// Decode and verify a signed license blob; only a payload signed with
/// `public_key` and issued for this product comes back as `Ok`.
fn verify_and_parse(
    blob_b64: &str,
    signature_b64: &str,
    public_key: &VerifyingKey,
) -> Result<LicensePayload, String> {
    let blob = BASE64
        .decode(blob_b64)
        .map_err(|_| "The license payload is not valid base64.".to_string())?;
    let signature = BASE64
        .decode(signature_b64)
        .map_err(|_| "The license signature is not valid base64.".to_string())?;
    let signature = Signature::from_slice(&signature)
        .map_err(|_| "The license signature has the wrong length.".to_string())?;
    public_key
        .verify_strict(&blob, &signature)
        .map_err(|_| "The license is not signed by the ApiWright product key.".to_string())?;
    let payload: LicensePayload = serde_json::from_slice(&blob)
        .map_err(|error| format!("Invalid license payload: {error}"))?;
    if payload.product != PRODUCT {
        return Err(format!(
            "The license was issued for '{}', not '{PRODUCT}'.",
            payload.product
        ));
    }
    Ok(payload)
}

// ---------------------------------------------------------------------
// Local storage
// ---------------------------------------------------------------------

fn config_file(name: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".config").join("forge").join(name))
}

fn load_license() -> Option<StoredLicense> {
    let text = std::fs::read_to_string(config_file(LICENSE_FILE)?).ok()?;
    let file: LicenseFile = serde_json::from_str(&text).ok()?;
    // A cache that no longer verifies (tampered, or written by an older
    // build with a different format) is treated as absent.
    let payload = verify_and_parse(&file.license, &file.signature, &product_public_key()).ok()?;
    Some(StoredLicense {
        blob: file.license,
        signature: file.signature,
        server: file.server,
        last_check: file.last_check,
        payload,
    })
}

fn save_license(license: &StoredLicense) -> Result<(), String> {
    let file = config_file(LICENSE_FILE)
        .ok_or_else(|| "Cannot determine the license path.".to_string())?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create the config directory: {error}"))?;
    }
    let text = serde_json::to_string_pretty(&LicenseFile {
        license: license.blob.clone(),
        signature: license.signature.clone(),
        server: license.server.clone(),
        last_check: license.last_check,
    })
    .map_err(|error| format!("Cannot serialize the license: {error}"))?;
    std::fs::write(file, text).map_err(|error| format!("Cannot save the license: {error}"))
}

fn remove_license() -> Result<(), String> {
    let Some(file) = config_file(LICENSE_FILE) else {
        return Ok(());
    };
    match std::fs::remove_file(file) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Cannot remove the license: {error}")),
    }
}

// ---------------------------------------------------------------------
// Commercial trial
// ---------------------------------------------------------------------

/// The 60-day commercial evaluation on the Free plan. Stored locally and
/// unenforced, like the noncommercial terms themselves — it marks the date
/// the evaluation started, nothing more.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Trial {
    started: DateTime<Utc>,
}

fn load_trial() -> Option<DateTime<Utc>> {
    let text = std::fs::read_to_string(config_file(TRIAL_FILE)?).ok()?;
    serde_json::from_str::<Trial>(&text).ok().map(|t| t.started)
}

/// Start the trial now, unless one was already started — the existing start
/// date always wins, so clicking the button again never resets the clock.
fn start_trial() -> Result<DateTime<Utc>, String> {
    if let Some(started) = load_trial() {
        return Ok(started);
    }
    let file =
        config_file(TRIAL_FILE).ok_or_else(|| "Cannot determine the trial path.".to_string())?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create the config directory: {error}"))?;
    }
    let started = Utc::now();
    let text = serde_json::to_string_pretty(&Trial { started })
        .map_err(|error| format!("Cannot serialize the trial: {error}"))?;
    std::fs::write(file, text).map_err(|error| format!("Cannot save the trial: {error}"))?;
    Ok(started)
}

/// Days of commercial evaluation left; zero or negative means expired.
pub fn trial_days_left(started: DateTime<Utc>, now: DateTime<Utc>) -> i64 {
    TRIAL_DAYS - (now - started).num_days()
}

// ---------------------------------------------------------------------
// Server protocol
// ---------------------------------------------------------------------

/// Outcome of asking a license server about a key. `Rejected` is the
/// server's verdict; transport failures and unverifiable responses surface
/// as `Err` on `validate` so callers can keep a cached license through
/// network trouble.
#[derive(Debug)]
pub enum Validation {
    Valid(StoredLicense),
    Rejected(String),
}

#[derive(Debug, Deserialize)]
struct ValidateResponse {
    valid: bool,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// Ask the license server about `key`. `server` is the Enterprise base URL;
/// `None` validates against the hosted server.
pub async fn validate(key: String, server: Option<String>) -> Result<Validation, String> {
    let base = server
        .clone()
        .unwrap_or_else(|| HOSTED_LICENSE_SERVER.to_string());
    validate_at(&base, key, server, &product_public_key()).await
}

async fn validate_at(
    base: &str,
    key: String,
    server: Option<String>,
    public_key: &VerifyingKey,
) -> Result<Validation, String> {
    let client = reqwest::Client::builder()
        .user_agent(format!("ApiWright/{}", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Cannot build the HTTP client: {error}"))?;
    let response = client
        .post(format!(
            "{}/v1/licenses/validate",
            base.trim_end_matches('/')
        ))
        .json(&serde_json::json!({
            "key": key,
            "product": PRODUCT,
            "version": env!("CARGO_PKG_VERSION"),
        }))
        .send()
        .await
        .map_err(|error| format!("License server unreachable: {error}"))?
        .error_for_status()
        .map_err(|error| format!("License server error: {error}"))?;
    let response = response
        .json::<ValidateResponse>()
        .await
        .map_err(|error| format!("Invalid license server response: {error}"))?;

    if !response.valid {
        return Ok(Validation::Rejected(
            response
                .message
                .unwrap_or_else(|| "The license key was rejected.".to_string()),
        ));
    }
    let (blob, signature) = match (response.license, response.signature) {
        (Some(blob), Some(signature)) => (blob, signature),
        _ => return Err("The license server response carries no signed license.".to_string()),
    };
    let payload = verify_and_parse(&blob, &signature, public_key)?;
    if payload.key != key {
        return Err("The signed license does not match the requested key.".to_string());
    }
    Ok(Validation::Valid(StoredLicense {
        blob,
        signature,
        server,
        last_check: Utc::now(),
        payload,
    }))
}

// ---------------------------------------------------------------------
// Dialog state
// ---------------------------------------------------------------------

#[derive(Default)]
pub struct LicenseState {
    pub open: bool,
    pub checking: bool,
    license: Option<StoredLicense>,
    trial_started: Option<DateTime<Utc>>,
    loaded: bool,
    key_input: String,
    server_input: String,
    error: Option<String>,
    notice: Option<String>,
}

impl LicenseState {
    fn ensure_loaded(&mut self) {
        if !self.loaded {
            self.license = load_license();
            self.trial_started = load_trial();
            self.loaded = true;
        }
    }

    /// Entitlement check for Pro-gated features: an active paid tier, or a
    /// running commercial trial.
    pub fn pro_features(&mut self) -> bool {
        self.ensure_loaded();
        if effective_tier(self.license.as_ref(), Utc::now()) != Tier::Free {
            return true;
        }
        self.trial_started
            .is_some_and(|started| trial_days_left(started, Utc::now()) > 0)
    }

    pub fn open_dialog(&mut self) {
        self.ensure_loaded();
        if let Some(license) = &self.license {
            self.server_input = license.server.clone().unwrap_or_default();
        }
        self.error = None;
        self.notice = None;
        self.open = true;
    }

    /// Silent startup re-check: only when a license is stored and the last
    /// check is old enough. Transport failures keep the cached license.
    pub fn revalidate_on_start(&mut self, bridge: &Bridge) {
        self.ensure_loaded();
        let Some(license) = &self.license else {
            return;
        };
        if Utc::now() - license.last_check < Duration::hours(RECHECK_AFTER_HOURS) {
            return;
        }
        self.checking = true;
        let cmd = Cmd::ValidateLicense {
            manual: false,
            key: license.payload.key.clone(),
            server: license.server.clone(),
        };
        if bridge.send(cmd).is_err() {
            self.checking = false;
        }
    }

    fn activate(&mut self, bridge: &Bridge) {
        let key = self.key_input.trim().to_string();
        if key.is_empty() {
            self.error = Some("Enter a license key.".to_string());
            return;
        }
        let server = match self.server_input.trim() {
            "" => None,
            url if url.starts_with("http://") || url.starts_with("https://") => {
                Some(url.to_string())
            }
            _ => {
                self.error = Some("The license server must be an http(s) URL.".to_string());
                return;
            }
        };
        self.checking = true;
        self.error = None;
        self.notice = None;
        if let Err(error) = bridge.send(Cmd::ValidateLicense {
            manual: true,
            key,
            server,
        }) {
            self.checking = false;
            self.error = Some(error);
        }
    }

    pub fn handle_validated(&mut self, manual: bool, result: Result<Validation, String>) {
        self.checking = false;
        match result {
            Ok(Validation::Valid(license)) => {
                if let Err(error) = save_license(&license) {
                    self.error = Some(error);
                }
                if manual {
                    self.notice = Some(format!(
                        "{} license activated.",
                        license.payload.tier.label()
                    ));
                    self.key_input.clear();
                }
                self.license = Some(license);
            }
            Ok(Validation::Rejected(message)) => {
                // The server explicitly refused the stored key: drop it.
                if !manual {
                    let _ = remove_license();
                    self.license = None;
                }
                self.error = Some(message);
                if !manual {
                    self.open = true;
                }
            }
            Err(error) if manual => self.error = Some(error),
            // Silent check, server unreachable or answer unverifiable: keep
            // the cached license until valid_until runs out.
            Err(_) => {}
        }
    }

    fn deactivate(&mut self) {
        match remove_license() {
            Ok(()) => {
                self.license = None;
                self.notice = Some("License removed. ApiWright is on the Free plan.".to_string());
            }
            Err(error) => self.error = Some(error),
        }
    }
}

// ---------------------------------------------------------------------
// Dialog UI
// ---------------------------------------------------------------------

enum DialogAction {
    None,
    Activate,
    Revalidate,
    Deactivate,
    StartTrial,
    OpenUrl(&'static str),
}

pub fn show(ctx: &egui::Context, state: &mut AppState, bridge: &Bridge) {
    let dialog = &mut state.dialogs.license;
    if !dialog.open {
        return;
    }
    dialog.ensure_loaded();
    let mut open = dialog.open;
    let mut action = DialogAction::None;
    egui::Window::new("License & Billing")
        .id(egui::Id::new("license-dialog"))
        .collapsible(false)
        .resizable(false)
        .default_width(440.0)
        .open(&mut open)
        .show(ctx, |ui| {
            let tier = effective_tier(dialog.license.as_ref(), Utc::now());
            ui.heading(format!("Current plan: {}", tier.label()));
            if let Some(license) = &dialog.license {
                egui::Grid::new("license-grid")
                    .num_columns(2)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        if !license.payload.licensed_to.is_empty() {
                            ui.label("Licensed to");
                            ui.monospace(&license.payload.licensed_to);
                            ui.end_row();
                        }
                        ui.label(if license.active(Utc::now()) {
                            "Paid until"
                        } else {
                            "Expired on"
                        });
                        ui.monospace(license.payload.valid_until.format("%Y-%m-%d").to_string());
                        ui.end_row();
                        ui.label("License server");
                        ui.monospace(license.server_url());
                        ui.end_row();
                    });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!dialog.checking, egui::Button::new("Validate now"))
                        .clicked()
                    {
                        action = DialogAction::Revalidate;
                    }
                    if ui
                        .add_enabled(!dialog.checking, egui::Button::new("Remove license"))
                        .clicked()
                    {
                        action = DialogAction::Deactivate;
                    }
                });
            } else {
                ui.label("Free is for personal and other noncommercial use only (see LICENSE).");
                ui.label("Any commercial use requires a Pro or Enterprise license.");
                ui.add_space(4.0);
                match dialog.trial_started {
                    None => {
                        ui.label(format!(
                            "Evaluating ApiWright for commercial use? Free for {TRIAL_DAYS} days, no key or account required."
                        ));
                        if ui.button(format!("Start {TRIAL_DAYS}-day commercial trial")).clicked() {
                            action = DialogAction::StartTrial;
                        }
                    }
                    Some(started) => {
                        let left = trial_days_left(started, Utc::now());
                        if left > 0 {
                            ui.strong(format!("Commercial trial active — {left} days left."));
                        } else {
                            ui.colored_label(
                                ui.visuals().warn_fg_color,
                                format!(
                                    "The commercial trial expired on {}. Commercial use now requires Pro.",
                                    (started + Duration::days(TRIAL_DAYS)).format("%Y-%m-%d")
                                ),
                            );
                        }
                    }
                }
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.strong(format!("ApiWright Pro — {PRO_PRICE}"));
            ui.label("Commercial use, billed monthly, cancel anytime.");
            if ui.button("Buy ApiWright Pro…").clicked() {
                action = DialogAction::OpenUrl(BUY_URL);
            }
            ui.add_space(6.0);
            ui.strong("ApiWright Enterprise");
            ui.label("Self-hosted license server, volume pricing and invoicing.");
            if ui.button("Contact for Enterprise…").clicked() {
                action = DialogAction::OpenUrl(ENTERPRISE_CONTACT_URL);
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.strong("Activate a license");
            ui.horizontal(|ui| {
                ui.label("License key");
                ui.add(
                    egui::TextEdit::singleline(&mut dialog.key_input)
                        .desired_width(280.0)
                        .hint_text("FORGE-…"),
                );
            });
            ui.horizontal(|ui| {
                ui.label("License server")
                    .on_hover_text("Enterprise only — leave empty for the hosted server");
                ui.add(
                    egui::TextEdit::singleline(&mut dialog.server_input)
                        .desired_width(280.0)
                        .hint_text(HOSTED_LICENSE_SERVER),
                );
            });
            if ui
                .add_enabled(
                    !dialog.checking,
                    egui::Button::new(if dialog.checking {
                        "Validating…"
                    } else {
                        "Activate"
                    }),
                )
                .clicked()
            {
                action = DialogAction::Activate;
            }

            if let Some(error) = &dialog.error {
                ui.add_space(4.0);
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if let Some(notice) = &dialog.notice {
                ui.add_space(4.0);
                ui.weak(notice);
            }
        });

    match action {
        DialogAction::None => {}
        DialogAction::Activate => dialog.activate(bridge),
        DialogAction::Revalidate => {
            if let Some(license) = &dialog.license {
                dialog.checking = true;
                dialog.error = None;
                dialog.notice = None;
                let cmd = Cmd::ValidateLicense {
                    manual: true,
                    key: license.payload.key.clone(),
                    server: license.server.clone(),
                };
                if let Err(error) = bridge.send(cmd) {
                    dialog.checking = false;
                    dialog.error = Some(error);
                }
            }
        }
        DialogAction::Deactivate => dialog.deactivate(),
        DialogAction::StartTrial => match start_trial() {
            Ok(started) => {
                dialog.trial_started = Some(started);
                dialog.notice = Some(format!(
                    "Commercial trial started — valid until {}.",
                    (started + Duration::days(TRIAL_DAYS)).format("%Y-%m-%d")
                ));
            }
            Err(error) => dialog.error = Some(error),
        },
        DialogAction::OpenUrl(url) => {
            if let Err(error) = open::that(url) {
                dialog.error = Some(format!("Cannot open {url}: {error}"));
            }
        }
    }
    dialog.open = open;
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn sign_payload(payload: serde_json::Value, signing_key: &SigningKey) -> (String, String) {
        let bytes = serde_json::to_vec(&payload).expect("payload serializes");
        let signature = signing_key.sign(&bytes);
        (BASE64.encode(&bytes), BASE64.encode(signature.to_bytes()))
    }

    fn stored(tier: Tier, valid_until: DateTime<Utc>) -> StoredLicense {
        StoredLicense {
            blob: String::new(),
            signature: String::new(),
            server: None,
            last_check: Utc::now(),
            payload: LicensePayload {
                key: "FORGE-TEST".to_string(),
                product: PRODUCT.to_string(),
                tier,
                licensed_to: "ACME".to_string(),
                valid_until,
            },
        }
    }

    #[test]
    fn effective_tier_falls_back_to_free_after_the_paid_period() {
        let now = Utc::now();
        assert_eq!(effective_tier(None, now), Tier::Free);
        assert_eq!(
            effective_tier(Some(&stored(Tier::Paid, now + Duration::days(20))), now),
            Tier::Paid
        );
        assert_eq!(
            effective_tier(
                Some(&stored(Tier::Enterprise, now - Duration::days(1))),
                now
            ),
            Tier::Free
        );
    }

    #[test]
    fn trial_expires_after_exactly_sixty_days() {
        let now = Utc::now();
        assert_eq!(trial_days_left(now, now), 60);
        assert_eq!(trial_days_left(now - Duration::days(59), now), 1);
        assert_eq!(trial_days_left(now - Duration::days(60), now), 0);
        assert_eq!(trial_days_left(now - Duration::days(61), now), -1);
    }

    #[test]
    fn only_payloads_signed_by_the_product_key_verify() {
        let signing_key = test_signing_key();
        let public_key = signing_key.verifying_key();
        let payload = serde_json::json!({
            "key": "FORGE-OK",
            "product": "forge",
            "tier": "paid",
            "licensed_to": "eric@example.test",
            "valid_until": "2027-01-01T00:00:00Z",
        });
        let (blob, signature) = sign_payload(payload.clone(), &signing_key);

        let parsed = verify_and_parse(&blob, &signature, &public_key).expect("valid signature");
        assert_eq!(parsed.tier, Tier::Paid);
        assert_eq!(parsed.licensed_to, "eric@example.test");

        // Signed by a different key: refused.
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let (blob2, signature2) = sign_payload(payload.clone(), &other);
        let error = verify_and_parse(&blob2, &signature2, &public_key).unwrap_err();
        assert!(error.contains("not signed"), "got: {error}");

        // Payload tampered after signing (tier upgraded): refused.
        let mut upgraded = payload;
        upgraded["tier"] = serde_json::json!("enterprise");
        let tampered_blob = BASE64.encode(serde_json::to_vec(&upgraded).unwrap());
        let error = verify_and_parse(&tampered_blob, &signature, &public_key).unwrap_err();
        assert!(error.contains("not signed"), "got: {error}");

        // Signed for another product: refused.
        let (blob3, signature3) = sign_payload(
            serde_json::json!({
                "key": "FORGE-OK",
                "product": "other-app",
                "tier": "paid",
                "licensed_to": "x",
                "valid_until": "2027-01-01T00:00:00Z",
            }),
            &signing_key,
        );
        let error = verify_and_parse(&blob3, &signature3, &public_key).unwrap_err();
        assert!(error.contains("issued for"), "got: {error}");
    }

    #[tokio::test]
    async fn validation_accepts_a_signed_paid_key() {
        let signing_key = test_signing_key();
        let (blob, signature) = sign_payload(
            serde_json::json!({
                "key": "FORGE-OK",
                "product": "forge",
                "tier": "paid",
                "licensed_to": "eric@example.test",
                "valid_until": "2027-01-01T00:00:00Z",
            }),
            &signing_key,
        );
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/licenses/validate"))
            .and(body_partial_json(
                serde_json::json!({"key": "FORGE-OK", "product": "forge"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "valid": true,
                "license": blob,
                "signature": signature,
            })))
            .mount(&server)
            .await;

        let validation = validate_at(
            &server.uri(),
            "FORGE-OK".to_string(),
            Some(server.uri()),
            &signing_key.verifying_key(),
        )
        .await
        .expect("server is reachable");
        let Validation::Valid(license) = validation else {
            panic!("expected a valid license, got {validation:?}");
        };
        assert_eq!(license.payload.tier, Tier::Paid);
        assert_eq!(license.payload.licensed_to, "eric@example.test");
        assert_eq!(license.server.as_deref(), Some(server.uri().as_str()));
    }

    #[tokio::test]
    async fn a_spoofed_server_without_the_signing_key_cannot_mint_licenses() {
        // The "server" answers valid:true but signs with its own key.
        let attacker_key = SigningKey::from_bytes(&[13u8; 32]);
        let (blob, signature) = sign_payload(
            serde_json::json!({
                "key": "FORGE-FAKE",
                "product": "forge",
                "tier": "enterprise",
                "licensed_to": "totally legit",
                "valid_until": "2099-01-01T00:00:00Z",
            }),
            &attacker_key,
        );
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/licenses/validate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "valid": true,
                "license": blob,
                "signature": signature,
            })))
            .mount(&server)
            .await;

        let error = validate_at(
            &server.uri(),
            "FORGE-FAKE".to_string(),
            Some(server.uri()),
            &test_signing_key().verifying_key(),
        )
        .await
        .expect_err("a foreign signature must not validate");
        assert!(error.contains("not signed"), "got: {error}");
    }

    #[tokio::test]
    async fn validation_reports_a_rejected_key_with_the_server_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/licenses/validate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "valid": false,
                "message": "subscription cancelled",
            })))
            .mount(&server)
            .await;

        let validation = validate_at(
            &server.uri(),
            "FORGE-DEAD".to_string(),
            None,
            &test_signing_key().verifying_key(),
        )
        .await
        .expect("server is reachable");
        let Validation::Rejected(message) = validation else {
            panic!("expected a rejection, got {validation:?}");
        };
        assert_eq!(message, "subscription cancelled");
    }

    #[tokio::test]
    async fn transport_failures_are_errors_not_rejections() {
        // RFC 2606: .invalid never resolves, so this fails in DNS.
        let error = validate_at(
            "http://license.invalid",
            "FORGE-OK".to_string(),
            None,
            &test_signing_key().verifying_key(),
        )
        .await
        .expect_err("license.invalid must not resolve");
        assert!(
            error.contains("unreachable"),
            "expected a transport error, got: {error}"
        );
    }
}
