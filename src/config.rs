//! Settings file and password hashing.
//!
//! Settings live in `%APPDATA%\show-lock\config.toml`. The lock password is
//! never stored, only an Argon2 hash of it.

use std::path::PathBuf;

use argon2::password_hash::{
    rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::Argon2;
use serde::{Deserialize, Serialize};

/// Shortest password the settings window accepts.
pub const MIN_PASSWORD_LEN: usize = 4;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Argon2 hash (PHC string) of the lock password. `None` until one is set.
    pub password_hash: Option<String>,
    /// Offer Windows Hello (fingerprint, face or Windows PIN) on the lock screen.
    pub windows_hello: bool,
    /// Shortcut that locks the screen.
    pub hotkey: Hotkey,
    /// Seconds without input before the password prompt fades away again.
    pub prompt_timeout_secs: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            password_hash: None,
            windows_hello: true,
            hotkey: Hotkey::default(),
            prompt_timeout_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    /// An uppercase letter or a digit.
    pub key: char,
}

impl Default for Hotkey {
    fn default() -> Self {
        Self {
            ctrl: true,
            alt: true,
            shift: false,
            win: false,
            key: 'L',
        }
    }
}

impl Hotkey {
    /// Keys the settings window offers.
    pub const KEYS: &'static str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

    /// A shortcut needs at least one modifier and a valid key, and Win+L is
    /// reserved by Windows.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !Self::KEYS.contains(self.key) {
            return Err("Pick a letter or a number for the shortcut.");
        }
        if !(self.ctrl || self.alt || self.shift || self.win) {
            return Err("The shortcut needs at least one of Ctrl, Alt, Shift or Win.");
        }
        if self.win && self.key == 'L' {
            return Err("Win+L is reserved by Windows. Pick another shortcut.");
        }
        Ok(())
    }

    /// Human-readable, e.g. "Ctrl+Alt+L".
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.win {
            parts.push("Win".to_string());
        }
        parts.push(self.key.to_string());
        parts.join("+")
    }
}

impl Config {
    pub fn has_password(&self) -> bool {
        self.password_hash.is_some()
    }

    /// Hash and store a new password.
    pub fn set_password(&mut self, password: &str) -> Result<(), String> {
        if password.chars().count() < MIN_PASSWORD_LEN {
            return Err(format!("Use at least {MIN_PASSWORD_LEN} characters."));
        }
        self.password_hash = Some(hash_password(password)?);
        Ok(())
    }

    /// Settings file location. `SHOW_LOCK_CONFIG` overrides it (for testing).
    pub fn path() -> PathBuf {
        if let Some(p) = std::env::var_os("SHOW_LOCK_CONFIG") {
            return PathBuf::from(p);
        }
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("show-lock").join("config.toml")
    }

    /// Load settings; a missing or unreadable file gives the defaults.
    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Save settings (written to a temporary file first, then swapped in).
    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)
            .map_err(|e| format!("Could not write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("Could not save {}: {e}", path.display()))
    }
}

pub fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

/// True if `password` matches the stored hash. A malformed hash never matches.
pub fn verify_password(hash: &str, password: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_round_trip() {
        let mut c = Config::default();
        assert!(!c.has_password());
        c.set_password("correct horse").unwrap();
        let hash = c.password_hash.clone().unwrap();
        assert!(!hash.contains("correct horse"));
        assert!(verify_password(&hash, "correct horse"));
        assert!(!verify_password(&hash, "Correct horse"));
        assert!(!verify_password(&hash, ""));
    }

    #[test]
    fn short_password_rejected() {
        let mut c = Config::default();
        assert!(c.set_password("abc").is_err());
        assert!(c.password_hash.is_none());
    }

    #[test]
    fn bad_hash_never_matches() {
        assert!(!verify_password("not a hash", "anything"));
        assert!(!verify_password("", ""));
    }

    #[test]
    fn toml_round_trip_and_defaults() {
        let c = Config {
            hotkey: Hotkey {
                ctrl: false,
                alt: true,
                shift: true,
                win: false,
                key: 'K',
            },
            windows_hello: false,
            ..Config::default()
        };
        let text = toml::to_string_pretty(&c).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), c);
        // Missing fields fall back to defaults.
        let partial: Config = toml::from_str("windows_hello = false").unwrap();
        assert_eq!(partial.hotkey, Hotkey::default());
        assert_eq!(partial.prompt_timeout_secs, 30);
    }

    #[test]
    fn hotkey_rules() {
        assert!(Hotkey::default().validate().is_ok());
        assert_eq!(Hotkey::default().label(), "Ctrl+Alt+L");
        let win_l = Hotkey {
            ctrl: false,
            alt: false,
            shift: false,
            win: true,
            key: 'L',
        };
        assert!(win_l.validate().is_err());
        let bare = Hotkey {
            ctrl: false,
            alt: false,
            shift: false,
            win: false,
            key: 'L',
        };
        assert!(bare.validate().is_err());
        let bad_key = Hotkey {
            key: '!',
            ..Hotkey::default()
        };
        assert!(bad_key.validate().is_err());
    }
}
