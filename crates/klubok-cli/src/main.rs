//! Консольный клиент для отладки ядра без графического интерфейса.
//!
//! Подключение к локальному серверу:   `cargo run -p klubok-cli`
//! Запись трафика:                     `cargo run -p klubok-cli -- --record recordings/x.local.bin`
//! Прогон записи без сети:             `cargo run -p klubok-cli -- --replay recordings/x.local.bin`

use std::io::Write;
use std::path::PathBuf;

use clap::Parser;
use klubok_automation::BlankLineSettings;
use klubok_core::{SessionConfig, SessionOutput};
use klubok_proto::{Charset, Color, StyledLine};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

#[derive(Parser)]
#[command(
    name = "klubok-cli",
    version,
    about = "Клубок — консольный мад-клиент для отладки"
)]
struct Args {
    /// Адрес сервера.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// Порт сервера.
    #[arg(long, default_value_t = 4000)]
    port: u16,
    /// Кодировка: utf8, koi8r или cp1251.
    #[arg(long, default_value = "utf8")]
    charset: Charset,
    /// Сколько пустых строк подряд оставлять (0 — убирать все).
    #[arg(long, default_value_t = 1)]
    max_blank: usize,
    /// Не убирать пустые строки сразу после промпта.
    #[arg(long)]
    keep_blank_after_prompt: bool,
    /// Отключить сжатие MCCP.
    #[arg(long)]
    no_mccp: bool,
    /// Показывать служебные сообщения и данные MSDP.
    #[arg(long, short)]
    debug: bool,
    /// Записывать сырые байты от сервера в файл.
    #[arg(long)]
    record: Option<PathBuf>,
    /// Не подключаться, а прогнать записанный файл.
    #[arg(long, conflicts_with = "record")]
    replay: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    // В старой консоли Windows ANSI-цвета выключены — включаем. В других системах ничего не делает.
    let _ = enable_ansi_support::enable_ansi_support();
    let config = SessionConfig {
        charset: args.charset,
        mccp: !args.no_mccp,
        blank_lines: BlankLineSettings {
            max_consecutive: args.max_blank,
            trim_after_prompt: !args.keep_blank_after_prompt,
        },
        ..SessionConfig::default()
    };
    let mut screen = Screen::new(args.debug);

    if let Some(path) = &args.replay {
        let recording = std::fs::read(path)?;
        for output in klubok_core::replay(config, &recording, 4096)? {
            screen.show(&output);
        }
        screen.finish();
        return Ok(());
    }

    let recorder = args
        .record
        .as_ref()
        .map(std::fs::File::create)
        .transpose()?;
    let address = format!("{}:{}", args.host, args.port);
    eprintln!("Подключаюсь к {address}…");

    let (command_tx, command_rx) = mpsc::channel::<String>(64);
    let (event_tx, mut event_rx) = mpsc::channel::<SessionOutput>(1024);

    // Ввод игрока: каждая строка stdin — команда. Конец stdin закрывает сессию.
    tokio::spawn(async move {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if command_tx.send(line).await.is_err() {
                break;
            }
        }
    });

    let session = tokio::spawn(async move {
        klubok_core::run_session(&address, config, command_rx, event_tx, recorder).await
    });

    while let Some(output) = event_rx.recv().await {
        screen.show(&output);
    }
    screen.finish();
    session.await??;
    Ok(())
}

/// Вывод в терминал.
struct Screen {
    debug: bool,
    /// Последним был напечатан промпт (курсор стоит после него, а не в начале строки).
    after_prompt: bool,
}

impl Screen {
    fn new(debug: bool) -> Screen {
        Screen {
            debug,
            after_prompt: false,
        }
    }

    fn show(&mut self, output: &SessionOutput) {
        let mut out = std::io::stdout().lock();
        match output {
            SessionOutput::Line(line) => {
                self.line_start(&mut out);
                let _ = writeln!(out, "{}", to_ansi(line));
            }
            SessionOutput::Prompt(line) => {
                self.line_start(&mut out);
                let _ = write!(out, "{}", to_ansi(line));
                self.after_prompt = true;
            }
            SessionOutput::Msdp(name, value) if self.debug => {
                self.line_start(&mut out);
                let _ = writeln!(out, "\x1b[2m[MSDP] {name} = {value}\x1b[0m");
            }
            SessionOutput::Info(text) if self.debug => {
                self.line_start(&mut out);
                let _ = writeln!(out, "\x1b[2m[клубок] {text}\x1b[0m");
            }
            SessionOutput::Disconnected => {
                self.line_start(&mut out);
                let _ = writeln!(out, "\x1b[2m[клубок] соединение закрыто\x1b[0m");
            }
            _ => {}
        }
        let _ = out.flush();
    }

    /// Если курсор стоит после промпта — перейти на новую строку.
    fn line_start(&mut self, out: &mut impl Write) {
        if self.after_prompt {
            let _ = writeln!(out);
            self.after_prompt = false;
        }
    }

    fn finish(&mut self) {
        self.line_start(&mut std::io::stdout());
    }
}

/// `StyledLine` → строка с ANSI-кодами для терминала.
fn to_ansi(line: &StyledLine) -> String {
    let mut s = String::new();
    for span in &line.spans {
        let st = span.style;
        let mut codes: Vec<String> = vec!["0".into()];
        if st.bold {
            codes.push("1".into());
        }
        if st.italic {
            codes.push("3".into());
        }
        if st.underline {
            codes.push("4".into());
        }
        codes.extend(color_code(st.fg, false));
        codes.extend(color_code(st.bg, true));
        s.push_str(&format!("\x1b[{}m{}", codes.join(";"), span.text));
    }
    s.push_str("\x1b[0m");
    s
}

fn color_code(color: Color, background: bool) -> Option<String> {
    let (base, bright, ext) = if background {
        (40, 100, 48)
    } else {
        (30, 90, 38)
    };
    match color {
        Color::Default => None,
        Color::Named(n) if n < 8 => Some((base + n as u32).to_string()),
        Color::Named(n) => Some((bright + (n as u32 - 8)).to_string()),
        Color::Indexed(n) => Some(format!("{ext};5;{n}")),
        Color::Rgb(r, g, b) => Some(format!("{ext};2;{r};{g};{b}")),
    }
}
