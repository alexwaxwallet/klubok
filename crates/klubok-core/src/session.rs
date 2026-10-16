//! `Session` — единственный владелец состояния подключения.
//!
//! Сама сессия ничего не знает о сети и времени: ей присылают `SessionInput`,
//! она возвращает список `SessionOutput`. Поэтому её легко тестировать,
//! просто подавая байты. Сеть и таймеры — в `net.rs`.

use klubok_automation::{Automation, BlankLineFilter, BlankLineSettings, Effect, Hotkey};
use klubok_proto::telnet::{self, opt};
use klubok_proto::{
    AnsiParser, Charset, InputDecoder, LineAssembler, MsdpValue, RawLine, StyledLine, TelnetEvent,
    Verb, msdp,
};

use crate::login::{AutoLogin, Credentials, LoginStep};

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
    /// Имя и пароль для автовхода.
    pub login: Option<Credentials>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            charset: Charset::Utf8,
            auto_charset: true,
            mccp: true,
            msdp: true,
            blank_lines: BlankLineSettings::default(),
            login: None,
        }
    }
}

/// Что приходит в сессию.
#[derive(Debug, Clone)]
pub enum SessionInput {
    /// Байты из сокета.
    Net(Vec<u8>),
    /// Команда, которую ввёл игрок: проходит через алиасы и `#`-команды.
    UserCommand(String),
    /// Текст, который надо отправить как есть (пароль, имя на входе).
    Raw(String),
    /// Сервер замолчал: показать незаконченную строку как промпт и придержанные пустые строки.
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
    /// Сообщение клиента для игрока (`#output`, списки алиасов, автовход).
    Echo(String),
    /// Предупреждение для игрока (ошибка в команде, неподдерживаемая команда).
    Warning(String),
    /// Служебное сообщение (переговоры telnet) — для отладки.
    Info(String),
    /// Звуковой сигнал.
    Bell,
    /// Настройки автоматизации изменились; внутри — их новый текст для сохранения.
    AutomationChanged(String),
    /// Список горячих клавиш изменился.
    HotkeysChanged(Vec<Hotkey>),
    /// Нужно разорвать соединение (`#zap`).
    CloseRequested,
    Disconnected,
}

pub struct Session {
    config: SessionConfig,
    automation: Automation,
    decoder: InputDecoder,
    lines: LineAssembler,
    ansi: AnsiParser,
    blank: BlankLineFilter,
    login: Option<AutoLogin>,
    /// Кодировка уже выбрана в меню при входе.
    charset_selected: bool,
}

impl Session {
    pub fn new(config: SessionConfig, automation: Automation) -> Session {
        Session {
            decoder: InputDecoder::new(),
            lines: LineAssembler::new(),
            ansi: AnsiParser::new(),
            blank: BlankLineFilter::new(config.blank_lines),
            login: config.login.clone().map(AutoLogin::new),
            charset_selected: !config.auto_charset,
            automation,
            config,
        }
    }

    /// Есть что показать по таймеру — сетевому циклу стоит запустить `FlushPartial`.
    pub fn has_pending(&self) -> bool {
        self.lines.has_partial() || self.blank.has_pending()
    }

