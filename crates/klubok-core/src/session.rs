//! `Session` — единственный владелец состояния подключения.
//!
//! Сама сессия ничего не знает о сети и времени: ей присылают `SessionInput`,
//! она возвращает список `SessionOutput`. Поэтому её легко тестировать,
//! просто подавая байты. Сеть и таймеры — в `net.rs`.

use std::path::PathBuf;
use std::time::Duration;

use klubok_automation::{Automation, BlankLineFilter, BlankLineSettings, Effect, Hotkey, WindowOp};
use klubok_proto::telnet::{self, opt};
use klubok_proto::{
    AnsiParser, Charset, InputDecoder, LineAssembler, MsdpValue, RawLine, StyledLine, TelnetEvent,
    Verb, msdp,
};

use crate::login::{AutoLogin, Credentials, LoginStep};
use crate::mapper::{MapEdit, MapView, Mapper, MapperEvent};
use crate::status::{self, CharStatus};

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
    /// Папка для логов `#log` с относительным именем файла (`None` — текущая папка).
    pub log_dir: Option<PathBuf>,
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
            log_dir: None,
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
    /// Пора выполнить таймеры (`#5:10 {…}`, `#delay`, тикер) — см. [`Session::next_deadline`].
    Timers,
    /// Сервер закрыл соединение.
    Disconnected,
    /// Заменить все настройки автоматизации (редактор настроек): текст в формате JMC.
    ReplaceAutomation(String),
    /// Правка карты мышью.
    MapEdit(MapEdit),
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
    /// Карта вокруг игрока (текущая зона и уровень).
    Map(MapView),
    /// Окно вывода (`#woutput` и др.).
    Window(WindowOp),
    /// Состояние персонажа по MSDP изменилось.
    Status(CharStatus),
    /// Начать или остановить лог сессии (`#log`) — файлом занимается сетевой цикл.
    Log(LogOp),
    /// Нужно разорвать соединение (`#zap`).
    CloseRequested,
    Disconnected,
}

