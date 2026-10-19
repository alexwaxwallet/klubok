//! Сетевой цикл: соединяет `Session` с сокетом, вводом игрока и таймерами.

use std::io::Write;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::Instant;

use klubok_automation::Automation;

use crate::session::{LogOp, Session, SessionConfig, SessionInput, SessionOutput};

/// Сколько ждать продолжения незаконченной строки, прежде чем показать её как промпт.
const PARTIAL_LINE_TIMEOUT: Duration = Duration::from_millis(150);

/// Подключается к серверу и крутит сессию, пока не закроется соединение
/// или не закроется канал команд.
///
/// * `session` — сессия с настройками, автоматизацией и картой профиля;
/// * `commands` — ввод игрока (`UserCommand`, `Raw`);
/// * `events` — всё, что нужно показать (строки, промпты, MSDP, служебные сообщения);
/// * `recorder` — если задан, туда пишутся сырые байты от сервера (для replay и тестов).
pub async fn run_session(
    address: &str,
    mut session: Session,
    mut commands: mpsc::Receiver<SessionInput>,
    events: mpsc::Sender<SessionOutput>,
    mut recorder: Option<std::fs::File>,
) -> anyhow::Result<()> {
    let stream = TcpStream::connect(address).await?;
    stream.set_nodelay(true)?;
    let (mut reader, mut writer) = stream.into_split();

    // Горячие клавиши — сразу, чтобы интерфейс знал их до первой строки от сервера.
    if events
        .send(SessionOutput::HotkeysChanged(session.hotkeys()))
        .await
        .is_err()
    {
        return Ok(());
    }
    for op in session.window_names() {
        if events.send(SessionOutput::Window(op)).await.is_err() {
            return Ok(());
        }
    }
    let mut buf = vec![0u8; 16 * 1024];
    // Когда показать незаконченную строку, если сервер так ничего и не допишет.
    let mut flush_at: Option<Instant> = None;
    // Время сессии считается от подключения; таймеры автоматизации — от него же.
    let started = Instant::now();
    session.set_now(Duration::ZERO);
    let mut timers_at = session.next_deadline().map(|d| started + d);
    let mut log = SessionLog::default();

    loop {
        // `select!` ждёт сразу несколько событий и обрабатывает то, что случилось первым.
        let input = tokio::select! {
            read = reader.read(&mut buf) => {
                let n = read?;
                if n == 0 {
                    SessionInput::Disconnected
                } else {
                    if let Some(file) = recorder.as_mut() {
                        // Запись маленькая и редкая, блокирующий write здесь допустим.
                        file.write_all(&buf[..n])?;
                    }
                    SessionInput::Net(buf[..n].to_vec())
                }
            }
            command = commands.recv() => match command {
                Some(input) => input,
                None => break, // интерфейс закрылся
            },
            _ = sleep_until(flush_at) => SessionInput::FlushPartial,
            _ = sleep_until(timers_at) => SessionInput::Timers,
        };
        session.set_now(started.elapsed());

        let disconnected = matches!(input, SessionInput::Disconnected);
        let outputs = session.handle(input)?;
        flush_at = session
            .has_pending()
            .then(|| Instant::now() + PARTIAL_LINE_TIMEOUT);
        timers_at = session.next_deadline().map(|d| started + d);

        let mut close = false;
        for output in outputs {
            // Ошибка записи лога сообщается игроку один раз, сессия продолжается.
            if let Some(warning) = log.write(&output) {
                let _ = events.send(SessionOutput::Warning(warning)).await;
            }
            match output {
                SessionOutput::Send(bytes) => writer.write_all(&bytes).await?,
                SessionOutput::CloseRequested => close = true,
                SessionOutput::Log(op) => {
                    let message = log.apply(op);
                    if events.send(message).await.is_err() {
                        return Ok(());
                    }
                }
                other => {
                    if events.send(other).await.is_err() {
                        return Ok(()); // интерфейс больше не слушает
                    }
                }
            }
        }

        if close {
            let _ = writer.shutdown().await;
            let _ = events.send(SessionOutput::Disconnected).await;
            break;
        }
        if disconnected {
            break;
        }
    }
    Ok(())
}

/// Лог сессии (`#log`): текст строк, промптов и сообщений клиента — без цветов.
/// Ввод игрока сюда не попадает (его эхо рисует интерфейс), поэтому и паролей в логе нет.
#[derive(Default)]
struct SessionLog {
    file: Option<std::io::BufWriter<std::fs::File>>,
    path: std::path::PathBuf,
}

impl SessionLog {
    /// Начать/остановить; возвращает сообщение для игрока.
    fn apply(&mut self, op: LogOp) -> SessionOutput {
        self.close();
        match op {
            LogOp::Stop if self.path.as_os_str().is_empty() => {
                SessionOutput::Echo("#log: лог не ведётся".to_string())
            }
            LogOp::Stop => {
                let path = std::mem::take(&mut self.path);
                SessionOutput::Echo(format!("#log: лог остановлен ({})", path.display()))
            }
            LogOp::Start { path, append } => match open_log(&path, append) {
                Ok(file) => {
                    self.file = Some(std::io::BufWriter::new(file));
                    self.path = path;
                    SessionOutput::Echo(format!("#log: пишу в {}", self.path.display()))
                }
                Err(e) => {
                    self.path.clear();
                    SessionOutput::Warning(format!("#log: не открыть {}: {e}", path.display()))
                }
            },
        }
    }

    /// Записать то, что увидит игрок. При ошибке лог закрывается и возвращается предупреждение.
    fn write(&mut self, output: &SessionOutput) -> Option<String> {
        let file = self.file.as_mut()?;
        let text = match output {
            SessionOutput::Line(line) | SessionOutput::Prompt(line) => line.text(),
            SessionOutput::Echo(text) | SessionOutput::Warning(text) => text.clone(),
            _ => return None,
        };
        // Сбрасываем на диск после каждой строки: лог должен пережить падение клиента.
        let result = writeln!(file, "{text}").and_then(|_| file.flush());
        match result {
            Ok(()) => None,
            Err(e) => {
                self.file = None;
                Some(format!("#log: запись остановлена: {e}"))
            }
        }
    }

    fn close(&mut self) {
        if let Some(mut file) = self.file.take() {
            let _ = file.flush();
        }
    }
}

fn open_log(path: &std::path::Path, append: bool) -> std::io::Result<std::fs::File> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(path)
}

/// Ждёт до момента `at`; если момента нет — ждёт вечно (ветка `select!` никогда не сработает).
async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Прогоняет записанный трафик через сессию без сети.
///
/// Байты подаются кусками по `chunk_size`, в конце — `FlushPartial` и `Disconnected`.
/// Отправка на сервер (`Send`) отбрасывается.
pub fn replay(
    config: SessionConfig,
    automation: Automation,
    recording: &[u8],
    chunk_size: usize,
) -> anyhow::Result<Vec<SessionOutput>> {
    let mut session = Session::new(config, automation);
    let mut all = Vec::new();
    let inputs = recording
        .chunks(chunk_size.max(1))
        .map(|c| SessionInput::Net(c.to_vec()))
        .chain([SessionInput::FlushPartial, SessionInput::Disconnected]);
    for input in inputs {
        all.extend(
            session
                .handle(input)?
                .into_iter()
                .filter(|o| !matches!(o, SessionOutput::Send(_))),
        );
    }
    Ok(all)
}
