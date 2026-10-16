//! Пароли персонажей.
//!
//! * Windows и macOS — системное хранилище (Диспетчер учётных данных Windows, Связка ключей).
//!   Пароль не лежит в папке программы, и его не унесёт копия папки.
//! * Остальные системы (Linux, Android) — файл `secrets.toml` в папке данных, **без шифрования**.
//!   На Linux системного хранилища в WSL обычно нет, на Android папка приложения закрыта от других программ.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Имя «сервиса» в системном хранилище.
#[cfg(any(windows, target_os = "macos"))]
const SERVICE: &str = "klubok";

pub struct Secrets {
    file: PathBuf,
}

impl Secrets {
    pub fn new(data_dir: &Path) -> Secrets {
        Secrets {
            file: data_dir.join("secrets.toml"),
        }
    }

    /// Где хранятся пароли — чтобы честно показать это в интерфейсе.
    pub fn storage_description(&self) -> String {
        if cfg!(windows) {
            "Диспетчер учётных данных Windows".to_string()
        } else if cfg!(target_os = "macos") {
            "Связка ключей macOS".to_string()
        } else {
            format!("файл {} (без шифрования)", self.file.display())
        }
    }

    pub fn get(&self, profile_id: &str) -> anyhow::Result<Option<String>> {
        #[cfg(any(windows, target_os = "macos"))]
        {
            match keyring::Entry::new(SERVICE, profile_id)?.get_password() {
                Ok(p) => Ok(Some(p)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(e.into()),
            }
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            Ok(self.read_file()?.remove(profile_id))
        }
    }

    pub fn set(&self, profile_id: &str, password: &str) -> anyhow::Result<()> {
        #[cfg(any(windows, target_os = "macos"))]
        {
            keyring::Entry::new(SERVICE, profile_id)?.set_password(password)?;
            Ok(())
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let mut all = self.read_file()?;
            all.insert(profile_id.to_string(), password.to_string());
            self.write_file(&all)
        }
    }

    pub fn delete(&self, profile_id: &str) -> anyhow::Result<()> {
        #[cfg(any(windows, target_os = "macos"))]
        {
            match keyring::Entry::new(SERVICE, profile_id)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(e.into()),
            }
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let mut all = self.read_file()?;
            if all.remove(profile_id).is_some() {
                self.write_file(&all)?;
            }
            Ok(())
        }
    }

    #[allow(dead_code)] // нужен только там, где нет системного хранилища
    fn read_file(&self) -> anyhow::Result<BTreeMap<String, String>> {
        match std::fs::read_to_string(&self.file) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(e.into()),
        }
    }

    #[allow(dead_code)]
    fn write_file(&self, all: &BTreeMap<String, String>) -> anyhow::Result<()> {
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.file, toml::to_string(all)?)?;
        Ok(())
    }
}