    pub fn hotkeys(&self) -> Vec<Hotkey> {
        self.automation.active_hotkeys()
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
                let effects = self.automation.user_input(&command);
                self.apply_effects(effects, &mut out);
            }
            SessionInput::Raw(text) => {
                out.push(SessionOutput::Send(self.encode(&text)));
            }
            SessionInput::FlushPartial => {
                for line in self.blank.flush() {
                    out.push(SessionOutput::Line(line));
                }
                if let Some(raw) = self.lines.take_partial() {
                    self.on_raw_line(raw, &mut out);
                }
            }
            SessionInput::Disconnected => {
                for line in self.blank.flush() {
                    out.push(SessionOutput::Line(line));
                }
                if let Some(raw) = self.lines.take_partial() {
                    self.on_raw_line(raw, &mut out);
                }
                out.push(SessionOutput::Disconnected);
            }
        }
        Ok(out)
    }

    fn apply_effects(&mut self, effects: Vec<Effect>, out: &mut Vec<SessionOutput>) {
        for effect in effects {
            match effect {
                Effect::Send(text) => out.push(SessionOutput::Send(self.encode(&text))),
                Effect::Echo(text) => out.push(SessionOutput::Echo(text)),
                Effect::Warning(text) => out.push(SessionOutput::Warning(text)),
                Effect::Bell => out.push(SessionOutput::Bell),
                Effect::Disconnect => out.push(SessionOutput::CloseRequested),
                Effect::Changed => {
                    out.push(SessionOutput::AutomationChanged(
                        self.automation.to_script(),
                    ));
                    out.push(SessionOutput::HotkeysChanged(
                        self.automation.active_hotkeys(),
                    ));
                }
            }
        }
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
        let (bytes, is_prompt) = match raw {
            RawLine::Line(b) => (b, false),
            RawLine::Prompt(b) => (b, true),
        };
        let mut line = self.ansi.parse(&self.config.charset.decode(&bytes));
        let text = line.text();
        if is_prompt {
            self.maybe_select_charset(&text, out);
        }
        self.run_login(&text, out);

        // Пока идёт вход, триггеры не запускаются: в настройках JMC часто есть свои
        // триггеры входа («Select one» → 2, «Введите имя» → имя), и они мешают автовходу.
        let logging_in =
            !self.charset_selected || self.login.as_ref().is_some_and(|l| !l.is_done());

        let mut effects = Vec::new();
        if is_prompt {
            if !logging_in {
                self.automation.on_prompt(&mut line, &mut effects);
            }
            for held in self.blank.prompt() {
                out.push(SessionOutput::Line(held));
            }
            out.push(SessionOutput::Prompt(line));
        } else if logging_in || self.automation.on_line(&mut line, &mut effects) {
            for shown in self.blank.line(line) {
                out.push(SessionOutput::Line(shown));
            }
        }
        // Команды триггеров отправляются после показа строки, которая их вызвала.
        self.apply_effects(effects, out);
    }

    fn run_login(&mut self, text: &str, out: &mut Vec<SessionOutput>) {
        let Some(login) = self.login.as_mut() else {
            return;
        };
        for step in login.check(text) {
            match step {
                LoginStep::SendSecret(secret) | LoginStep::Send(secret) => {
                    out.push(SessionOutput::Send(self.encode(&secret)));
                }
                LoginStep::Notice(text) => out.push(SessionOutput::Echo(text)),
            }
        }
    }

    /// При входе сервер спрашивает кодировку («Select one : ») — отвечаем сами.
    fn maybe_select_charset(&mut self, prompt: &str, out: &mut Vec<SessionOutput>) {
        if self.charset_selected || !prompt.contains("Select one") {
            return;
        }
        self.charset_selected = true;
        let choice = self.config.charset.server_menu_choice();
        out.push(SessionOutput::Send(self.encode(choice)));
        out.push(SessionOutput::Info(format!(
            "выбрана кодировка {:?} (пункт {choice})",
            self.config.charset
        )));
    }

    /// Текст → байты для сервера: кодировка, перевод строки, удвоение IAC.
    fn encode(&self, command: &str) -> Vec<u8> {
        let mut bytes = self.config.charset.encode(command);
        bytes.extend(b"\r\n");
        telnet::escape_iac(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(config: SessionConfig) -> Session {
        Session::new(config, Automation::new())
    }

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

    fn shown(out: &[SessionOutput]) -> Vec<String> {
        out.iter()
            .filter_map(|o| match o {
                SessionOutput::Line(l) | SessionOutput::Prompt(l) => Some(l.text()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn переговоры_при_подключении() {
        let mut s = session(SessionConfig::default());
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
        let mut s = session(SessionConfig::default());
        feed(&mut s, b"Using keytable\r\n  5) UTF-8\r\nSelect one : ");
        assert!(s.has_pending());
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
        let mut s = session(config);
        let out = s
            .handle(SessionInput::UserCommand("я".to_string()))
            .unwrap();
        assert_eq!(sent(&out), vec![b"\xff\xff\r\n".to_vec()]);
    }

    #[test]
    fn пустые_строки_схлопываются() {
        let mut s = session(SessionConfig::default());
        let mut bytes = b"20H> \xff\xf9\r\n\r\n".to_vec();
        bytes.extend("Вы ударили.\r\n\r\n\r\nОн умер.\r\n\r\n21H> ".as_bytes());
        bytes.extend(b"\xff\xf9");
        let out = feed(&mut s, &bytes);
        assert_eq!(
            shown(&out),
            ["20H> ", "Вы ударили.", "", "Он умер.", "21H> "]
        );
    }

    #[test]
    fn автовход() {
        let config = SessionConfig {
            login: Some(Credentials {
                name: "Тестер".into(),
                password: "секрет;#1".into(),
            }),
            ..SessionConfig::default()
        };
        let mut s = session(config);
        let out = feed(&mut s, "Введите имя персонажа: ".as_bytes());
        assert!(sent(&out).is_empty());
        let out = s.handle(SessionInput::FlushPartial).unwrap();
        // Пароль уходит как есть: «;» и «#» не трогаются.
        assert_eq!(sent(&out), vec!["Тестер секрет;#1\r\n".as_bytes().to_vec()]);
    }

    #[test]
    fn триггеры_входа_из_jmc_не_мешают_автовходу() {
        let mut automation = Automation::new();
        automation.import(
            "#action TEXT {^Select one : } {2} {5} {default}\n\
             #action TEXT {^Введите имя персонажа} {Колдун} {5} {default}",
        );
        let config = SessionConfig {
            login: Some(Credentials {
                name: "Тестер".into(),
                password: "п".into(),
            }),
            ..SessionConfig::default()
        };
        let mut s = Session::new(config, automation);
        feed(&mut s, b"Select one : ");
        let out = s.handle(SessionInput::FlushPartial).unwrap();
        assert_eq!(sent(&out), vec![b"5\r\n".to_vec()]);
        feed(&mut s, "Введите имя персонажа: ".as_bytes());
        let out = s.handle(SessionInput::FlushPartial).unwrap();
        assert_eq!(sent(&out), vec!["Тестер п\r\n".as_bytes().to_vec()]);
    }

    #[test]
    fn триггер_и_алиас_в_сессии() {
        let mut automation = Automation::new();
        automation
            .import("#action {^Вы проголодались.} {есть хлеб}\n#alias {вх} {колд !вихрь! %1}");
        let config = SessionConfig {
            auto_charset: false, // меню кодировки не будет — сразу «в игре»
            ..SessionConfig::default()
        };
        let mut s = Session::new(config, automation);

        let out = feed(&mut s, "Вы проголодались.\r\n".as_bytes());
        assert_eq!(sent(&out), vec!["есть хлеб\r\n".as_bytes().to_vec()]);

        let out = s
            .handle(SessionInput::UserCommand("вх тролль; см".into()))
            .unwrap();
        assert_eq!(
            sent(&out),
            vec![
                "колд !вихрь! тролль\r\n".as_bytes().to_vec(),
                "см\r\n".as_bytes().to_vec()
            ]
        );

        let out = s
            .handle(SessionInput::UserCommand("#alias {а} {б}".into()))
            .unwrap();
        assert!(out.iter().any(
            |o| matches!(o, SessionOutput::AutomationChanged(t) if t.contains("#alias {а} {б}"))
        ));
    }
}
