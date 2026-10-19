//! Окно «Клубка»: тонкая прослойка между ядром (`klubok-core`) и интерфейсом.
//!
//! Интерфейс вызывает команды (профили, `connect`, `send`, `disconnect`), а получает
//! события `session` — пачки строк, собранные примерно за 16 мс.

mod secrets;
mod ui;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use klubok_core::{
    Automation, Credentials, Mapper, Profile, ProfileStore, Session, SessionConfig, SessionInput,
    SessionOutput, decode_settings_file,
};
use klubok_map::{MapStore, SqliteStore};
use klubok_proto::Charset;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::mpsc;
use tokio::time::Instant;

use secrets::Secrets;
use ui::UiEvent;

/// Как долго копить строки перед отправкой в интерфейс.
const BATCH_WINDOW: Duration = Duration::from_millis(16);
/// Изменённые настройки (переменные меняются часто) пишутся на диск не чаще, чем раз в столько.
const SAVE_DELAY: Duration = Duration::from_secs(2);

/// Состояние приложения.
///
/// Это прослойка UI, а не домен, поэтому здесь `Mutex` допустим: Tauri вызывает
/// команды из разных потоков.
struct AppState {
    data_dir: PathBuf,
    store: ProfileStore,
    secrets: Secrets,
    /// Канал ввода текущего подключения и id его профиля.
    connection: Mutex<Option<(String, mpsc::Sender<SessionInput>)>>,
    /// Последний текст настроек подключённого профиля: на диск он попадает с задержкой,
    /// а редактору нужен свежий.
    latest_automation: Arc<Mutex<Option<String>>>,
}

impl AppState {
    fn connected_profile(&self) -> Option<String> {
        self.connection
            .lock()
            .ok()?
            .as_ref()
            .filter(|(_, tx)| !tx.is_closed())
            .map(|(id, _)| id.clone())
    }
}

/// Папка данных: рядом с программой (портативный режим), если туда можно писать,
/// иначе — стандартная папка приложения.
fn find_data_dir(app: &AppHandle) -> PathBuf {
    let portable = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("klubok-data")))
        .filter(|dir| is_writable(dir));
    portable.unwrap_or_else(|| {
        app.path()
            .app_data_dir()
            .unwrap_or_else(|_| PathBuf::from("klubok-data"))
    })
}

