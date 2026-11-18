use std::{
    fs,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::oauth::{Grant, PendingRefresh, Registration};

const STORAGE_ERROR: &str = "Secure ChatGPT session storage is unavailable. Check your system credential store and local app data.";
const SERVICE: &str = "com.meetily-sapphire.chatgpt";

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct SavedSession {
    pub host_id: String,
    pub registration: Option<Registration>,
    #[serde(default)]
    pub registrations: Vec<Registration>,
    pub pending_client_id: Option<String>,
    pub grant: Option<Grant>,
    pub pending_refresh: Option<PendingRefresh>,
    pub auto_summary: bool,
    #[serde(default)]
    pub disconnect_pending: bool,
}

impl Default for SavedSession {
    fn default() -> Self {
        Self {
            host_id: format!("urn:uuid:{}", uuid::Uuid::new_v4()),
            registration: None,
            registrations: Vec::new(),
            pending_client_id: None,
            grant: None,
            pending_refresh: None,
            auto_summary: false,
            disconnect_pending: false,
        }
    }
}

#[derive(Clone)]
pub(super) struct Storage {
    path: PathBuf,
    account: String,
    key: Arc<Mutex<Option<[u8; 32]>>>,
}

impl Storage {
    #[cfg(test)]
    pub(super) fn for_test(path: PathBuf) -> Self {
        Self {
            path,
            account: String::new(),
            key: Arc::new(Mutex::new(Some([42; 32]))),
        }
    }

    pub fn open(dir: PathBuf) -> Result<(Self, SavedSession), String> {
        let path = dir.join("chatgpt-session.enc");
        let account = format!(
            "session-key-{:x}",
            Sha256::digest(dir.to_string_lossy().as_bytes())
        );
        let storage = Self {
            path,
            account,
            key: Arc::new(Mutex::new(None)),
        };
        let saved = if storage
            .path
            .try_exists()
            .map_err(|_| STORAGE_ERROR.to_string())?
        {
            storage.read()?
        } else {
            SavedSession::default()
        };
        Ok((storage, saved))
    }

    fn key(&self, create: bool) -> Result<[u8; 32], String> {
        let mut cached = self.key.lock().map_err(|_| STORAGE_ERROR.to_string())?;
        if let Some(key) = *cached {
            return Ok(key);
        }
        let entry =
            keyring::Entry::new(SERVICE, &self.account).map_err(|_| STORAGE_ERROR.to_string())?;
        let key = match entry.get_password() {
            Ok(encoded) => {
                let bytes = STANDARD
                    .decode(encoded)
                    .map_err(|_| STORAGE_ERROR.to_string())?;
                bytes.try_into().map_err(|_| STORAGE_ERROR.to_string())?
            }
            Err(keyring::Error::NoEntry)
                if create
                    && !self
                        .path
                        .try_exists()
                        .map_err(|_| STORAGE_ERROR.to_string())? =>
            {
                let mut key = [0u8; 32];
                OsRng.fill_bytes(&mut key);
                entry
                    .set_password(&STANDARD.encode(key))
                    .map_err(|_| STORAGE_ERROR.to_string())?;
                key
            }
            Err(_) => return Err(STORAGE_ERROR.to_string()),
        };
        *cached = Some(key);
        Ok(key)
    }

    pub(super) fn read(&self) -> Result<SavedSession, String> {
        let data = fs::read(&self.path).map_err(|_| STORAGE_ERROR.to_string())?;
        if data.len() < 12 {
            return Err(STORAGE_ERROR.to_string());
        }
        let cipher =
            Aes256Gcm::new_from_slice(&self.key(false)?).map_err(|_| STORAGE_ERROR.to_string())?;
        let plain = cipher
            .decrypt(Nonce::from_slice(&data[..12]), &data[12..])
            .map_err(|_| STORAGE_ERROR.to_string())?;
        serde_json::from_slice(&plain).map_err(|_| STORAGE_ERROR.to_string())
    }

    pub fn write(&self, saved: &SavedSession) -> Result<(), String> {
        fs::create_dir_all(
            self.path
                .parent()
                .ok_or_else(|| STORAGE_ERROR.to_string())?,
        )
        .map_err(|_| STORAGE_ERROR.to_string())?;
        let plain = serde_json::to_vec(saved).map_err(|_| STORAGE_ERROR.to_string())?;
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let cipher =
            Aes256Gcm::new_from_slice(&self.key(true)?).map_err(|_| STORAGE_ERROR.to_string())?;
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), plain.as_ref())
            .map_err(|_| STORAGE_ERROR.to_string())?;
        let mut file = tempfile::NamedTempFile::new_in(
            self.path
                .parent()
                .ok_or_else(|| STORAGE_ERROR.to_string())?,
        )
        .map_err(|_| STORAGE_ERROR.to_string())?;
        file.write_all(&nonce)
            .and_then(|_| file.write_all(&ciphertext))
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| STORAGE_ERROR.to_string())?;
        file.persist(&self.path)
            .map_err(|_| STORAGE_ERROR.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_state_is_authenticated_and_replaced_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::for_test(dir.path().join("session"));
        let mut saved = SavedSession::default();
        let host_id = saved.host_id.clone();
        storage.write(&saved).unwrap();
        saved.auto_summary = true;
        saved.pending_refresh = Some(PendingRefresh {
            access_token: "replacement-access".into(),
            refresh_token: "replacement-refresh".into(),
            id_token: None,
            expires_at: 123,
            earliest_refresh_at: None,
            scopes: vec![],
            received_at: 100,
        });
        storage.write(&saved).unwrap();
        let restored = storage.read().unwrap();
        assert_eq!(restored.host_id, host_id);
        assert!(restored.auto_summary);
        assert_eq!(
            restored.pending_refresh.unwrap().refresh_token,
            "replacement-refresh"
        );
        let mut bytes = fs::read(&storage.path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(&host_id));
        assert!(!String::from_utf8_lossy(&bytes).contains("replacement-refresh"));
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(&storage.path, bytes).unwrap();
        assert!(storage.read().is_err());
    }

    #[test]
    fn fresh_profiles_do_not_touch_credentials_or_create_files() {
        let dir = tempfile::tempdir().unwrap();
        let first_path = dir.path().join("first");
        let second_path = dir.path().join("second");
        let (first, first_state) = Storage::open(first_path.clone()).unwrap();
        let (second, _) = Storage::open(second_path.clone()).unwrap();
        assert_ne!(first.account, second.account);
        assert!(first_state.grant.is_none());
        assert!(!first_path.exists());
        assert!(!second_path.exists());
    }
}
