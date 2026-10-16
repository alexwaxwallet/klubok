//! Сетевой цикл: соединяет `Session` с сокетом, вводом игрока и таймерами.

use std::io::Write;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::Instant;

use klubok_automation::Automation;

use crate::session::{Session, SessionConfig, SessionInput, SessionOutput};

/// Сколько ждать продолжения незаконченной строки, прежде чем показать её как промпт.
const PARTIAL_LINE_TIMEOUT: Duration = Duration::from_millis(150);

/// Подключается к серверу и крутит сессию, пока не закроется соединение
/// или не закроется канал команд.
///
/// * `automation` — алиасы, триггеры и прочие настройки профиля;
/// * `commands` — ввод игрока (`UserCommand`, `Raw`);
/// * `events` — всё, что нужно показать (строки, промпты, MSDP, служебные сообщения);
/// * `recorder` — если задан, туда пишутся сырые байты от сервера (для replay и тестов).
pub async fn run_session(
    address: &str,
    config: SessionConfig,
    automation: Automation,
    mut commands: mpsc::Receiver<SessionInput>,
    events: mpsc::Sender<SessionOutput>,
    mut recorder: Option<std::fs::File>,
) -> anyhow::Result<()> {
    let stream = TcpStream::connect(address).await?;
    stream.set_nodelay(true)?;
    let (mut reader, mut writer) = stream.into_split();

    let mut session = Session::new(config, automation);
    // Горячие клавиши — сразу, чтобы интерфейс знал их до первой строки от сервера.
    if events
        .send(SessionOutput::HotkeysChanged(session.hotkeys()))
        .await
        .is_err()
    {
        return Ok(());
    }
    let mut buf = vec![0u8; 16 * 1024];
    // Когда показать незаконченную строку, если сервер так ничего и не допишет.
    let mut flush_at: Option<Instant> = None;

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
        };

        let disconnected = matches!(input, SessionInput::Disconnected);
        let outputs = session.handle(input)?;
        flush_at = session
            .has_pending()
            .then(|| Instant::now() + PARTIAL_LINE_TIMEOUT);

        let mut close = false;
        for output in outputs {
            match output {
                SessionOutput::Send(bytes) => writer.write_all(&bytes).await?,
                SessionOutput::CloseRequested => close = true,
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
