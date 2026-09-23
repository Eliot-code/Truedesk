use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::unattended;

const DEFAULT_RELAY_URL: &str = "ws://127.0.0.1:8080/ws";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub id: String,
    pub relay_url: String,
    pub allow_list: Vec<String>,
    pub recent_ids: Vec<String>,
    #[serde(default)]
    pub unattended_password_set: bool,
    // Read only for migration. This field is never serialized back to disk.
    #[serde(default, rename = "unattended_password", skip_serializing)]
    legacy_unattended_password: Option<String>,
    #[serde(default)]
    pub unattended_enabled: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            id: generate_id(),
            relay_url: DEFAULT_RELAY_URL.to_string(),
            allow_list: Vec::new(),
            recent_ids: Vec::new(),
            unattended_password_set: false,
            legacy_unattended_password: None,
            unattended_enabled: false,
        }
    }
}

impl Config {
    fn take_legacy_unattended_password(&mut self) -> Option<String> {
        self.legacy_unattended_password.take()
    }
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
    pub config: Config,
}

impl ConfigStore {
    pub fn load(app_data_dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = app_data_dir.as_ref();
        fs::create_dir_all(dir)?;
        let path = dir.join("config.json");
        let mut config = match fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Config::default(),
            Err(error) => return Err(error),
        };
        if let Some(password) = config.take_legacy_unattended_password() {
            // Migration is best effort. The plaintext is removed from the
            // in-memory config regardless, so save() cannot preserve it.
            config.unattended_password_set = unattended::set_password(&password).is_ok();
        }
        let store = Self { path, config };
        store.save()?;
        Ok(store)
    }

    pub fn save(&self) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.config).map_err(io::Error::other)?;
        fs::write(&self.path, bytes)
    }

    pub fn ensure_identity(&mut self) -> String {
        if self.config.id.len() != 9 || !self.config.id.bytes().all(|b| b.is_ascii_digit()) {
            self.config.id = generate_id();
            let _ = self.save();
        }
        self.config.id.clone()
    }

    pub fn add_allow(&mut self, id: &str) -> io::Result<()> {
        add_unique(&mut self.config.allow_list, id);
        self.save()
    }

    // TODO(Task 6): call from start_hosting to auto-accept allow-listed peers
    // once the relay's Paired frame carries the requester ID.
    #[allow(dead_code)]
    pub fn is_allowed(&self, id: &str) -> bool {
        self.config.allow_list.iter().any(|allowed| allowed == id)
    }

    pub fn add_recent(&mut self, id: &str) -> io::Result<()> {
        add_recent(&mut self.config.recent_ids, id);
        self.save()
    }
}

pub fn generate_id() -> String {
    use rand::Rng;
    rand::thread_rng()
        .gen_range(100_000_000..=999_999_999)
        .to_string()
}

fn add_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn add_recent(values: &mut Vec<String>, value: &str) {
    values.retain(|existing| existing != value);
    values.insert(0, value.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_id_is_nine_digits() {
        let id = generate_id();
        assert_eq!(id.len(), 9);
        assert!(id.bytes().all(|byte| byte.is_ascii_digit()));
    }

    #[test]
    fn allow_list_adds_and_contains_once() {
        let mut values = Vec::new();
        add_unique(&mut values, "123456789");
        add_unique(&mut values, "123456789");
        assert_eq!(values, vec!["123456789"]);
        assert!(values.iter().any(|id| id == "123456789"));
    }

    #[test]
    fn recent_ids_are_deduplicated_and_most_recent_first() {
        let mut values = Vec::new();
        add_recent(&mut values, "111111111");
        add_recent(&mut values, "222222222");
        add_recent(&mut values, "111111111");
        assert_eq!(values, vec!["111111111", "222222222"]);
    }

    #[test]
    fn legacy_unattended_password_is_migrated_to_a_marker() {
        let mut config = Config {
            legacy_unattended_password: Some("fixed-secret".into()),
            unattended_enabled: true,
            ..Config::default()
        };
        let password = config.take_legacy_unattended_password();
        config.unattended_password_set = password.is_some();
        let encoded = serde_json::to_string(&config).unwrap();
        assert_eq!(password.as_deref(), Some("fixed-secret"));
        assert!(config.unattended_password_set);
        assert!(!encoded.contains("fixed-secret"));
        assert!(!encoded.contains("\"unattended_password\""));
    }

    #[test]
    fn unattended_access_marker_round_trips_without_a_secret() {
        let config = Config {
            unattended_password_set: true,
            unattended_enabled: true,
            ..Config::default()
        };
        let encoded = serde_json::to_string(&config).unwrap();
        let decoded: Config = serde_json::from_str(&encoded).unwrap();
        assert!(decoded.unattended_password_set);
        assert!(decoded.unattended_enabled);
    }
}
