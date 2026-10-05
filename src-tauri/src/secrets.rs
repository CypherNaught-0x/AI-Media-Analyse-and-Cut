//! The LLM API key, kept in the OS credential store (macOS Keychain, Windows
//! Credential Manager, Linux Secret Service) instead of the webview's
//! localStorage. The backend reads it when it makes a request, so the key
//! never travels over IPC after it has been saved.

use crate::error::AppError;
use std::sync::{Mutex, OnceLock};

const SERVICE: &str = "itemis.ai-media-cutter";
const API_KEY_ACCOUNT: &str = "llm-api-key";

/// Where secrets are stored; the OS store in the app, memory in tests.
pub(crate) trait SecretStore: Send + Sync {
    fn get(&self) -> Result<Option<String>, String>;
    fn set(&self, value: &str) -> Result<(), String>;
    fn delete(&self) -> Result<(), String>;
}

struct KeyringStore;

impl KeyringStore {
    fn entry() -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, API_KEY_ACCOUNT)
            .map_err(|error| format!("The system credential store is unavailable: {error}"))
    }
}

impl SecretStore for KeyringStore {
    fn get(&self) -> Result<Option<String>, String> {
        match Self::entry()?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(format!(
                "Failed to read the API key from the keychain: {error}"
            )),
        }
    }

    fn set(&self, value: &str) -> Result<(), String> {
        Self::entry()?
            .set_password(value)
            .map_err(|error| format!("Failed to save the API key to the keychain: {error}"))
    }

    fn delete(&self) -> Result<(), String> {
        match Self::entry()?.delete_credential() {
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
}

impl ApiKey {
    pub(crate) fn new(store: Box<dyn SecretStore>) -> Self {
        Self {
            store,
            cached: Mutex::new(None),
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
            .ok_or_else(|| AppError::failed("No API key is configured. Add one in Settings."))
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

/// The app's API key, backed by the OS credential store.
pub(crate) fn api_key() -> &'static ApiKey {
    static API_KEY: OnceLock<ApiKey> = OnceLock::new();
    API_KEY.get_or_init(|| ApiKey::new(Box::new(KeyringStore)))
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
    fn set_trims_and_clear_removes() {
        let key = ApiKey::new(Box::<MemoryStore>::default());
        assert!(key.set("   ").is_err());
        key.set("  sk-456\n").unwrap();
        assert_eq!(key.require().unwrap(), "sk-456");
        key.clear().unwrap();
        assert!(!key.is_set().unwrap());
    }
}