fn is_writable(dir: &Path) -> bool {
    std::fs::create_dir_all(dir).is_ok() && {
        let probe = dir.join(".write-test");
        let ok = std::fs::write(&probe, b"").is_ok();
        let _ = std::fs::remove_file(probe);
        ok
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Файл карты профиля: `<данные>/maps/<имя>.sqlite`.
fn map_path(data_dir: &Path, profile: &Profile) -> PathBuf {
    let name: String = profile
        .map
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
    let name = if name.is_empty() {
        "bylins".to_string()
    } else {
        name
    };
    data_dir.join("maps").join(format!("{name}.sqlite"))
}

fn open_map(data_dir: &Path, profile: &Profile) -> Result<SqliteStore, String> {
    let path = map_path(data_dir, profile);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    SqliteStore::open(&path).map_err(|e| format!("карта {}: {e}", path.display()))
}

// ───────────────────────────── профили ─────────────────────────────

#[derive(Serialize)]
struct ProfileInfo {
    id: String,
    profile: Profile,
    has_password: bool,
}

#[derive(Serialize)]
struct ProfileList {
    profiles: Vec<ProfileInfo>,
    data_dir: String,
    password_storage: String,
}

#[tauri::command]
fn list_profiles(state: State<'_, AppState>) -> Result<ProfileList, String> {
    let profiles = state
        .store
        .list()
        .map_err(err)?
        .into_iter()
        .map(|(id, profile)| ProfileInfo {
            has_password: matches!(state.secrets.get(&id), Ok(Some(_))),
            id,
            profile,
        })
        .collect();
    Ok(ProfileList {
        profiles,
        data_dir: state.data_dir.display().to_string(),
        password_storage: state.secrets.storage_description(),
    })
}

/// Сохраняет профиль. `password`: `None` — не менять, пустая строка — забыть пароль.
#[tauri::command]
fn save_profile(
    state: State<'_, AppState>,
    id: Option<String>,
    profile: Profile,
    password: Option<String>,
) -> Result<String, String> {
    if profile.name.trim().is_empty() {
        return Err("Укажите название профиля.".into());
    }
    profile.charset.parse::<Charset>()?;
    let id = state.store.save(id.as_deref(), &profile).map_err(err)?;
    match password.as_deref() {
        None => {}
        Some("") => state.secrets.delete(&id).map_err(err)?,
        Some(p) => state
            .secrets
            .set(&id, p)
            .map_err(|e| format!("Пароль не сохранён: {e}"))?,
    }
    Ok(id)
}

#[tauri::command]
fn delete_profile(state: State<'_, AppState>, id: String) -> Result<(), String> {
    if state.connected_profile().as_deref() == Some(id.as_str()) {
        return Err("Сначала отключитесь.".into());
    }
    state.secrets.delete(&id).map_err(err)?;
    state.store.delete(&id).map_err(err)
}

#[derive(Serialize)]
struct ImportResult {
    aliases: usize,
    actions: usize,
    variables: usize,
    highlights: usize,
    hotkeys: usize,
    substitutions: usize,
    login_actions: Vec<String>,
    unsupported: Vec<(String, usize)>,
}

/// Добавляет в профиль настройки из файла `.set` JMC (байты файла — кодировка определяется сама).
#[tauri::command]
fn import_set(
    state: State<'_, AppState>,
    id: String,
    bytes: Vec<u8>,
) -> Result<ImportResult, String> {
    if state.connected_profile().as_deref() == Some(id.as_str()) {
        return Err("Импорт — только при отключённом профиле.".into());
    }
    let mut automation = Automation::new();
    automation.import(&state.store.load_automation(&id).map_err(err)?);
    let report = automation.import(&decode_settings_file(&bytes));
    state
        .store
        .save_automation(&id, &automation.to_script())
        .map_err(err)?;
    Ok(ImportResult {
        aliases: report.aliases,
        actions: report.actions,
        variables: report.variables,
        highlights: report.highlights,
        hotkeys: report.hotkeys,
        substitutions: report.substitutions,
        login_actions: report.login_actions,
        unsupported: report.unsupported.into_iter().collect(),
    })
}

#[derive(Serialize)]
struct MapImportResult {
    files: usize,
    rooms_total: usize,
    /// Сколько комнат добавлено (уже известные карте не перезаписываются).
    rooms_added: usize,
    synthetic: usize,
    duplicates: usize,
    unresolved_exits: usize,
    errors: Vec<String>,
}

/// Добавляет в карту профиля карты MudMapper: игрок выбирает папку с файлами `.mm2`.
/// `None` — выбор отменён.
#[tauri::command]
async fn import_mm2(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<MapImportResult>, String> {
    if state.connected_profile().is_some() {
        return Err("Импорт карты — только при отключённом клиенте.".into());
    }
    let Some(folder) = app
        .dialog()
        .file()
        .set_title("Папка с картами MudMapper (.mm2)")
        .blocking_pick_folder()
    else {
        return Ok(None);
    };
    let folder = folder.into_path().map_err(err)?;
    let profile = state.store.load(&id).map_err(err)?;
    let data_dir = state.data_dir.clone();

    // Чтение и запись карты — долгие, уводим с потока команд.
    tauri::async_runtime::spawn_blocking(move || {
        let mut zones = Vec::new();
        let mut errors = Vec::new();
        let entries = std::fs::read_dir(&folder).map_err(err)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("mm2"))
            {
                continue;
            }
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            match std::fs::read(&path)
                .map_err(err)
                .and_then(|b| klubok_mm2::load_mm2(&b).map_err(err))
            {
                Ok(zone) => zones.push((name, zone)),
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
        if zones.is_empty() {
            return Err(format!("в папке {} нет карт .mm2", folder.display()));
        }
        let (imported, stats) = klubok_mm2::convert::to_map(&zones);

        let mut store = open_map(&data_dir, &profile)?;
        let mut map = store.load().map_err(err)?;
        let before = map.len();
        for room in imported.rooms() {
            if map.room(room.vnum).is_none() {
                map.insert_room(room.clone());
            }
        }
        let rooms_added = map.len() - before;
        store.save_map(&map).map_err(err)?;
        Ok(Some(MapImportResult {
            files: zones.len(),
            rooms_total: stats.rooms,
            rooms_added,
            synthetic: stats.synthetic,
            duplicates: stats.duplicates,
            unresolved_exits: stats.unresolved_exits,
            errors,
        }))
    })
    .await
    .map_err(err)?
}

/// Настройки автоматизации профиля текстом JMC (для редактора).
#[tauri::command]
fn get_automation(state: State<'_, AppState>, id: String) -> Result<String, String> {
    if state.connected_profile().as_deref() == Some(id.as_str())
        && let Some(text) = state.latest_automation.lock().map_err(err)?.clone()
    {
        return Ok(text);
    }
    state.store.load_automation(&id).map_err(err)
}

/// Сохраняет настройки из редактора. Если профиль подключён — сессия сразу их применит.
#[tauri::command]
async fn set_automation(
    state: State<'_, AppState>,
    id: String,
    text: String,
) -> Result<(), String> {
    let sender = state
        .connection
        .lock()
        .map_err(err)?
        .as_ref()
        .filter(|(connected, tx)| *connected == id && !tx.is_closed())
        .map(|(_, tx)| tx.clone());
    match sender {
        // Сессия пришлёт AutomationChanged, и текст сохранится как обычно.
        Some(tx) => tx
            .send(SessionInput::ReplaceAutomation(text))
            .await
            .map_err(|_| "нет подключения".to_string()),
        None => state.store.save_automation(&id, &text).map_err(err),
    }
}

// ───────────────────────────── подключение ─────────────────────────────

#[tauri::command]
fn connect(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    let profile = state.store.load(&id).map_err(err)?;
    let charset: Charset = profile.charset.parse()?;

    let mut automation = Automation::new();
    automation.import(&state.store.load_automation(&id).map_err(err)?);

    let password = state.secrets.get(&id).map_err(err)?;
    let login = match (profile.character.trim(), password) {
        (name, Some(password)) if !name.is_empty() => Some(Credentials {
            name: name.to_string(),
            password,
        }),
        _ => None,
    };

    let config = SessionConfig {
        charset,
        blank_lines: profile.blank_lines,
        login,
        log_dir: Some(state.data_dir.join("logs")),
        ..SessionConfig::default()
    };

    let mut store = open_map(&state.data_dir, &profile)?;
    let map = store.load().map_err(err)?;
    let session =
        Session::new(config, automation).with_mapper(Mapper::new(map, Some(Box::new(store))));

    let (input_tx, input_rx) = mpsc::channel::<SessionInput>(64);
    let (event_tx, event_rx) = mpsc::channel::<SessionOutput>(4096);
    // Старое подключение закроется само: его канал ввода выбрасывается здесь.
    *state.connection.lock().map_err(err)? = Some((id.clone(), input_tx));

    let address = format!("{}:{}", profile.host, profile.port);
    let app_for_session = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = klubok_core::run_session(&address, session, input_rx, event_tx, None).await;
        if let Err(error) = result {
            let _ = app_for_session.emit(
                "session",
                vec![UiEvent::ConnectionLost {
                    text: format!("{address}: {error}"),
                }],
            );
        }
    });
    let store = state.store.clone();
    if let Ok(mut latest) = state.latest_automation.lock() {
        *latest = None;
    }
    let latest = state.latest_automation.clone();
    tauri::async_runtime::spawn(forward_events(app, event_rx, store, id, latest));
    Ok(())
}

/// Ввод игрока. `raw` — отправить как есть, без алиасов и `#`-команд (пароль).
#[tauri::command]
async fn send(state: State<'_, AppState>, text: String, raw: bool) -> Result<(), String> {
    let sender = state
        .connection
        .lock()
        .map_err(err)?
        .as_ref()
        .map(|(_, tx)| tx.clone());
    let input = if raw {
        SessionInput::Raw(text)
    } else {
        SessionInput::UserCommand(text)
    };
    match sender {
        Some(sender) => sender
            .send(input)
            .await
            .map_err(|_| "нет подключения".to_string()),
        None => Err("нет подключения".to_string()),
    }
}

/// Правка карты мышью (сдвиг, удаление комнат) — только при подключении:
/// карта живёт в сессии.
#[tauri::command]
async fn map_edit(state: State<'_, AppState>, edit: klubok_core::MapEdit) -> Result<(), String> {
    let sender = state
        .connection
        .lock()
        .map_err(err)?
        .as_ref()
        .map(|(_, tx)| tx.clone());
    match sender {
        Some(tx) => tx
            .send(SessionInput::MapEdit(edit))
            .await
            .map_err(|_| "нет подключения".to_string()),
        None => Err("Править карту можно при подключении.".to_string()),
    }
}

#[tauri::command]
fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    // Закрытие канала ввода завершает сетевой цикл.
    state.connection.lock().map_err(err)?.take();
    Ok(())
}

