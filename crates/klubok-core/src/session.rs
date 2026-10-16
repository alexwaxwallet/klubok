//! `Session` — единственный владелец состояния подключения.
//!
//! Сама сессия ничего не знает о сети и времени: ей присылают `SessionInput`,
//! она возвращает список `SessionOutput`. Поэтому её легко тестировать,
//! просто подавая байты. Сеть и таймеры — в `net.rs`.

use klubok_automation::{BlankLineFilter, BlankLineSettings};
use klubok_proto::telnet::{self, opt};
use klubok_proto::{
    AnsiParser, Charset, InputDecoder, LineAssembler, MsdpValue, RawLine, StyledLine, TelnetEvent,
    Verb, msdp,
};

/// Настройки подключения.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub charset: Charset,
    /// Самому выбрать кодировку в меню «Using keytable» при входе.
    pub auto_charset: bool,
    /// Соглашаться на сжатие MCCP v2.
    pub mccp: bool,
    /// Соглашаться на MSDP и подписываться на комнату.
    pub msdp: bool,
    pub blank_lines: BlankLineSettings,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            charset: Charset::Utf8,
            auto_charset: true,
            mccp: true,
            msdp: true,
            blank_lines: BlankLineSettings::default(),
        }
    }
}

/// Что приходит в сессию.
#[derive(Debug, Clone)]
pub enum SessionInput {
    /// Байты из сокета.
    Net(Vec<u8>),
    /// Команда, которую ввёл игрок.
    UserCommand(String),
    /// Сервер замолчал, не закончив строку и не прислав GA: показать её как промпт.
    FlushPartial,
    /// Сервер закрыл соединение.
    Disconnected,
}

/// Что выходит из сессии.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionOutput {
    /// Строка для экрана.
    Line(StyledLine),
    /// Промпт (строка без перевода строки, после которой ждут ввода).
    Prompt(StyledLine),
    /// Байты, которые нужно отправить серверу.
    Send(Vec<u8>),
    /// Переменная MSDP (например, `ROOM`).
    Msdp(String, MsdpValue),
    /// Служебное сообщение клиента (переговоры, ошибки) — для отладки.
    Info(String),
    Disconnected,
}

pub struct Session {
    config: SessionConfig,
    decoder: InputDecoder,
    lines: LineAssembler,
    ansi: AnsiParser,
    blank: BlankLineFilter,
    /// Кодировка уже выбрана в меню при входе.
    charset_selected: bool,
}

impl Session {
    pub fn new(config: SessionConfig) -> Session {
        Session {
            decoder: InputDecoder::new(),
            lines: LineAssembler::new(),
            ansi: AnsiParser::new(),
            blank: BlankLineFilter::new(config.blank_lines),
            charset_selected: !config.auto_charset,
            config,
        }
    }

    /// Есть незаконченная строка — сетевому циклу стоит запустить таймер `FlushPartial`.
    pub fn has_partial(&self) -> bool {
        self.lines.has_partial()
    }

    pub fn handle(&mut self, input: SessionInput) -> anyhow::Result<Vec<SessionOutput>> {
        let mut out = Vec::new();
        match input {
            SessionInput::Net(bytes) => {
                for event in self.decoder.feed(&bytes)? {
                    self.on_telnet(event, &mut out);
                }
            }
            SessionInput::UserCommand(command) => {
                out.push(SessionOutput::Send(self.encode_command(&command)));
            }
            SessionInput::FlushPartial => {
                if let Some(raw) = self.lines.take_partial() {
                    self.on_raw_line(raw, &mut out);
                }
            }
            SessionInput::Disconnected => {
                if let Some(raw) = self.lines.take_partial() {
                    self.on_raw_line(raw, &mut out);
                }
                out.push(SessionOutput::Disconnected);
            }
        }
        Ok(out)
    }

    fn on_telnet(&mut self, event: TelnetEvent, out: &mut Vec<SessionOutput>) {
        match event {
            TelnetEvent::Data(data) => {
                let mut raw = Vec::new();
                self.lines.feed(&data, &mut raw);
                for line in raw {
                    self.on_raw_line(line, out);
                }
            }
            TelnetEvent::GoAhead => {
                let mut raw = Vec::new();
                self.lines.go_ahead(&mut raw);
                for line in raw {
                    self.on_raw_line(line, out);
                }
            }
            TelnetEvent::Negotiate(verb, option) => self.negotiate(verb, option, out),
            TelnetEvent::Subnegotiation(opt::MSDP, data) => {
                for (name, value) in msdp::decode(&data, self.config.charset) {
                    out.push(SessionOutput::Msdp(name, value));
                }
            }
            TelnetEvent::Subnegotiation(option, _) => {
                out.push(SessionOutput::Info(format!(
                    "неизвестная подкоманда telnet {option}"
                )));
            }
            TelnetEvent::CompressionStarted => {
                out.push(SessionOutput::Info("сжатие MCCP v2 включено".to_string()));
            }
        }
    }

