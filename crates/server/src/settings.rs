//! Persistence for the WebUI model configuration.
//!
//! Non-secret fields (provider / base_url / model / api_key_env) are written to
//! a plain JSON file under the user's config directory. The API key is the only
//! secret, and it is stored in the OS keychain (Windows Credential Manager /
//! macOS Keychain / Linux Secret Service) via the `keyring` crate — never on
//! disk in plaintext.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use reagent_core::ModelSettings;
use serde::{Deserialize, Serialize};

const KEYRING_SERVICE: &str = "ReAgent";
const KEYRING_USER: &str = "model-api-key";

/// Upper bound on how long a keyring call may block. On Linux the Secret Service
/// backend can hang indefinitely when the wallet is locked (it waits for an
/// unlock prompt), so this bounds it and lets a locked wallet degrade to
/// in-memory rather than wedging the settings endpoint.
const KEYRING_TIMEOUT: Duration = Duration::from_secs(5);

/// Run a blocking keyring operation on a helper thread with a timeout. The
/// keyring backends are synchronous and (on Linux) can block on a Secret Service
/// unlock prompt; this keeps a locked wallet from hanging the request. On
/// timeout the helper thread is abandoned — it is parked on a prompt the user
/// can still dismiss — and the operation is treated as failed.
fn keyring_op<T: Send + 'static>(op: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(op());
    });
    rx.recv_timeout(KEYRING_TIMEOUT).ok()
}

/// Store the API key in the keychain. A missing or locked keyring is non-fatal:
/// the key stays in server memory for this session only.
fn keyring_set(key: &str) {
    let key = key.to_string();
    match keyring_op(move || match keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
        Ok(entry) => entry.set_password(&key).map_err(|e| e.to_string()),
        Err(err) => Err(err.to_string()),
    }) {
        Some(Ok(())) => {}
        Some(Err(err)) => eprintln!(
            "warning: could not save API key to keyring ({err}); it will live in memory for this session only"
        ),
        None => eprintln!(
            "warning: keyring save timed out (wallet locked?); API key will live in memory for this session only"
        ),
    }
}

/// Remove the API key from the keychain. A missing entry or unavailable keyring
/// is non-fatal.
fn keyring_clear() {
    match keyring_op(
        || match keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
            Ok(entry) => entry.delete_credential().map_err(|e| e.to_string()),
            Err(err) => Err(err.to_string()),
        },
    ) {
        Some(Ok(())) => {}
        Some(Err(err)) => eprintln!("warning: could not clear API key from keyring ({err})"),
        None => eprintln!("warning: keyring clear timed out (wallet locked?)"),
    }
}

/// Config file location: `<config_dir>/reagent/settings.json`.
fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("reagent").join("settings.json"))
}

/// Load persisted settings: non-secret fields from the config file, the API key
/// from the keychain. Returns `None` when nothing is configured yet.
pub fn load_settings() -> Result<Option<ModelSettings>> {
    let Some(path) = config_path() else {
        return Ok(None);
    };

    let mut settings: Option<ModelSettings> = if path.is_file() {
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("read settings {}", path.display()))?;
        Some(
            serde_json::from_str(&raw)
                .with_context(|| format!("parse settings {}", path.display()))?,
        )
    } else {
        None
    };

    match keyring_op(|| {
        match keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
            Ok(entry) => match entry.get_password() {
                Ok(key) if !key.is_empty() => Some(key),
                Ok(_) => None, // empty key: treat as unset
                Err(err) => {
                    eprintln!(
                        "warning: could not read API key from keyring ({err}); using env vars"
                    );
                    None
                }
            },
            Err(err) => {
                eprintln!("warning: keyring unavailable ({err}); using env vars");
                None
            }
        }
    }) {
        Some(Some(key)) => {
            settings.get_or_insert_with(ModelSettings::default).api_key = Some(key);
        }
        Some(None) => {} // no key stored, or keyring unavailable: fall back to env
        None => {
            eprintln!("warning: keyring lookup timed out (wallet locked?); using env vars");
        }
    }

    Ok(settings.filter(|s| !s.provider.is_empty()))
}

/// Persist `settings`. The `api_key` field is `skip_serializing`, so writing the
/// config file never emits it; only the keychain receives it. When `clear_key`
/// is set the keychain entry is removed instead.
pub fn save_settings(settings: &ModelSettings, clear_key: bool) -> Result<()> {
    if let Some(path) = config_path() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        let json = serde_json::to_string_pretty(settings).context("serialize settings")?;
        std::fs::write(&path, json)
            .with_context(|| format!("write settings {}", path.display()))?;
    }

    if clear_key {
        keyring_clear();
    } else if let Some(key) = &settings.api_key {
        if key.is_empty() {
            keyring_clear();
        } else {
            keyring_set(key);
        }
    }
    // api_key == None means "keep the existing key": do nothing.

    Ok(())
}

/// What `GET /api/settings` returns — non-secret fields plus a masked key hint,
/// never the full key.
#[derive(Debug, Serialize)]
pub struct SettingsResponse {
    pub provider: String,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub api_key_env: Option<String>,
    pub api_key_set: bool,
    pub api_key_hint: Option<String>,
}

pub fn describe(settings: &ModelSettings) -> SettingsResponse {
    let api_key_set = settings
        .api_key
        .as_ref()
        .map(|key| !key.is_empty())
        .unwrap_or(false);
    SettingsResponse {
        provider: settings.provider.clone(),
        base_url: settings.base_url.clone(),
        model: settings.model.clone(),
        api_key_env: settings.api_key_env.clone(),
        api_key_set,
        api_key_hint: settings.api_key.as_deref().and_then(mask_key),
    }
}

/// Request body for `PUT /api/settings`. Flattens `ModelSettings` so a client
/// sends the same field names, plus an optional `clear_api_key` flag.
#[derive(Debug, Deserialize)]
pub struct SettingsUpdate {
    #[serde(flatten)]
    pub settings: ModelSettings,
    #[serde(default)]
    pub clear_api_key: bool,
}

/// A short masked hint like `sk-••••abcd`, revealing only the first 3 and last
/// 4 characters so the user can recognise which key is stored.
fn mask_key(key: &str) -> Option<String> {
    if key.len() <= 8 {
        return Some("••••••".to_string());
    }
    Some(format!("{}••••{}", &key[..3], &key[key.len() - 4..]))
}
