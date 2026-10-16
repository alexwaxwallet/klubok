//! Окно «Клубка»: тонкая прослойка между ядром (`klubok-core`) и интерфейсом.
//!
//! Интерфейс вызывает команды (`connect`, `send`, `disconnect`), а получает
//! события `session` — пачки строк, собранные примерно за 16 мс.

mod ui;

use std::sync::Mutex;
use std::time::Duration;

use klubok_core::{SessionConfig, SessionOutput};
use klubok_proto::Charset;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;
use tokio::time::Instant;

use ui::UiEvent;

/// Как долго копить строки перед отправкой в интерфейс.
const BATCH_WINDOW: Duration = Duration::from_millis(16);

/// Состояние приложения: канал команд текущего подключения (если оно есть).
///
/// Это прослойка UI, а не домен, поэтому здесь `Mutex` допустим: Tauri вызывает
/// команды из разных потоков.
#[derive(Default)]
struct AppState {
    commands: Mutex<Option<mpsc::Sender<String>>>,
}

#[tauri::command]
fn connect(
    app: AppHandle,
    state: State<'_, AppState>,
    host: String,
    port: u16,
    charset: String,
) -> Result<(), String> {
    let charset: Charset = charset.parse()?;
    let config = SessionConfig {
        charset,
        ..SessionConfig::default()
    };

    let (command_tx, command_rx) = mpsc::channel::<String>(64);
    let (event_tx, event_rx) = mpsc::channel::<SessionOutput>(4096);
    // Старое подключение закроется само: его канал команд выбрасывается здесь.
    *state.commands.lock().map_err(|e| e.to_string())? = Some(command_tx);

    let address = format!("{host}:{port}");
    let app_for_session = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = klubok_core::run_session(&address, config, command_rx, event_tx, None).await;
        if let Err(error) = result {
            let _ = app_for_session.emit(
                "session",
                vec![UiEvent::Error {
                    text: format!("{address}: {error}"),
                }],
            );
        }
    });
    tauri::async_runtime::spawn(forward_events(app, event_rx));
    Ok(())
}

#[tauri::command]
async fn send(state: State<'_, AppState>, text: String) -> Result<(), String> {
    let sender = state.commands.lock().map_err(|e| e.to_string())?.clone();
    match sender {
        Some(sender) => sender
            .send(text)
            .await
            .map_err(|_| "нет подключения".to_string()),
        None => Err("нет подключения".to_string()),
    }
}

#[tauri::command]
fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    // Закрытие канала команд завершает сетевой цикл.
    state.commands.lock().map_err(|e| e.to_string())?.take();
    Ok(())
}

/// Пересылает события сессии в интерфейс пачками, чтобы спам в бою не подвесил окно.
async fn forward_events(app: AppHandle, mut events: mpsc::Receiver<SessionOutput>) {
    let mut connection_closed = false;
    while let Some(first) = events.recv().await {
        let mut batch: Vec<UiEvent> = Vec::new();
        batch.extend(UiEvent::from_output(first));

        let deadline = Instant::now() + BATCH_WINDOW;
        loop {
            match tokio::time::timeout_at(deadline, events.recv()).await {
                Ok(Some(event)) => batch.extend(UiEvent::from_output(event)),
                Ok(None) => {
                    connection_closed = true;
                    break;
                }
                Err(_) => break, // окно пачки истекло
            }
        }

        if batch.iter().any(|e| matches!(e, UiEvent::Disconnected)) {
            connection_closed = true;
        }
        let _ = app.emit("session", batch);
        if connection_closed {
            break;
        }
    }
    if !connection_closed {
        let _ = app.emit("session", vec![UiEvent::Disconnected]);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![connect, send, disconnect])
        .run(tauri::generate_context!())
        .expect("не удалось запустить окно Клубка");
}