    /// Отвечает на предложения сервера. Сами мы ничего не предлагаем: Былины
    /// не отвечают на неизвестные опции, а их байты попадают в текст команды.
    fn negotiate(&mut self, verb: Verb, option: u8, out: &mut Vec<SessionOutput>) {
        let reply = match (verb, option) {
            (Verb::Will, opt::COMPRESS2) if self.config.mccp => Verb::Do,
            (Verb::Will, opt::MSDP) if self.config.msdp => {
                out.push(SessionOutput::Send(telnet::negotiate(Verb::Do, opt::MSDP)));
                out.push(SessionOutput::Send(msdp::report("ROOM")));
                out.push(SessionOutput::Info(
                    "MSDP включён, подписка на ROOM".to_string(),
                ));
                return;
            }
            (Verb::Will, _) => Verb::Dont,
            (Verb::Do, _) => Verb::Wont,
            // На отказы не отвечаем, чтобы не зациклиться.
            (Verb::Wont | Verb::Dont, _) => return,
        };
        out.push(SessionOutput::Send(telnet::negotiate(reply, option)));
    }

    fn on_raw_line(&mut self, raw: RawLine, out: &mut Vec<SessionOutput>) {
        match raw {
            RawLine::Line(bytes) => {
                let line = self.ansi.parse(&self.config.charset.decode(&bytes));
                if self.blank.keep(&line) {
                    out.push(SessionOutput::Line(line));
                }
            }
            RawLine::Prompt(bytes) => {
                let line = self.ansi.parse(&self.config.charset.decode(&bytes));
                self.maybe_select_charset(&line, out);
                self.blank.on_prompt();
                out.push(SessionOutput::Prompt(line));
            }
        }
    }

    /// При входе сервер спрашивает кодировку («Select one : ») — отвечаем сами.
    fn maybe_select_charset(&mut self, prompt: &StyledLine, out: &mut Vec<SessionOutput>) {
        if self.charset_selected || !prompt.text().contains("Select one") {
            return;
        }
        self.charset_selected = true;
        let choice = self.config.charset.server_menu_choice();
        out.push(SessionOutput::Send(self.encode_command(choice)));
        out.push(SessionOutput::Info(format!(
            "выбрана кодировка {:?} (пункт {choice})",
            self.config.charset
        )));
    }

    fn encode_command(&self, command: &str) -> Vec<u8> {
        let mut bytes = self.config.charset.encode(command);
        bytes.extend(b"\r\n");
        telnet::escape_iac(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(session: &mut Session, bytes: &[u8]) -> Vec<SessionOutput> {
        session
            .handle(SessionInput::Net(bytes.to_vec()))
            .expect("разбор")
    }

    fn sent(out: &[SessionOutput]) -> Vec<Vec<u8>> {
        out.iter()
            .filter_map(|o| match o {
                SessionOutput::Send(b) => Some(b.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn переговоры_при_подключении() {
        let mut s = Session::new(SessionConfig::default());
        let out = feed(&mut s, b"\xff\xfbE\xff\xfbV\xff\xfbU");
        assert_eq!(
            sent(&out),
            vec![
                b"\xff\xfdE".to_vec(),
                msdp::report("ROOM"),
                b"\xff\xfdV".to_vec(),
                b"\xff\xfeU".to_vec(),
            ]
        );
    }

    #[test]
    fn сам_выбирает_кодировку() {
        let mut s = Session::new(SessionConfig::default());
        feed(&mut s, b"Using keytable\r\n  5) UTF-8\r\nSelect one : ");
        assert!(s.has_partial());
        let out = s.handle(SessionInput::FlushPartial).unwrap();
        assert_eq!(sent(&out), vec![b"5\r\n".to_vec()]);

        // Второй раз меню не трогаем.
        feed(&mut s, b"Select one : ");
        let out = s.handle(SessionInput::FlushPartial).unwrap();
        assert!(sent(&out).is_empty());
    }

    #[test]
    fn команда_в_cp1251_с_буквой_я() {
        let config = SessionConfig {
            charset: Charset::Cp1251,
            ..SessionConfig::default()
        };
        let mut s = Session::new(config);
        let out = s
            .handle(SessionInput::UserCommand("я".to_string()))
            .unwrap();
        assert_eq!(sent(&out), vec![b"\xff\xff\r\n".to_vec()]);
    }

    #[test]
    fn пустые_строки_схлопываются() {
        let mut s = Session::new(SessionConfig::default());
        let mut bytes = b"20H> \xff\xf9\r\n\r\n".to_vec();
        bytes.extend("Вы ударили.\r\n\r\n\r\nОн умер.\r\n".as_bytes());
        let out = feed(&mut s, &bytes);
        let texts: Vec<String> = out
            .iter()
            .filter_map(|o| match o {
                SessionOutput::Line(l) | SessionOutput::Prompt(l) => Some(l.text()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["20H> ", "Вы ударили.", "", "Он умер."]);
    }
}