/// Что сделать с логом сессии.
#[derive(Debug, Clone, PartialEq)]
pub enum LogOp {
    /// Писать показанный текст в файл; `append` — дописывать, иначе начать заново.
    Start { path: PathBuf, append: bool },
    Stop,
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
    mapper: Mapper,
    /// Время от начала сессии (сообщает сетевой цикл через `set_now`).
    now: Duration,
    /// Сервер согласился на MSDP.
    msdp_active: bool,
    /// Комнату уже запросили после входа в игру (см. `request_room`).
    room_requested: bool,
    /// Состояние персонажа (жизнь, бодрость, группа…) из MSDP.
    status: CharStatus,
    /// Прошлый игровой промпт: `STATE` запрашиваем, только когда промпт изменился.
    last_prompt: String,
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
            mapper: Mapper::default(),
            now: Duration::ZERO,
            msdp_active: false,
            room_requested: false,
            status: CharStatus::default(),
            last_prompt: String::new(),
            automation,
            config,
        }
    }

    /// Подключает карту (по умолчанию — пустая карта в памяти).
    pub fn with_mapper(mut self, mapper: Mapper) -> Session {
        self.mapper = mapper;
        self
    }

    /// Есть что показать по таймеру — сетевому циклу стоит запустить `FlushPartial`.
    pub fn has_pending(&self) -> bool {
        self.lines.has_partial() || self.blank.has_pending()
    }

    /// Сообщает сессии время от её начала. Сетевой цикл вызывает это перед каждым `handle`.
    pub fn set_now(&mut self, now: Duration) {
        self.automation.set_now(now);
        self.now = now;
    }

    /// Когда (от начала сессии) прислать `SessionInput::Timers`.
    pub fn next_deadline(&self) -> Option<Duration> {
        self.automation.next_deadline()
    }

    /// Заголовки окон вывода из настроек — интерфейсу при подключении.
    pub fn window_names(&self) -> Vec<WindowOp> {
        self.automation
            .window_names
            .iter()
            .map(|(window, name)| WindowOp::Name {
                window: *window,
                name: name.clone(),
            })
            .collect()
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
            SessionInput::MapEdit(edit) => {
                let events = self.mapper.edit(edit);
                self.apply_mapper(events, &mut out);
            }
            SessionInput::ReplaceAutomation(text) => {
                let mut automation = Automation::new();
                automation.import(&text);
                self.automation = automation;
                self.apply_effects(vec![Effect::Changed], &mut out);
                out.push(SessionOutput::Echo("Настройки обновлены.".to_string()));
            }
            SessionInput::Raw(text) => {
                out.push(SessionOutput::Send(self.encode(&text)));
            }
            SessionInput::Timers => {
                let effects = self.automation.tick(self.now);
                self.apply_effects(effects, &mut out);
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
                if let Some(line) = self.automation.merge_flush() {
                    self.show_line(line, false, &mut out);
                }
                self.mapper.reset();
                self.automation.set_connected(false);
                out.push(SessionOutput::Disconnected);
            }
        }
        Ok(out)
    }

    fn apply_effects(&mut self, effects: Vec<Effect>, out: &mut Vec<SessionOutput>) {
        for effect in effects {
            match effect {
                Effect::Send(text) => self.send(&text, out),
                Effect::Echo(text) => out.push(SessionOutput::Echo(text)),
                Effect::Warning(text) => out.push(SessionOutput::Warning(text)),
                Effect::Bell => out.push(SessionOutput::Bell),
                Effect::Window(op) => out.push(SessionOutput::Window(op)),
                Effect::Disconnect => out.push(SessionOutput::CloseRequested),
                Effect::Client { name, args } if name == "log" => self.log_command(&args, out),
                Effect::Client { name, .. } if name == "connect" => {
                    out.push(SessionOutput::Echo(
                        "#connect: уже подключены (переподключение — кнопкой вверху)".to_string(),
                    ));
                }
                Effect::Client { name, args } => {
                    let events = self.mapper.command(&name, &args);
                    self.apply_mapper(events, out);
                }
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

    /// `#log {файл} {append|overwrite}`; без файла — остановить. Относительное имя — в папке логов.
    fn log_command(&mut self, args: &[String], out: &mut Vec<SessionOutput>) {
        let file = args.first().map(|a| a.trim()).unwrap_or_default();
        if file.is_empty() {
            out.push(SessionOutput::Log(LogOp::Stop));
            return;
        }
        let mode = args.get(1).map(|a| a.trim().to_lowercase()).unwrap_or_default();
        let append = !mode.starts_with('o') && !mode.starts_with("пере");
        let mut path = PathBuf::from(file);
        if path.is_relative()
            && let Some(dir) = &self.config.log_dir
        {
            path = dir.join(path);
        }
        out.push(SessionOutput::Log(LogOp::Start { path, append }));
    }

    /// Команда серверу: маппер запоминает шаги, чтобы узнать скрытые выходы.
    fn send(&mut self, text: &str, out: &mut Vec<SessionOutput>) {
        self.mapper.note_sent(text);
        out.push(SessionOutput::Send(self.encode(text)));
    }

    fn apply_mapper(&mut self, events: Vec<MapperEvent>, out: &mut Vec<SessionOutput>) {
        for event in events {
            match event {
                MapperEvent::Send(text) => self.send(&text, out),
                MapperEvent::Echo(text) => out.push(SessionOutput::Echo(text)),
                MapperEvent::Warning(text) => out.push(SessionOutput::Warning(text)),
                MapperEvent::View(view) => out.push(SessionOutput::Map(view)),
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
                    if name == "ROOM" {
                        let events = self.mapper.on_room(&value);
                        out.push(SessionOutput::Msdp(name, value));
                        self.apply_mapper(events, out);
                    } else if self.status.update(&name, &value) {
                        out.push(SessionOutput::Status(self.status.clone()));
                    } else {
                        out.push(SessionOutput::Msdp(name, value));
                    }
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
                self.msdp_active = true;
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
                // Промпт закрывает незаконченную склейку строк.
                if let Some(merged) = self.automation.merge_flush() {
                    self.show_line(merged, false, out);
                }
                self.automation.on_prompt(&mut line, &mut effects);
            }
            for held in self.blank.prompt() {
                out.push(SessionOutput::Line(held));
            }
            if !logging_in {
                self.request_room(&text, out);
                self.mapper.on_prompt();
            }
            out.push(SessionOutput::Prompt(line));
            self.apply_effects(effects, out);
        } else if logging_in {
            self.show_line(line, true, out);
        } else {
            // Маппер видит исходную строку (до склейки и замен).
            let events = self.mapper.on_line(&text);
            self.apply_mapper(events, out);
            for line in self.automation.merge_line(line) {
                self.show_line(line, false, out);
            }
        }
    }

    /// Строка от сервера (уже после склейки): триггеры, замены, подсветка, пустые строки.
    /// `raw` — показать как есть, без автоматизации (идёт вход).
    fn show_line(&mut self, mut line: StyledLine, raw: bool, out: &mut Vec<SessionOutput>) {
        let mut effects = Vec::new();
        if raw || self.automation.on_line(&mut line, &mut effects) {
            for shown in self.blank.line(line) {
                out.push(SessionOutput::Line(shown));
            }
        }
        // Команды триггеров отправляются после показа строки, которая их вызвала.
        self.apply_effects(effects, out);
    }

    /// После входа (и переподключения) сервер не присылает комнату, пока игрок не шагнёт
    /// или не посмотрит. Чтобы карта появилась сразу, на первом игровом промпте просим
    /// MSDP `SEND ROOM`.
    ///
    /// Только на игровом промпте (кончается на `>`): до входа персонажа у сервера нет
    /// персонажа, и запрос комнаты там опасен для сервера.
    ///
    /// Заодно — состояние персонажа: подписка на то, что сервер присылает сам, и первые значения.
    /// Текущие жизнь и бодрость (`STATE`) сервер сам не присылает — просим при каждом новом промпте
    /// (промпт обычно показывает жизнь, так что изменился промпт — изменилось и состояние).
    fn request_room(&mut self, prompt: &str, out: &mut Vec<SessionOutput>) {
        if !self.msdp_active || !prompt.trim_end().ends_with('>') {
            return;
        }
        if !self.room_requested {
            self.room_requested = true;
            out.push(SessionOutput::Send(msdp::send("ROOM")));
            for name in status::REPORTED {
                out.push(SessionOutput::Send(msdp::report(name)));
                out.push(SessionOutput::Send(msdp::send(name)));
            }
        }
        if prompt != self.last_prompt {
            self.last_prompt = prompt.to_string();
            out.push(SessionOutput::Send(msdp::send("STATE")));
        }
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

    #[test]
    fn склейка_строк_до_промпта() {
        let mut automation = Automation::new();
        automation.import("#merge {^Вы несете:} {^%0конец} {, }");
        let config = SessionConfig {
            auto_charset: false,
            ..SessionConfig::default()
        };
        let mut s = Session::new(config, automation);
        let mut bytes = "Вы несете:\r\n  меч\r\n  щит\r\n".as_bytes().to_vec();
        bytes.extend("20H> ".as_bytes());
        bytes.extend(b"\xff\xf9");
        let out = feed(&mut s, &bytes);
        assert_eq!(shown(&out), ["Вы несете:, меч, щит", "20H> "]);
    }

    #[test]
    fn таймер_отправляет_команду() {
        let mut automation = Automation::new();
        automation.import("#alias {старт} {#delay {2} {см}}");
        let config = SessionConfig {
            auto_charset: false,
            ..SessionConfig::default()
        };
        let mut s = Session::new(config, automation);
        s.set_now(Duration::from_secs(10));
        let out = s.handle(SessionInput::UserCommand("старт".into())).unwrap();
        assert!(sent(&out).is_empty());
        assert_eq!(s.next_deadline(), Some(Duration::from_secs(12)));
        s.set_now(Duration::from_secs(12));
        let out = s.handle(SessionInput::Timers).unwrap();
        assert_eq!(sent(&out), ["см\r\n".as_bytes().to_vec()]);
    }

    #[test]
    fn комната_запрашивается_на_первом_игровом_промпте() {
        let config = SessionConfig {
            auto_charset: false,
            ..SessionConfig::default()
        };
        let mut s = session(config);
        // Без MSDP не просим.
        let out = feed(&mut s, b"20H> \xff\xf9");
        assert!(!sent(&out).contains(&msdp::send("ROOM")));
        feed(&mut s, b"\xff\xfbE");
        // Промпт не игровой (вход) — не просим.
        let out = feed(&mut s, "Введите имя: ".as_bytes());
        assert!(sent(&out).is_empty());
        let out = feed(&mut s, b"\xff\xf9");
        assert!(!sent(&out).contains(&msdp::send("ROOM")));
        let out = feed(&mut s, b"500H 82M> \xff\xf9");
        let out = sent(&out);
        assert_eq!(out[0], msdp::send("ROOM"));
        // Заодно — подписка на состояние персонажа и текущие жизнь/бодрость.
        assert!(out.contains(&msdp::report("MAX_HIT")));
        assert_eq!(out.last(), Some(&msdp::send("STATE")));
        // Тот же промпт — ничего не просим.
        let out = feed(&mut s, b"500H 82M> \xff\xf9");
        assert!(sent(&out).is_empty());
        // Промпт изменился — только STATE.
        let out = feed(&mut s, b"450H 82M> \xff\xf9");
        assert_eq!(sent(&out), vec![msdp::send("STATE")]);
    }

    #[test]
    fn окно_вывода_из_триггера() {
        let mut automation = Automation::new();
        automation.import("#action {^%0 сказал вам : '%1'} {#woutput 1 {light green} {%0: %1}}\n#wname 1 {Болтовня}");
        let config = SessionConfig {
            auto_charset: false,
            ..SessionConfig::default()
        };
        let mut s = Session::new(config, automation);
        assert_eq!(
            s.window_names(),
            [WindowOp::Name {
                window: 1,
                name: "Болтовня".into()
            }]
        );
        let out = feed(&mut s, "Петя сказал вам : 'привет'\r\n".as_bytes());
        let line = out
            .iter()
            .find_map(|o| match o {
                SessionOutput::Window(WindowOp::Output { window: 1, line }) => Some(line.clone()),
                _ => None,
            })
            .expect("строка в окне 1");
        assert_eq!(line.text(), "Петя: привет");
        assert_ne!(line.spans[0].style, klubok_proto::Style::default());
    }

    #[test]
    fn log_относительный_путь_в_папке_логов() {
        let config = SessionConfig {
            log_dir: Some(PathBuf::from("данные/logs")),
            ..SessionConfig::default()
        };
        let mut s = session(config);
        let logs = |out: Vec<SessionOutput>| -> Vec<LogOp> {
            out.into_iter()
                .filter_map(|o| match o {
                    SessionOutput::Log(op) => Some(op),
                    _ => None,
                })
                .collect()
        };
        let out = s.handle(SessionInput::UserCommand("#log {бой.txt}".into())).unwrap();
        assert_eq!(
            logs(out),
            vec![LogOp::Start {
                path: PathBuf::from("данные/logs/бой.txt"),
                append: true
            }]
        );
        let out = s.handle(SessionInput::UserCommand("#log x.txt overwrite".into())).unwrap();
        assert!(matches!(&logs(out)[0], LogOp::Start { append: false, .. }));
        let out = s.handle(SessionInput::UserCommand("#log".into())).unwrap();
        assert_eq!(logs(out), vec![LogOp::Stop]);
    }
}