/// Пересылает события сессии в интерфейс пачками и сохраняет изменённые настройки.
async fn forward_events(
    app: AppHandle,
    mut events: mpsc::Receiver<SessionOutput>,
    store: ProfileStore,
    profile_id: String,
    latest: Arc<Mutex<Option<String>>>,
) {
    // Последний несохранённый текст настроек.
    let mut unsaved: Option<String> = None;
    let mut last_save = Instant::now();
    let save = |text: String| {
        if let Err(e) = store.save_automation(&profile_id, &text) {
            let _ = app.emit(
                "session",
                vec![UiEvent::Warning {
                    text: format!("Настройки не сохранены: {e}"),
                }],
            );
        }
    };

    let mut closed = false;
    while !closed {
        // Если есть несохранённое — ждём не дольше SAVE_DELAY, потом сохраняем.
        let first = if unsaved.is_some() {
            match tokio::time::timeout(SAVE_DELAY, events.recv()).await {
                Ok(event) => event,
                Err(_) => {
                    if let Some(text) = unsaved.take() {
                        save(text);
                        last_save = Instant::now();
                    }
                    continue;
                }
            }
        } else {
            events.recv().await
        };
        let Some(first) = first else { break };

        let mut batch: Vec<UiEvent> = Vec::new();
        let mut take = |event: SessionOutput, batch: &mut Vec<UiEvent>| match event {
            SessionOutput::AutomationChanged(text) => {
                if let Ok(mut latest) = latest.lock() {
                    *latest = Some(text.clone());
                }
                unsaved = Some(text);
            }
            other => batch.extend(UiEvent::from_output(other)),
        };
        take(first, &mut batch);

        let deadline = Instant::now() + BATCH_WINDOW;
        loop {
            match tokio::time::timeout_at(deadline, events.recv()).await {
                Ok(Some(event)) => take(event, &mut batch),
                Ok(None) => {
                    closed = true;
                    break;
                }
                Err(_) => break, // окно пачки истекло
            }
        }

        if batch.iter().any(|e| matches!(e, UiEvent::Disconnected)) {
            closed = true;
        } else if closed {
            batch.push(UiEvent::Disconnected);
        }
        if !batch.is_empty() {
            let _ = app.emit("session", batch);
        }
        if unsaved.is_some() && last_save.elapsed() >= SAVE_DELAY {
            save(unsaved.take().unwrap_or_default());
            last_save = Instant::now();
        }
    }
    if let Some(text) = unsaved {
        save(text);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = find_data_dir(app.handle());
            app.manage(AppState {
                store: ProfileStore::new(&data_dir),
                secrets: Secrets::new(&data_dir),
                data_dir,
                connection: Mutex::new(None),
                latest_automation: Arc::new(Mutex::new(None)),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_profiles,
            save_profile,
            delete_profile,
            import_set,
            import_mm2,
            get_automation,
            set_automation,
            connect,
            send,
            map_edit,
            disconnect
        ])
        .run(tauri::generate_context!())
        .expect("не удалось запустить окно Клубка");
}
