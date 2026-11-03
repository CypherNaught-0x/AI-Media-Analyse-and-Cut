//! The API keys (the LLM provider's and OpenRouter's), kept in the OS credential store (macOS Keychain, Windows
//! Credential Manager, Linux Secret Service) instead of the webview's
//! localStorage. The backend reads it when it makes a request, so the key
//! never travels over IPC after it has been saved.

use crate::error::AppError;
use std::sync::{Mutex, OnceLock};

const SERVICE: &str = "itemis.ai-media-cutter";
const API_KEY_ACCOUNT: &str = "llm-api-key";
/// OpenRouter serves both speech-to-text and LLM requests, so its key is kept
/// apart from the LLM provider's key: a Gemini user can still transcribe
/// through OpenRouter.
const OPENROUTER_KEY_ACCOUNT: &str = "openrouter-api-key";

/// Where secrets are stored; the OS store in the app, memory in tests.
pub(crate) trait SecretStore: Send + Sync {
    fn get(&self) -> Result<Option<String>, String>;
    fn set(&self, value: &str) -> Result<(), String>;
    fn delete(&self) -> Result<(), String>;
}

struct KeyringStore {
    account: &'static str,
}

impl KeyringStore {
    fn entry(&self) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, self.account)
            .map_err(|error| format!("The system credential store is unavailable: {error}"))
    }
}

impl SecretStore for KeyringStore {
    fn get(&self) -> Result<Option<String>, String> {
        match self.entry()?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(format!(
                "Failed to read the API key from the keychain: {error}"
            )),
        }
    }

    fn set(&self, value: &str) -> Result<(), String> {
        self.entry()?
            .set_password(value)
            .map_err(|error| format!("Failed to save the API key to the keychain: {error}"))
    }

    fn delete(&self) -> Result<(), String> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(format!(
                "Failed to remove the API key from the keychain: {error}"
            )),
        }
    }
}

/// The API key with an in-memory cache, so the credential store (which may
/// ask the user for permission) is read at most once per session.
pub(crate) struct ApiKey {
    store: Box<dyn SecretStore>,
    /// `None` until loaded; then the stored key, if any.
    cached: Mutex<Option<Option<String>>>,
    /// Shown when a request needs the key and none is stored.
    missing_message: &'static str,
}

impl ApiKey {
    pub(crate) fn new(store: Box<dyn SecretStore>) -> Self {
        Self::new_named(store, "No API key is configured. Add one in Settings.")
    }

    pub(crate) fn new_named(store: Box<dyn SecretStore>, missing_message: &'static str) -> Self {
        Self {
            store,
            cached: Mutex::new(None),
            missing_message,
        }
    }

    fn load(&self) -> Result<Option<String>, String> {
        let mut cached = self.cached.lock().expect("api key cache poisoned");
        if let Some(value) = cached.as_ref() {
            return Ok(value.clone());
        }
        let value = self.store.get()?.filter(|key| !key.trim().is_empty());
        *cached = Some(value.clone());
        Ok(value)
    }

    /// The key for a request, or an error telling the user to add one.
    pub(crate) fn require(&self) -> Result<String, AppError> {
        self.load()?
            .ok_or_else(|| AppError::failed(self.missing_message))
    }

    pub(crate) fn is_set(&self) -> Result<bool, AppError> {
        Ok(self.load()?.is_some())
    }

    pub(crate) fn set(&self, key: &str) -> Result<(), AppError> {
        let key = key.trim();
        if key.is_empty() {
            return Err(AppError::failed("The API key is empty"));
        }
        self.store.set(key)?;
        *self.cached.lock().expect("api key cache poisoned") = Some(Some(key.to_string()));
        Ok(())
    }

    pub(crate) fn clear(&self) -> Result<(), AppError> {
        self.store.delete()?;
        *self.cached.lock().expect("api key cache poisoned") = Some(None);
        Ok(())
    }
}

/// The LLM provider's API key, backed by the OS credential store.
pub(crate) fn api_key() -> &'static ApiKey {
    static API_KEY: OnceLock<ApiKey> = OnceLock::new();
    API_KEY.get_or_init(|| {
        ApiKey::new(Box::new(KeyringStore {
            account: API_KEY_ACCOUNT,
        }))
    })
}

