//! Профили персонажей: куда подключаться, кем входить, что показывать.
//!
//! Каждый профиль — папка `<данные>/profiles/<id>/`:
//! * `profile.toml` — подключение, имя персонажа, настройки пустых строк;
//! * `automation.set` — алиасы, триггеры, переменные и т.д. в формате JMC (UTF-8).
//!
//! Пароль здесь **не хранится** — его кладёт в системное хранилище приложение.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use klubok_automation::BlankLineSettings;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    /// Название профиля (обычно имя персонажа).
    pub name: String,
    pub host: String,
    pub port: u16,
    /// `utf8`, `koi8r` или `cp1251`.
    pub charset: String,
    /// Имя персонажа для автовхода (пусто — без автовхода).
    pub character: String,
    pub blank_lines: BlankLineSettings,
    /// Имя карты: файл `<данные>/maps/<map>.sqlite`. Профили одного мира делят карту.
    pub map: String,
    /// Переподключаться, если связь оборвалась не по воле игрока.
    pub reconnect: bool,
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            name: String::new(),
            host: "bylins.su".to_string(),
            port: 4000,
            charset: "utf8".to_string(),
            character: String::new(),
            blank_lines: BlankLineSettings::default(),
            map: "bylins".to_string(),
            reconnect: true,
        }
    }
}

const PROFILE_FILE: &str = "profile.toml";
const AUTOMATION_FILE: &str = "automation.set";

/// Папка с профилями.
#[derive(Debug, Clone)]
pub struct ProfileStore {
    root: PathBuf,
}

impl ProfileStore {
    /// `data_dir` — корень данных приложения; профили лежат в `data_dir/profiles`.
    pub fn new(data_dir: &Path) -> ProfileStore {
        ProfileStore {
            root: data_dir.join("profiles"),
        }
    }

    fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    /// Все профили: (id, профиль), по алфавиту.
    pub fn list(&self) -> anyhow::Result<Vec<(String, Profile)>> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Ok(out); // папки ещё нет — профилей нет
        };
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().to_string();
            if entry.path().join(PROFILE_FILE).is_file() {
                out.push((id.clone(), self.load(&id)?));
            }
        }
        out.sort_by_key(|a| a.1.name.to_lowercase());
        Ok(out)
    }

    pub fn load(&self, id: &str) -> anyhow::Result<Profile> {
        let path = self.dir(id).join(PROFILE_FILE);
        let text =
            fs::read_to_string(&path).with_context(|| format!("чтение {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("разбор {}", path.display()))
    }

    /// Сохраняет профиль. Если `id` не задан — создаёт новый по имени профиля. Возвращает id.
    pub fn save(&self, id: Option<&str>, profile: &Profile) -> anyhow::Result<String> {
        let id = match id {
            Some(id) => id.to_string(),
            None => self.free_id(&profile.name),
        };
        let dir = self.dir(&id);
        fs::create_dir_all(&dir).with_context(|| format!("создание {}", dir.display()))?;
        write_atomic(&dir.join(PROFILE_FILE), &toml::to_string_pretty(profile)?)?;
        Ok(id)
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        let dir = self.dir(id);
        if dir.is_dir() {
            fs::remove_dir_all(&dir).with_context(|| format!("удаление {}", dir.display()))?;
        }
        Ok(())
    }

    /// Текст настроек автоматизации (пусто, если файла нет).
    pub fn load_automation(&self, id: &str) -> anyhow::Result<String> {
        let path = self.dir(id).join(AUTOMATION_FILE);
        match fs::read_to_string(&path) {
            Ok(text) => Ok(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(e).with_context(|| format!("чтение {}", path.display())),
        }
    }

    pub fn save_automation(&self, id: &str, script: &str) -> anyhow::Result<()> {
        let dir = self.dir(id);
        fs::create_dir_all(&dir)?;
        write_atomic(&dir.join(AUTOMATION_FILE), script)
    }

    /// Имя папки из названия профиля: без символов, запрещённых в Windows, и не занятое.
    fn free_id(&self, name: &str) -> String {
        let base: String = name
            .trim()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let base = if base.trim_matches('_').is_empty() {
            "профиль".to_string()
        } else {
            base
        };
        let mut id = base.clone();
        let mut n = 2;
        while self.dir(&id).exists() {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }
}

/// Пишет файл через временный, чтобы при сбое не остался обрезанный файл.
fn write_atomic(path: &Path, text: &str) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text).with_context(|| format!("запись {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("запись {}", path.display()))?;
    Ok(())
}

/// Текст файла настроек: UTF-8, а если это не UTF-8 — Windows-1251 (старые `.set` JMC).
pub fn decode_settings_file(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes); // BOM
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => klubok_proto::Charset::Cp1251.decode(bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> (ProfileStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("klubok-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        (ProfileStore::new(&dir), dir)
    }

    #[test]
    fn сохранить_загрузить_удалить() {
        let (store, dir) = temp_store("profiles");
        let profile = Profile {
            name: "Колдун".into(),
            character: "Колдун".into(),
            ..Profile::default()
        };
        let id = store.save(None, &profile).unwrap();
        assert_eq!(id, "Колдун");
        // Второй профиль с тем же названием получает другую папку.
        assert_eq!(store.save(None, &profile).unwrap(), "Колдун-2");

        assert_eq!(store.load(&id).unwrap(), profile);
        assert_eq!(store.list().unwrap().len(), 2);

        store.save_automation(&id, "#alias {а} {б}\n").unwrap();
        assert_eq!(store.load_automation(&id).unwrap(), "#alias {а} {б}\n");
        assert_eq!(store.load_automation("Колдун-2").unwrap(), "");

        store.delete(&id).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn недопустимые_символы_в_имени() {
        let (store, dir) = temp_store("ids");
        let id = store
            .save(
                None,
                &Profile {
                    name: "a/b:c?".into(),
                    ..Profile::default()
                },
            )
            .unwrap();
        assert_eq!(id, "a_b_c_");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn кодировка_файла_настроек() {
        assert_eq!(decode_settings_file("#alias {а}".as_bytes()), "#alias {а}");
        assert_eq!(decode_settings_file(b"#alias {\xe0}"), "#alias {а}");
        assert_eq!(decode_settings_file(b"\xef\xbb\xbf#var"), "#var");
    }
}