/// The OpenRouter API key, backed by the OS credential store.
pub(crate) fn openrouter_api_key() -> &'static ApiKey {
    static API_KEY: OnceLock<ApiKey> = OnceLock::new();
    API_KEY.get_or_init(|| {
        ApiKey::new_named(
            Box::new(KeyringStore {
                account: OPENROUTER_KEY_ACCOUNT,
            }),
            "No OpenRouter API key is configured. Add one in Settings.",
        )
    })
}

/// Whether `base_url` points at OpenRouter (`openrouter.ai` or a regional
/// domain such as `eu.openrouter.ai`).
pub(crate) fn is_openrouter_url(base_url: &str) -> bool {
    let without_scheme = base_url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let host = without_scheme
        .split(['/', ':', '?'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    host == "openrouter.ai" || host.ends_with(".openrouter.ai")
}

/// The key for a request to `base_url`: OpenRouter's for OpenRouter, the LLM
/// provider's otherwise.
pub(crate) fn key_for_base_url(base_url: &str) -> Result<String, AppError> {
    if is_openrouter_url(base_url) {
        openrouter_api_key().require()
    } else {
        api_key().require()
    }
}

#[tauri::command]
#[specta::specta]
pub fn set_openrouter_api_key(key: String) -> Result<(), AppError> {
    openrouter_api_key().set(&key)
}

#[tauri::command]
#[specta::specta]
pub fn clear_openrouter_api_key() -> Result<(), AppError> {
    openrouter_api_key().clear()
}

#[tauri::command]
#[specta::specta]
pub fn has_openrouter_api_key() -> Result<bool, AppError> {
    openrouter_api_key().is_set()
}

#[tauri::command]
#[specta::specta]
pub fn set_api_key(key: String) -> Result<(), AppError> {
    api_key().set(&key)
}

#[tauri::command]
#[specta::specta]
pub fn clear_api_key() -> Result<(), AppError> {
    api_key().clear()
}

#[tauri::command]
#[specta::specta]
pub fn has_api_key() -> Result<bool, AppError> {
    api_key().is_set()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Default)]
    struct MemoryStore {
        value: Mutex<Option<String>>,
        reads: Arc<AtomicUsize>,
    }

    impl SecretStore for MemoryStore {
        fn get(&self) -> Result<Option<String>, String> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(self.value.lock().unwrap().clone())
        }
        fn set(&self, value: &str) -> Result<(), String> {
            *self.value.lock().unwrap() = Some(value.to_string());
            Ok(())
        }
        fn delete(&self) -> Result<(), String> {
            *self.value.lock().unwrap() = None;
            Ok(())
        }
    }

    #[test]
    fn a_missing_key_tells_the_user_where_to_add_one() {
        let key = ApiKey::new(Box::<MemoryStore>::default());
        assert!(!key.is_set().unwrap());
        assert!(key.require().unwrap_err().message.contains("Settings"));
    }

    #[test]
    fn the_store_is_read_once_per_session() {
        let store = MemoryStore::default();
        *store.value.lock().unwrap() = Some("sk-123".into());
        let reads = store.reads.clone();
        let key = ApiKey::new(Box::new(store));

        assert_eq!(key.require().unwrap(), "sk-123");
        assert_eq!(key.require().unwrap(), "sk-123");
        assert!(key.is_set().unwrap());
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn openrouter_urls_are_recognised_by_host() {
        assert!(is_openrouter_url("https://openrouter.ai/api"));
        assert!(is_openrouter_url("https://eu.openrouter.ai/api/v1"));
        assert!(is_openrouter_url("openrouter.ai"));
        assert!(!is_openrouter_url(
            "https://generativelanguage.googleapis.com"
        ));
        assert!(!is_openrouter_url("https://openrouter.ai.example.com/api"));
        assert!(!is_openrouter_url("https://example.com/openrouter.ai"));
    }

    #[test]
    fn set_trims_and_clear_removes() {
        let key = ApiKey::new(Box::<MemoryStore>::default());
        assert!(key.set("   ").is_err());
        key.set("  sk-456\n").unwrap();
        assert_eq!(key.require().unwrap(), "sk-456");
        key.clear().unwrap();
        assert!(!key.is_set().unwrap());
    }
}
