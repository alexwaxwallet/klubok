//! Интерпретатор команд в стиле JMC и обработка строк от сервера.
//!
//! `Automation` хранит алиасы, триггеры, переменные и т.д. и ничего не знает о сети:
//! на вход — текст команд или строка от сервера, на выход — список `Effect`
//! (что отправить, что показать). Сессия решает, что с этим делать.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use klubok_proto::StyledLine;

use crate::color::{HighlightColor, parse_highlight};
use crate::expr;
use crate::js::{ScriptCall, ScriptEngine, ScriptSlot};
use crate::merge::{MergeRule, Merger};
use crate::pattern::Pattern;
use crate::script::{
    braced, hash_command_rest, parse_hash_command, split_commands, substitute_params,
    substitute_vars,
};
use crate::timers::{
    DECISECOND, DEFAULT_TICK_SIZE, MAX_TIMERS, ScriptTimer, TICK_TEXT, TICK_WARNING_TEXT,
    TickEvent, Ticker, Timer, TimerKind, parse_loop_spec, parse_repeat, parse_seconds,
};

/// Насколько глубоко алиасы и триггеры могут вызывать друг друга (защита от зацикливания).
const MAX_DEPTH: u8 = 16;
/// Цикл без задержки (`#100 {…}`) выполняется сразу; больше повторов не делаем.
const MAX_INSTANT_REPEATS: i64 = 10_000;
pub const DEFAULT_GROUP: &str = "default";
const DEFAULT_PRIORITY: i32 = 5;

/// Что должна сделать сессия.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Отправить команду серверу (уже после алиасов и подстановки переменных).
    Send(String),
    /// Показать сообщение клиента (`#output`, списки алиасов и т.п.).
    Echo(String),
    /// Предупреждение: ошибка в команде, неподдерживаемая команда.
    Warning(String),
    /// Звуковой сигнал (`#bell`).
    Bell,
    /// Разорвать соединение (`#zap`).
    Disconnect,
    /// Настройки изменились — их стоит сохранить.
    Changed,
    /// Команда клиента вне автоматизации (карта: `#map`, `#go`) — её выполняет сессия.
    /// Аргументы уже с подставленными переменными.
    Client { name: String, args: Vec<String> },
}

/// Команды, которые автоматизация не выполняет сама, а передаёт сессии ([`Effect::Client`]).
/// `connect` — `#connect адрес порт` (в JMC чаще пишут сокращённо: `#con localhost 2000`).
pub const CLIENT_COMMANDS: &[&str] = &["map", "go", "stop", "connect"];

#[derive(Debug, Clone)]
pub struct Alias {
    pub name: String,
    pub body: String,
    pub group: String,
}

#[derive(Debug, Clone)]
pub struct Action {
    /// Тип проверки из JMC (`TEXT`, `RAW`, `COLOR`). Пока все проверяются как TEXT.
    pub kind: String,
    pub pattern: Pattern,
    pub body: String,
    pub priority: i32,
    pub group: String,
}

#[derive(Debug, Clone)]
pub struct Highlight {
    pub color_spec: String,
    color: HighlightColor,
    pub pattern: Pattern,
    pub group: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotkey {
    /// Клавиша в записи JMC: `F5`, `Ctrl+NUM7`, `Alt+NUM9`.
    pub key: String,
    pub body: String,
    pub group: String,
}

/// Замена текста в строке; `replacement: None` — строку скрыть (gag).
#[derive(Debug, Clone)]
pub struct Substitution {
    pub pattern: Pattern,
    pub replacement: Option<String>,
    pub group: String,
}

/// Итог импорта настроек.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub aliases: usize,
    pub actions: usize,
    pub variables: usize,
    pub highlights: usize,
    pub hotkeys: usize,
    pub substitutions: usize,
    /// Триггеры входа (кодировка, имя, пароль). При автовходе Клубок их не запускает.
    pub login_actions: Vec<String>,
    /// Строки, которые сохранены как есть, но пока не работают (команда → сколько раз).
    pub unsupported: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Default)]
pub struct Automation {
    pub aliases: Vec<Alias>,
    pub actions: Vec<Action>,
    pub variables: BTreeMap<String, String>,
    pub highlights: Vec<Highlight>,
    pub hotkeys: Vec<Hotkey>,
    pub substitutions: Vec<Substitution>,
    /// Склейка строк (`#merge`).
    pub merges: Vec<MergeRule>,
    merger: Merger,
    /// Выключенные группы (`#group disable имя`).
    pub disabled_groups: BTreeSet<String>,
    /// Строки настроек, которые клиент пока не понимает, — сохраняются при записи.
    pub preserved: Vec<String>,
    /// О каких неподдерживаемых командах уже предупредили (чтобы не спамить).
    warned: BTreeSet<String>,
    /// Тикер JMC (`#tickon`, `#tickset`, `#ticksize`).
    pub ticker: Ticker,

    // ── время ──
    /// «Сейчас» — время от начала сессии, которое сообщила сессия (`set_now`/`tick`).
    now: Duration,
    /// Сессия сообщила, что соединения нет: тикер молчит, `jmc.isConnected` — false.
    disconnected: bool,
    /// Циклы с задержкой и `#delay` по номерам 0…15 (номер нужен для `#abort 3`).
    timers: BTreeMap<usize, Timer>,
    /// `#wait`: до этого момента обычные команды не отправляются, а копятся в очереди.
    wait_until: Option<Duration>,
    wait_queue: Vec<String>,

    // ── скрипты ──
    /// Движок JS для `#script` (создаётся при первом использовании).
    scripts: ScriptSlot,
    /// Таймеры скриптов (`jmc.setTimer`) по номерам.
    script_timers: BTreeMap<i64, ScriptTimer>,
    /// Обработчики событий скриптов (`jmc.registerHandler`): событие в нижнем регистре → код.
    script_handlers: BTreeMap<String, String>,
}

// `Automation` передают в задачу tokio — он обязан оставаться `Send`.
// Если это сломается, сборка упадёт здесь, а не где-то в сессии.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Automation>();
};

/// Команды настроек JMC, которые при импорте просто сохраняются (на поведение пока не влияют).
const SETTING_COMMANDS: &[&str] = &[
    "message",
    "multiaction",
    "multihighlight",
    "presub",
    "echo",
    "ignore",
    "speedwalk",
    "togglesubs",
    "verbat",
    "verbatim",
    "colon",
    "race",
    "proxy",
    "telnet",
    "oob",
    "broadcast",
    "promptend",
    "codepage",
    "comment",
    "secure",
];

impl Automation {
    pub fn new() -> Automation {
        Automation::default()
    }

    fn enabled(&self, group: &str) -> bool {
        !self.disabled_groups.contains(group)
    }

    // ───────────────────────────── время ─────────────────────────────

    /// Сообщает движку, который сейчас час: время от начала сессии (монотонное).
    ///
    /// Сессия вызывает это перед каждым `user_input`/`on_line`/`on_prompt`: команды вроде
    /// `#5:10 {…}`, `#wait`, `#tickset` должны знать «сейчас», чтобы поставить таймер.
    /// Почему не параметр каждого метода: так не меняются сигнатуры, которыми уже пользуется
    /// сессия, а в тестах время задаётся одной строкой. Сам движок часов не читает —
    /// поэтому тесты с выдуманным временем детерминированы.
    pub fn set_now(&mut self, now: Duration) {
        self.now = now;
    }

    /// Есть ли соединение с сервером. Без соединения тикер не показывает сообщений
    /// (как в JMC), а `jmc.isConnected` в скриптах — `false`. По умолчанию — есть.
    pub fn set_connected(&mut self, connected: bool) {
        self.disconnected = !connected;
    }

    /// Ограничение времени одного `#script` (по умолчанию 1 с).
    pub fn set_script_time_limit(&mut self, limit: Duration) {
        self.scripts.time_limit = limit;
    }

    /// Выполняет всё, что созрело к моменту `now`: очередь `#wait`, циклы с задержкой,
    /// `#delay`, таймеры скриптов, сообщения тикера. Когда звать в следующий раз —
    /// [`Automation::next_deadline`].
    pub fn tick(&mut self, now: Duration) -> Vec<Effect> {
        self.set_now(now);
        let mut fx = Vec::new();

        if self.wait_until.is_some_and(|t| now >= t) {
            self.wait_until = None;
            self.flush_wait_queue(0, &mut fx);
        }

        // Номера по порядку; команда таймера может менять сами таймеры (`#abort all`),
        // поэтому каждый раз ищем заново.
        for id in 0..MAX_TIMERS {
            let Some(timer) = self.timers.get_mut(&id) else {
                continue;
            };
            if !timer.active || timer.next > now {
                continue;
            }
            let (counter, done) = timer.fire(now);
            let command = match timer.kind {
                TimerKind::Cycle => timer.command.replace("%0", &counter.to_string()),
                TimerKind::Delay => timer.command.clone(),
            };
            if done || timer.kind == TimerKind::Delay {
                self.timers.remove(&id);
            }
            self.run(&command, 0, &mut fx);
        }

        let ids: Vec<i64> = self.script_timers.keys().copied().collect();
        for id in ids {
            let Some(timer) = self.script_timers.get_mut(&id) else {
                continue;
            };
            if timer.due() <= now {
                timer.last = now;
                timer.pre_done = false;
                self.fire_script_event("timer", id, &mut fx);
            } else if timer.pre_due().is_some_and(|t| t <= now) {
                timer.pre_done = true;
                self.fire_script_event("pretimer", id, &mut fx);
            }
        }

        // Счёт тиков идёт всегда, сообщения — только при соединении.
        let event = self.ticker.poll(now);
        if !self.disconnected {
            let text = match event {
                Some(TickEvent::Tick) => Some(TICK_TEXT),
                Some(TickEvent::Warning) => Some(TICK_WARNING_TEXT),
                None => None,
            };
            if let Some(text) = text {
                // Как в JMC: сообщение тикера проходит через триггеры (`#action {#ТИК} {…}`).
                fx.push(Effect::Echo(text.to_string()));
                self.fire_actions(text, &mut fx);
            }
        }
        dedup_changed(fx)
    }

    /// Когда в следующий раз вызвать [`Automation::tick`]; `None` — ждать нечего.
    pub fn next_deadline(&self) -> Option<Duration> {
        let timers = self.timers.values().filter(|t| t.active).map(|t| t.next);
        let script_timers = self
            .script_timers
            .values()
            .flat_map(|t| [Some(t.due()), t.pre_due()])
            .flatten();
        let ticker = if self.disconnected {
            None
        } else {
            self.ticker.deadline()
        };
        timers
            .chain(script_timers)
            .chain(self.wait_until)
            .chain(ticker)
            .min()
    }

    /// Запускает обработчик события скрипта (`jmc.registerHandler("Timer", …)`).
    fn fire_script_event(&mut self, event: &str, id: i64, fx: &mut Vec<Effect>) {
        if let Some(code) = self.script_handlers.get(event).cloned() {
            self.run_script(&code, vec![id.to_string()], 0, fx);
        }
    }

    fn flush_wait_queue(&mut self, depth: u8, fx: &mut Vec<Effect>) {
        // Команда из очереди может снова включить `#wait` — тогда остальные снова встанут в очередь.
        for command in std::mem::take(&mut self.wait_queue) {
            self.run(&command, depth, fx);
        }
    }

    // ───────────────────────────── ввод игрока ─────────────────────────────

    /// Команда, введённая игроком (или из горячей клавиши).
    pub fn user_input(&mut self, text: &str) -> Vec<Effect> {
        let mut fx = Vec::new();
        if text.trim().is_empty() {
            // Пустой Enter — тоже команда (например, «нажмите ENTER»).
            fx.push(Effect::Send(String::new()));
        } else {
            self.run(text, 0, &mut fx);
        }
        dedup_changed(fx)
    }

    /// Выполняет текст команд: разбивает по `;`, разворачивает алиасы, выполняет `#`-команды.
    fn run(&mut self, text: &str, depth: u8, fx: &mut Vec<Effect>) {
        if depth > MAX_DEPTH {
            fx.push(Effect::Warning(format!(
                "слишком глубокая вложенность алиасов/триггеров, остановлено на «{text}»"
            )));
            return;
        }
        for command in split_commands(text) {
            if command.starts_with('#') {
                self.hash_command(&command, depth, fx);
            } else {
                self.plain_command(&command, depth, fx);
            }
        }
    }

    fn plain_command(&mut self, command: &str, depth: u8, fx: &mut Vec<Effect>) {
        if self.wait_until.is_some() {
            // `#wait`: обычные команды (и алиасы) ждут; `#`-команды выполняются сразу.
            self.wait_queue
                .push(substitute_vars(command, &self.variables));
            return;
        }
        let (word, rest) = match command.split_once(char::is_whitespace) {
            Some((w, r)) => (w, r.trim()),
            None => (command, ""),
        };
        let alias = self
            .aliases
            .iter()
            .find(|a| a.name == word && self.enabled(&a.group))
            .cloned();
        match alias {
            Some(alias) => {
                // %0 — всё после имени алиаса, %1… — отдельные слова.
                let mut params = vec![rest.to_string()];
                params.extend(rest.split_whitespace().map(str::to_string));
                let body = substitute_params(&alias.body, &params);
                self.run(&body, depth + 1, fx);
            }
            None => fx.push(Effect::Send(substitute_vars(command, &self.variables))),
        }
    }

    fn hash_command(&mut self, command: &str, depth: u8, fx: &mut Vec<Effect>) {
        let Some((name, args)) = parse_hash_command(command) else {
            return;
        };
        // Сокращения как в JMC: `#con` → `#connect`, `#script` → `#scriptlet`.
        let name = canonical_command(&name);
        let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
        let var_arg = |i: usize| substitute_vars(&arg(i), &self.variables);

        match name.as_str() {
            "scriptlet" | "script" => {
                // Как в JMC: весь остаток строки — код; `$переменные` подставляются до запуска.
                let code = substitute_vars(hash_command_rest(command), &self.variables);
                self.run_script(&code, Vec::new(), depth, fx);
            }
            "tick" => {
                let text = if self.ticker.enabled {
                    format!("#{} секунд до тика.", self.ticker.seconds_left(self.now))
                } else {
                    "#Тикер выключен".to_string()
                };
                fx.push(Effect::Echo(text));
            }
            "tickon" | "tickoff" => {
                self.ticker.catch_up(self.now);
                self.ticker.enabled = name == "tickon";
                fx.push(Effect::Echo(
                    if self.ticker.enabled {
                        "#Тикер ВКЛЮЧЕН."
                    } else {
                        "#Тикер ВЫКЛЮЧЕН."
                    }
                    .to_string(),
                ));
                fx.push(Effect::Changed);
            }
            "tickset" => self.ticker.reset(self.now),
            "ticksize" => match var_arg(0).trim().parse::<u64>() {
                Ok(secs) if secs > 0 => {
                    self.ticker.size = Duration::from_secs(secs);
                    self.ticker.reset(self.now);
                    fx.push(Effect::Echo(
                        "#Установлена новая длительность тика.".to_string(),
                    ));
                    fx.push(Effect::Changed);
                }
                _ => fx.push(Effect::Echo(
                    "#Пожалуйста, укажите длительность тика в секундах.".to_string(),
                )),
            },
            "wait" | "wt" => self.wait_command(var_arg(0).trim(), depth, fx),
            "delay" => match parse_seconds(&var_arg(0)) {
                Some(delay) if args.len() >= 2 => {
                    self.add_timer(
                        Timer {
                            kind: TimerKind::Delay,
                            command: arg(1),
                            counter: 1,
                            end: 1,
                            step: 1,
                            period: Duration::ZERO,
                            next: self.now + delay,
                            active: true,
                        },
                        fx,
                    );
                }
                _ => fx.push(Effect::Warning(
                    "#delay {секунды} {команды}: не понял задержку".to_string(),
                )),
            },
            "loop" => match parse_loop_spec(&var_arg(0)) {
                Some((from, to, step, delay)) => {
                    self.start_cycle(from, to, step, delay, &arg(1), depth, fx)
                }
                None => fx.push(Effect::Warning(format!(
                    "#loop {{{}}}: нужно «от,до[|шаг][:задержка]»",
                    arg(0)
                ))),
            },
            "break" | "abort" | "resume" | "pinch" => self.timer_control(&name, &var_arg(0)),
            "tmlist" | "llist" => fx.push(Effect::Echo(self.list_timers())),
            _ if name.starts_with(|c: char| c.is_ascii_digit()) => match parse_repeat(&name) {
                // `#5 {см}` — пять раз сразу; `#5:10 {см}` — пять раз, раз в секунду.
                Some((count, delay)) => {
                    let body = if args.len() == 1 {
                        arg(0)
                    } else {
                        hash_command_rest(command).trim().to_string()
                    };
                    self.start_cycle(1, count, 1, delay, &body, depth, fx);
                }
                None => fx.push(Effect::Warning(format!(
                    "#{name}: нужно «#число {{команды}}» или «#число:задержка {{команды}}»"
                ))),
            },
            "alias" | "ali" | "al" => match args.len() {
                0 => fx.push(Effect::Echo(self.list_aliases(None))),
                1 => fx.push(Effect::Echo(self.list_aliases(Some(&args[0])))),
                _ => {
                    self.set_alias(&args[0], &args[1], args.get(2));
                    fx.push(Effect::Changed);
                }
            },
            "unalias" | "unali" => {
                let before = self.aliases.len();
                self.aliases.retain(|a| a.name != arg(0));
                if self.aliases.len() != before {
                    fx.push(Effect::Changed);
                }
            }
            "action" | "act" | "ac" => {
                let (kind, rest) = split_action_kind(&args);
                match rest.len() {
                    0 => fx.push(Effect::Echo(self.list_actions(None))),
                    1 => fx.push(Effect::Echo(self.list_actions(Some(&rest[0])))),
                    _ => {
                        let priority = rest
                            .get(2)
                            .and_then(|p| p.trim().parse().ok())
                            .unwrap_or(DEFAULT_PRIORITY);
                        self.set_action(&kind, &rest[0], &rest[1], priority, rest.get(3));
                        fx.push(Effect::Changed);
                    }
                }
            }
            "unaction" | "unact" | "unac" => {
                let before = self.actions.len();
                self.actions.retain(|a| a.pattern.source != arg(0));
                if self.actions.len() != before {
                    fx.push(Effect::Changed);
                }
            }
            "variable" | "var" => match args.len() {
                0 => fx.push(Effect::Echo(self.list_variables())),
                1 => {
                    let value = self.variables.get(&args[0]).cloned().unwrap_or_default();
                    fx.push(Effect::Echo(format!("${} = {value}", args[0])));
                }
                _ => {
                    let value = substitute_vars(&args[1..].join(" "), &self.variables);
                    self.set_variable(&args[0], &value, fx);
                }
            },
            "unvariable" | "unvar" => {
                if self.variables.remove(&arg(0)).is_some() {
                    fx.push(Effect::Changed);
                }
            }
            "highlight" | "high" => {
                if args.len() < 2 {
                    fx.push(Effect::Echo(self.list_highlights()));
                } else if self.set_highlight(&args[0], &args[1], args.get(2)) {
                    fx.push(Effect::Changed);
                } else {
                    fx.push(Effect::Warning(format!("непонятный цвет «{}»", args[0])));
                }
            }
            "unhighlight" | "unhigh" => {
                self.highlights.retain(|h| h.pattern.source != arg(0));
                fx.push(Effect::Changed);
            }
            "hot" | "hotkey" => {
                if args.len() < 2 {
                    fx.push(Effect::Echo(self.list_hotkeys()));
                } else {
                    self.set_hotkey(&args[0], &args[1], args.get(2));
                    fx.push(Effect::Changed);
                }
            }
            "unhot" | "unhotkey" => {
                self.hotkeys
                    .retain(|h| !h.key.eq_ignore_ascii_case(&arg(0)));
                fx.push(Effect::Changed);
            }
            "gag" => {
                self.set_substitution(&arg(0), None, args.get(1));
                fx.push(Effect::Changed);
            }
            "merge" => {
                if args.len() < 2 {
                    fx.push(Effect::Echo(self.list_merges()));
                } else {
                    self.set_merge(&args[0], &args[1], args.get(2), args.get(3));
                    fx.push(Effect::Changed);
                }
            }
            "unmerge" => {
                self.merges.retain(|m| m.start.source != arg(0));
                fx.push(Effect::Changed);
            }
            "substitute" | "sub" | "subs" => {
                if args.len() < 2 {
                    fx.push(Effect::Echo(self.list_substitutions()));
                } else {
                    self.set_substitution(&args[0], Some(&args[1]), args.get(2));
                    fx.push(Effect::Changed);
                }
            }
            "ungag" | "unsubstitute" | "unsub" | "unsubs" => {
                self.substitutions.retain(|s| s.pattern.source != arg(0));
                fx.push(Effect::Changed);
            }
            "if" => {
                let condition = substitute_vars(&arg(0), &self.variables);
                match expr::eval(&condition) {
                    Ok(v) if v.truthy() => self.run(&arg(1), depth + 1, fx),
                    Ok(_) => self.run(&arg(2), depth + 1, fx),
                    Err(e) => fx.push(Effect::Warning(format!("#if {{{condition}}}: {e}"))),
                }
            }
            "math" => {
                let expression = substitute_vars(&args[1..].join(" "), &self.variables);
                match expr::eval(&expression) {
                    Ok(v) => self.set_variable(&arg(0), &v.to_text(), fx),
                    Err(e) => fx.push(Effect::Warning(format!("#math {}: {e}", arg(0)))),
                }
            }
            "group" => {
                let mode = arg(0).to_lowercase();
                let group = arg(1);
                match mode.as_str() {
                    "enable" | "on" => {
                        self.disabled_groups.remove(&group);
                        fx.push(Effect::Changed);
                    }
                    "disable" | "off" => {
                        self.disabled_groups.insert(group);
                        fx.push(Effect::Changed);
                    }
                    _ => {} // другие формы JMC (`#group local default`) пока ни на что не влияют
                }
            }
            "output" | "showme" => {
                fx.push(Effect::Echo(substitute_vars(
                    &args.join(" "),
                    &self.variables,
                )));
            }
            "woutput" => {
                // Отдельных окон вывода пока нет — показываем в основном.
                fx.push(Effect::Echo(substitute_vars(
                    &args[1..].join(" "),
                    &self.variables,
                )));
            }
            "bell" => fx.push(Effect::Bell),
            "zap" => fx.push(Effect::Disconnect),
            _ if CLIENT_COMMANDS.contains(&name.as_str()) => fx.push(Effect::Client {
                args: args
                    .iter()
                    .map(|a| substitute_vars(a, &self.variables))
                    .collect(),
                name,
            }),
            _ if SETTING_COMMANDS.contains(&name.as_str()) => {}
            _ => {
                if self.warned.insert(name.clone()) {
                    fx.push(Effect::Warning(format!(
                        "команда #{name} пока не поддерживается (пропущена)"
                    )));
                }
            }
        }
    }

    // ───────────────────────────── таймеры ─────────────────────────────

    /// `#wait N` (десятые секунды) / `#wait cancel` / `#wait 0` — как в JMC.
    fn wait_command(&mut self, arg: &str, depth: u8, fx: &mut Vec<Effect>) {
        if !arg.is_empty() && "cancel".starts_with(&arg.to_lowercase()) {
            self.wait_queue.clear();
            self.wait_until = None;
            return;
        }
        if arg.starts_with(|c: char| c.is_ascii_digit()) {
            let tenths: u64 = arg
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap_or(0);
            self.wait_until = (tenths > 0).then(|| self.now + DECISECOND * tenths as u32);
        }
        if self.wait_until.is_none() {
            self.flush_wait_queue(depth, fx);
        }
    }

    /// Цикл JMC: `command` выполняется для счётчика от `from` до `to` с шагом `step`
    /// (`%0` — значение счётчика). Без задержки — сразу, с задержкой — по таймеру.
    #[allow(clippy::too_many_arguments)]
    fn start_cycle(
        &mut self,
        from: i64,
        to: i64,
        step: i64,
        delay_tenths: u64,
        command: &str,
        depth: u8,
        fx: &mut Vec<Effect>,
    ) {
        let step = step.max(1);
        if delay_tenths == 0 {
            let mut counter = from;
            for _ in 0..MAX_INSTANT_REPEATS {
                self.run(&command.replace("%0", &counter.to_string()), depth + 1, fx);
                if from <= to {
                    counter += step;
                    if counter > to {
                        return;
                    }
                } else {
                    counter -= step;
                    if counter < to {
                        return;
                    }
                }
            }
            fx.push(Effect::Warning(format!(
                "цикл остановлен после {MAX_INSTANT_REPEATS} повторов"
            )));
            return;
        }
        let period = DECISECOND * u32::try_from(delay_tenths).unwrap_or(u32::MAX);
        self.add_timer(
            Timer {
                kind: TimerKind::Cycle,
                command: command.to_string(),
                counter: from,
                end: to,
                step,
                period,
                // Как в JMC: первый раз — на ближайшей десятой доли секунды, дальше — раз в период.
                next: self.now + DECISECOND,
                active: true,
            },
            fx,
        );
    }

    fn add_timer(&mut self, timer: Timer, fx: &mut Vec<Effect>) {
        match (0..MAX_TIMERS).find(|id| !self.timers.contains_key(id)) {
            Some(id) => {
                self.timers.insert(id, timer);
            }
            None => fx.push(Effect::Warning(format!(
                "все {MAX_TIMERS} таймеров заняты — «{}» не запущен (#tmlist, #abort)",
                timer.command
            ))),
        }
    }

    /// `#abort {номер|all}` — убрать; `#break` — приостановить (повторно — убрать);
    /// `#resume` (`#pinch`) — продолжить приостановленный.
    fn timer_control(&mut self, command: &str, target: &str) {
        let target = target.trim();
        let ids: Vec<usize> = if target.eq_ignore_ascii_case("all") {
            self.timers.keys().copied().collect()
        } else {
            target.parse().ok().into_iter().collect()
        };
        for id in ids {
            let Some(timer) = self.timers.get_mut(&id) else {
                continue;
            };
            match command {
                "abort" => {
                    self.timers.remove(&id);
                }
                "break" if timer.active => timer.active = false,
                "break" => {
                    self.timers.remove(&id);
                }
                _ => timer.active = true,
            }
        }
    }

    fn list_timers(&self) -> String {
        let items = self
            .timers
            .iter()
            .map(|(id, t)| {
                let next = t.next.saturating_sub(self.now).as_secs_f64();
                let state = if t.active { "" } else { ", остановлен" };
                match t.kind {
                    TimerKind::Cycle => format!(
                        "#{id}: {{{}}} — осталось {} раз, каждые {:.1} с, следующий через {next:.1} с{state}",
                        t.command,
                        t.remaining(),
                        t.period.as_secs_f64()
                    ),
                    TimerKind::Delay => {
                        format!("#{id}: {{{}}} — через {next:.1} с{state}", t.command)
                    }
                }
            })
            .collect();
        listing("Таймеры", items)
    }

    // ───────────────────────────── скрипты ─────────────────────────────

    /// Выполняет JS-код `#script` и затем — то, что он попросил (`jmc.parse` и т.п.).
    fn run_script(&mut self, code: &str, event: Vec<String>, depth: u8, fx: &mut Vec<Effect>) {
        if self.scripts.engine.is_none() {
            match ScriptEngine::new() {
                Ok(engine) => self.scripts.engine = Some(engine),
                Err(e) => {
                    fx.push(Effect::Warning(format!(
                        "#script: не удалось запустить JavaScript: {e}"
                    )));
                    return;
                }
            }
        }
        let limit = self.scripts.time_limit;
        let connected = !self.disconnected;
        let Some(engine) = self.scripts.engine.as_mut() else {
            return;
        };
        let result = engine.run(code, &self.variables, event, connected, limit);
        for call in result.calls {
            self.script_call(call, depth, fx);
        }
        if result.dropped > 0 {
            fx.push(Effect::Warning(format!(
                "#script: слишком много вызовов jmc за один раз, {} пропущено",
                result.dropped
            )));
        }
        if let Some(error) = result.error {
            fx.push(Effect::Warning(format!("#script: {error}")));
        }
    }

    fn script_call(&mut self, call: ScriptCall, depth: u8, fx: &mut Vec<Effect>) {
        match call {
            // Через `run` — значит, работают алиасы, `#`-команды и защита от рекурсии.
            ScriptCall::Parse(command) => self.run(&command, depth + 1, fx),
            ScriptCall::Send(line) => fx.push(Effect::Send(line)),
            ScriptCall::Echo(text) => fx.push(Effect::Echo(text)),
            ScriptCall::Bell => fx.push(Effect::Bell),
            ScriptCall::SetVar(name, value) => self.set_variable(&name, &value, fx),
            ScriptCall::Connect(address, port) => fx.push(Effect::Client {
                name: "connect".to_string(),
                args: vec![address, port],
            }),
            ScriptCall::Disconnect => fx.push(Effect::Disconnect),
            ScriptCall::SetTimer { id, interval, pre } => {
                let tenths = |n: i64| DECISECOND * u32::try_from(n.max(0)).unwrap_or(u32::MAX);
                self.script_timers.insert(
                    id,
                    ScriptTimer {
                        id,
                        interval: tenths(interval).max(DECISECOND),
                        pre: tenths(pre),
                        last: self.now,
                        pre_done: false,
                    },
                );
            }
            ScriptCall::KillTimer(id) => {
                self.script_timers.remove(&id);
            }
            ScriptCall::RegisterHandler(event, code) => {
                let event = event.to_lowercase();
                if !matches!(event.as_str(), "timer" | "pretimer")
                    && self.warned.insert(format!("event {event}"))
                {
                    fx.push(Effect::Warning(format!(
                        "jmc.registerHandler: событие «{event}» пока не поддерживается \
                         (работают Timer и PreTimer)"
                    )));
                }
                if code.trim().is_empty() {
                    self.script_handlers.remove(&event);
                } else {
                    self.script_handlers.insert(event, code);
                }
            }
            ScriptCall::SetHotkey(key, command) => {
                self.set_hotkey(&key, &command, None);
                fx.push(Effect::Changed);
            }
            ScriptCall::Unsupported(method) => {
                if self.warned.insert(format!("jmc.{method}")) {
                    fx.push(Effect::Warning(format!(
                        "jmc.{method} пока не поддерживается (пропущено)"
                    )));
                }
            }
        }
    }

    fn set_variable(&mut self, name: &str, value: &str, fx: &mut Vec<Effect>) {
        if self.variables.get(name).map(String::as_str) != Some(value) {
            self.variables.insert(name.to_string(), value.to_string());
            fx.push(Effect::Changed);
        }
    }

    fn set_alias(&mut self, name: &str, body: &str, group: Option<&String>) {
        let alias = Alias {
            name: name.to_string(),
            body: body.to_string(),
            group: group_or_default(group),
        };
        match self.aliases.iter_mut().find(|a| a.name == name) {
            Some(existing) => *existing = alias,
            None => self.aliases.push(alias),
        }
    }

    fn set_action(
        &mut self,
        kind: &str,
        pattern: &str,
        body: &str,
        priority: i32,
        group: Option<&String>,
    ) {
        let action = Action {
            kind: kind.to_string(),
            pattern: Pattern::new(pattern),
            body: body.to_string(),
            priority,
            group: group_or_default(group),
        };
        match self
            .actions
            .iter_mut()
            .find(|a| a.pattern.source == pattern)
        {
            Some(existing) => *existing = action,
            None => self.actions.push(action),
        }
        // Меньшее число — раньше; при равенстве — в порядке добавления.
        self.actions.sort_by_key(|a| a.priority);
    }

    fn set_highlight(&mut self, spec: &str, pattern: &str, group: Option<&String>) -> bool {
        let Some(color) = parse_highlight(spec) else {
            return false;
        };
        let highlight = Highlight {
            color_spec: spec.to_string(),
            color,
            pattern: Pattern::new(pattern),
            group: group_or_default(group),
        };
        match self
            .highlights
            .iter_mut()
            .find(|h| h.pattern.source == pattern)
        {
            Some(existing) => *existing = highlight,
            None => self.highlights.push(highlight),
        }
        true
    }

    fn set_hotkey(&mut self, key: &str, body: &str, group: Option<&String>) {
        let hotkey = Hotkey {
            key: key.to_string(),
            body: body.to_string(),
            group: group_or_default(group),
        };
        match self
            .hotkeys
            .iter_mut()
            .find(|h| h.key.eq_ignore_ascii_case(key))
        {
            Some(existing) => *existing = hotkey,
            None => self.hotkeys.push(hotkey),
        }
    }

    fn set_substitution(
        &mut self,
        pattern: &str,
        replacement: Option<&String>,
        group: Option<&String>,
    ) {
        let sub = Substitution {
            pattern: Pattern::new(pattern),
            replacement: replacement.cloned(),
            group: group_or_default(group),
        };
        match self
            .substitutions
            .iter_mut()
            .find(|s| s.pattern.source == pattern)
        {
            Some(existing) => *existing = sub,
            None => self.substitutions.push(sub),
        }
    }

    /// Горячие клавиши включённых групп — для интерфейса.
    pub fn active_hotkeys(&self) -> Vec<Hotkey> {
        self.hotkeys
            .iter()
            .filter(|h| self.enabled(&h.group))
            .cloned()
            .collect()
    }

    // ───────────────────────────── строки от сервера ─────────────────────────────

    /// Склейка (`#merge`): строка от сервера → строки, которые пора обрабатывать дальше
    /// (`on_line`). Пока склейка не закончилась, возвращает пустой список.
    pub fn merge_line(&mut self, line: StyledLine) -> Vec<StyledLine> {
        if self.merges.is_empty() && !self.merger.is_open() {
            return vec![line];
        }
        let disabled = &self.disabled_groups;
        self.merger
            .line(&mut self.merges, line, &self.variables, |g| {
                !disabled.contains(g)
            })
    }

    /// Промпт или разрыв: незаконченная склейка показывается как есть.
    pub fn merge_flush(&mut self) -> Option<StyledLine> {
        self.merger.flush()
    }

    fn set_merge(
        &mut self,
        start: &str,
        end: &str,
        separator: Option<&String>,
        group: Option<&String>,
    ) {
        let group = group.cloned().unwrap_or_else(|| DEFAULT_GROUP.to_string());
        let separator = separator.cloned().unwrap_or_else(|| " ".to_string());
        let rule = MergeRule::new(start, end, &separator, &group);
        match self.merges.iter_mut().find(|m| m.start.source == start) {
            Some(existing) => *existing = rule,
            None => self.merges.push(rule),
        }
    }

    fn list_merges(&self) -> String {
        let items: Vec<String> = self
            .merges
            .iter()
            .map(|m| {
                format!(
                    "#merge {{{}}} {{{}}} {{{}}}",
                    m.start.source, m.end_source, m.separator
                )
            })
            .collect();
        if items.is_empty() {
            "Склеек нет. #merge {начало} {конец или число строк} {разделитель}".to_string()
        } else {
            items.join("\n")
        }
    }

    /// Обрабатывает строку от сервера: замены и скрытие, триггеры, подсветка.
    /// Возвращает `false`, если строку нужно скрыть. Триггеры срабатывают и на скрытых строках.
    pub fn on_line(&mut self, line: &mut StyledLine, fx: &mut Vec<Effect>) -> bool {
        let keep = self.apply_substitutions(line);
        self.fire_actions(&line.text(), fx);
        if keep {
            self.apply_highlights(line);
        }
        keep
    }

    /// Промпт: триггеры и подсветка, но не скрытие.
    pub fn on_prompt(&mut self, line: &mut StyledLine, fx: &mut Vec<Effect>) {
        self.fire_actions(&line.text(), fx);
        self.apply_highlights(line);
    }

    fn apply_substitutions(&mut self, line: &mut StyledLine) -> bool {
        let vars = &self.variables;
        for sub in self.substitutions.iter_mut() {
            if self.disabled_groups.contains(&sub.group) {
                continue;
            }
            let text = line.text();
            let Some(m) = sub.pattern.find(&text, vars) else {
                continue;
            };
            match &sub.replacement {
                None => return false,
                Some(replacement) => {
                    let replacement =
                        substitute_vars(&substitute_params(replacement, &m.params), vars);
                    line.replace(m.range, &replacement);
                }
            }
        }
        true
    }

    fn fire_actions(&mut self, text: &str, fx: &mut Vec<Effect>) {
        // Сначала собираем, что сработало, потом выполняем: тело триггера может менять сами триггеры.
        let vars = &self.variables;
        let mut fired = Vec::new();
        for action in self.actions.iter_mut() {
            if self.disabled_groups.contains(&action.group) {
                continue;
            }
            if let Some(m) = action.pattern.find(text, vars) {
                fired.push(substitute_params(&action.body, &m.params));
            }
        }
        for body in fired {
            self.run(&body, 1, fx);
        }
    }

    fn apply_highlights(&mut self, line: &mut StyledLine) {
        let vars = &self.variables;
        for h in self.highlights.iter_mut() {
            if self.disabled_groups.contains(&h.group) {
                continue;
            }
            // Подсвечиваем каждое вхождение, а не только первое.
            let text = line.text();
            let mut offset = 0;
            while offset <= text.len() {
                let Some(m) = h.pattern.find(&text[offset..], vars) else {
                    break;
                };
                let (start, end) = (offset + m.range.start, offset + m.range.end);
                let color = h.color;
                line.restyle(start..end, |style| color.apply(style));
                if end == start || h.pattern.source.starts_with('^') {
                    break;
                }
                offset = end;
            }
        }
    }

    // ───────────────────────────── импорт и запись ─────────────────────────────

    /// Загружает настройки из текста `.set` JMC (или из файла, записанного `to_script`).
    /// Ничего не выполняет: только определения алиасов, триггеров, переменных и т.п.
    pub fn import(&mut self, text: &str) -> ImportReport {
        let mut report = ImportReport::default();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            let Some((name, args)) = parse_hash_command(line) else {
                *report.unsupported.entry("(не команда)".into()).or_default() += 1;
                self.preserved.push(line.to_string());
                continue;
            };
            let name = canonical_command(&name);
            let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
            match name.as_str() {
                "ticksize" if arg(0).trim().parse::<u64>().is_ok_and(|n| n > 0) => {
                    let secs = arg(0).trim().parse().unwrap_or(60);
                    self.ticker.size = Duration::from_secs(secs);
                    self.ticker.reset(self.now);
                }
                "tickon" => self.ticker.enabled = true,
                "tickoff" => self.ticker.enabled = false,
                "alias" | "ali" | "al" if args.len() >= 2 => {
                    self.set_alias(&args[0], &args[1], args.get(2));
                    report.aliases += 1;
                }
                "action" | "act" | "ac" => {
                    let (kind, rest) = split_action_kind(&args);
                    if rest.len() >= 2 {
                        let priority = rest
                            .get(2)
                            .and_then(|p| p.trim().parse().ok())
                            .unwrap_or(DEFAULT_PRIORITY);
                        self.set_action(&kind, &rest[0], &rest[1], priority, rest.get(3));
                        report.actions += 1;
                        if is_login_pattern(&rest[0]) {
                            report.login_actions.push(rest[0].clone());
                        }
                    }
                }
                "variable" | "var" if !args.is_empty() => {
                    self.variables.insert(arg(0), args[1..].join(" "));
                    report.variables += 1;
                }
                "highlight" | "high" if args.len() >= 2 => {
                    if self.set_highlight(&args[0], &args[1], args.get(2)) {
                        report.highlights += 1;
                    } else {
                        *report
                            .unsupported
                            .entry("highlight (цвет)".into())
                            .or_default() += 1;
                        self.preserved.push(line.to_string());
                    }
                }
                "hot" | "hotkey" if args.len() >= 2 => {
                    self.set_hotkey(&args[0], &args[1], args.get(2));
                    report.hotkeys += 1;
                }
                "gag" if !args.is_empty() => {
                    self.set_substitution(&args[0], None, args.get(1));
                    report.substitutions += 1;
                }
                "substitute" | "sub" | "subs" if args.len() >= 2 => {
                    self.set_substitution(&args[0], Some(&args[1]), args.get(2));
                    report.substitutions += 1;
                }
                "merge" if args.len() >= 2 => {
                    self.set_merge(&args[0], &args[1], args.get(2), args.get(3));
                    report.substitutions += 1;
                }
                "group" if matches!(arg(0).to_lowercase().as_str(), "disable" | "off") => {
                    self.disabled_groups.insert(arg(1));
                }
                "group" if matches!(arg(0).to_lowercase().as_str(), "enable" | "on") => {
                    self.disabled_groups.remove(&arg(1));
                }
                _ => {
                    if !SETTING_COMMANDS.contains(&name.as_str()) && name != "group" {
                        *report.unsupported.entry(format!("#{name}")).or_default() += 1;
                    }
                    if !self.preserved.iter().any(|p| p == line) {
                        self.preserved.push(line.to_string());
                    }
                }
            }
        }
        // Команды внутри алиасов и триггеров, которые пока не выполняются.
        let bodies = self
            .aliases
            .iter()
            .map(|a| a.body.as_str())
            .chain(self.actions.iter().map(|a| a.body.as_str()))
            .chain(self.hotkeys.iter().map(|h| h.body.as_str()));
        for body in bodies {
            for name in inner_unsupported(body) {
                *report
                    .unsupported
                    .entry(format!("#{name} (внутри)"))
                    .or_default() += 1;
            }
        }
        report
    }

    /// Все настройки в виде текста (формат JMC, UTF-8). Обратная операция — `import`.
    pub fn to_script(&self) -> String {
        let mut out = String::new();
        let mut line = |s: String| {
            out.push_str(&s);
            out.push('\n');
        };
        if self.ticker.size != DEFAULT_TICK_SIZE {
            line(format!("#ticksize {}", self.ticker.size.as_secs()));
        }
        if self.ticker.enabled {
            line("#tickon".to_string());
        }
        for (name, value) in &self.variables {
            line(format!("#variable {} {}", braced(name), braced(value)));
        }
        for a in &self.aliases {
            line(format!(
                "#alias {} {} {}",
                braced(&a.name),
                braced(&a.body),
                braced(&a.group)
            ));
        }
        for a in &self.actions {
            line(format!(
                "#action {} {} {} {} {}",
                a.kind,
                braced(&a.pattern.source),
                braced(&a.body),
                braced(&a.priority.to_string()),
                braced(&a.group)
            ));
        }
        for h in &self.highlights {
            line(format!(
                "#highlight {} {} {}",
                braced(&h.color_spec),
                braced(&h.pattern.source),
                braced(&h.group)
            ));
        }
        for h in &self.hotkeys {
            line(format!(
                "#hot {} {} {}",
                braced(&h.key),
                braced(&h.body),
                braced(&h.group)
            ));
        }
        for s in &self.substitutions {
            match &s.replacement {
                None => line(format!(
                    "#gag {} {}",
                    braced(&s.pattern.source),
                    braced(&s.group)
                )),
                Some(r) => line(format!(
                    "#substitute {} {} {}",
                    braced(&s.pattern.source),
                    braced(r),
                    braced(&s.group)
                )),
            }
        }
        for m in &self.merges {
            line(format!(
                "#merge {} {} {} {}",
                braced(&m.start.source),
                braced(&m.end_source),
                braced(&m.separator),
                braced(&m.group)
            ));
        }
        for g in &self.disabled_groups {
            line(format!("#group disable {}", braced(g)));
        }
        for p in &self.preserved {
            line(p.clone());
        }
        out
    }

    // ───────────────────────────── списки для #alias и т.п. ─────────────────────────────

    fn list_aliases(&self, name: Option<&str>) -> String {
        let items: Vec<String> = self
            .aliases
            .iter()
            .filter(|a| name.is_none_or(|n| a.name == n))
            .map(|a| format!("#alias {{{}}} {{{}}}", a.name, a.body))
            .collect();
        listing("Алиасы", items)
    }

    fn list_actions(&self, pattern: Option<&str>) -> String {
        let items: Vec<String> = self
            .actions
            .iter()
            .filter(|a| pattern.is_none_or(|p| a.pattern.source.contains(p)))
            .map(|a| {
                format!(
                    "#action {{{}}} {{{}}} {{{}}}",
                    a.pattern.source, a.body, a.priority
                )
            })
            .collect();
        listing("Триггеры", items)
    }

    fn list_variables(&self) -> String {
        let items = self
            .variables
            .iter()
            .map(|(k, v)| format!("${k} = {v}"))
            .collect();
        listing("Переменные", items)
    }

    fn list_highlights(&self) -> String {
        let items = self
            .highlights
            .iter()
            .map(|h| format!("#highlight {{{}}} {{{}}}", h.color_spec, h.pattern.source))
            .collect();
        listing("Подсветка", items)
    }

    fn list_hotkeys(&self) -> String {
        let items = self
            .hotkeys
            .iter()
            .map(|h| format!("#hot {{{}}} {{{}}}", h.key, h.body))
            .collect();
        listing("Горячие клавиши", items)
    }

    fn list_substitutions(&self) -> String {
        let items = self
            .substitutions
            .iter()
            .map(|s| match &s.replacement {
                None => format!("#gag {{{}}}", s.pattern.source),
                Some(r) => format!("#substitute {{{}}} {{{r}}}", s.pattern.source),
            })
            .collect();
        listing("Замены", items)
    }
}

fn listing(title: &str, items: Vec<String>) -> String {
    if items.is_empty() {
        format!("{title}: нет.")
    } else {
        format!("{title} ({}):\n{}", items.len(), items.join("\n"))
    }
}

fn group_or_default(group: Option<&String>) -> String {
    match group {
        Some(g) if !g.trim().is_empty() => g.trim().to_string(),
        _ => DEFAULT_GROUP.to_string(),
    }
}

/// Похоже на триггер входа: выбор кодировки, имя, пароль.
fn is_login_pattern(pattern: &str) -> bool {
    let p = pattern.to_lowercase();
    ["select one", "введите имя", "пароль", "имя и пароль"]
        .iter()
        .any(|k| p.contains(k))
}

/// `#action TEXT {…} …` — первый аргумент может быть типом проверки.
fn split_action_kind(args: &[String]) -> (String, &[String]) {
    match args.first() {
        Some(k) if ["text", "raw", "color"].contains(&k.to_lowercase().as_str()) => {
            (k.to_uppercase(), &args[1..])
        }
        _ => ("TEXT".to_string(), args),
    }
}

/// Команды, которые выполняет сам движок (полные имена и принятые у нас сокращения).
const SUPPORTED: &[&str] = &[
    "merge",
    "unmerge",
    "alias",
    "ali",
    "al",
    "unalias",
    "unali",
    "action",
    "act",
    "ac",
    "unaction",
    "unact",
    "unac",
    "variable",
    "var",
    "unvariable",
    "unvar",
    "highlight",
    "high",
    "unhighlight",
    "unhigh",
    "hot",
    "hotkey",
    "unhot",
    "unhotkey",
    "gag",
    "ungag",
    "substitute",
    "sub",
    "subs",
    "unsubstitute",
    "unsub",
    "unsubs",
    "if",
    "math",
    "group",
    "output",
    "showme",
    "woutput",
    "bell",
    "zap",
    // скрипты
    "scriptlet",
    "script",
    // время
    "tick",
    "tickon",
    "tickoff",
    "tickset",
    "ticksize",
    "wait",
    "wt",
    "delay",
    "loop",
    "break",
    "abort",
    "resume",
    "pinch",
    "tmlist",
    "llist",
];

/// Все команды JMC 3.6 в порядке из его `cmds.h`. В JMC команду можно сокращать:
/// сокращение означает **первую** команду списка, которая с него начинается
/// (`#con` → `#connect`, `#script` → `#scriptlet`, `#t` → `#tolower`).
const JMC_COMMANDS: &[&str] = &[
    "variable",
    "action",
    "alias",
    "read",
    "if",
    "group",
    "loop",
    "tolower",
    "toupper",
    "antisubstitute",
    "drop",
    "nodrop",
    "cr",
    "echo",
    "highlight",
    "ignore",
    "info",
    "killall",
    "kickall",
    "log",
    "map",
    "mark",
    "math",
    "message",
    "output",
    "path",
    "pathdir",
    "presub",
    "return",
    "scriptlet",
    "savepath",
    "showme",
    "speedwalk",
    "textin",
    "substitute",
    "gag",
    "tick",
    "tickon",
    "tickoff",
    "tickset",
    "ticksize",
    "togglesubs",
    "unaction",
    "unalias",
    "unantisubstitute",
    "unpath",
    "unvar",
    "write",
    "zap",
    "hotkey",
    "unhotkey",
    "connect",
    "bell",
    "char",
    "status",
    "tabadd",
    "tabdel",
    "multiaction",
    "multisubstitute",
    "multihighlight",
    "verbatim",
    "nope",
    "quit",
    "hidewindow",
    "restorewindow",
    "tray",
    "reloadscripts",
    "use",
    "unuse",
    "systemexec",
    "ps",
    "tslist",
    "terminate",
    "tskill",
    "play",
    "woutput",
    "wdock",
    "wshow",
    "wpos",
    "wlog",
    "wname",
    "logadd",
    "logpass",
    "flash",
    "next",
    "daa",
    "whisper",
    "hide",
    "race",
    "spit",
    "stick",
    "lick",
    "feed",
    "colon",
    "wait",
    "wt",
    "break",
    "abort",
    "pinch",
    "resume",
    "tmlist",
    "llist",
    "unsubstitute",
    "comment",
    "sos",
    "grab",
    "help",
    "clean",
    "prefix",
    "winamp",
    "wamp",
    "autoreconnect",
    "run",
    "clear",
    "wclear",
    "loopback",
    "broadcast",
    "strcmp",
    "match",
    "srandom",
    "random",
    "sync",
    "proxy",
    "telnet",
    "promptend",
    "wsize",
    "replace",
    "secure",
    "codepage",
    "bar",
    "oob",
    "mapper",
];

/// Имя, которое движок знает как есть (без разворачивания сокращений).
fn is_known_command(name: &str) -> bool {
    SUPPORTED.contains(&name) || SETTING_COMMANDS.contains(&name) || CLIENT_COMMANDS.contains(&name)
}

/// Полное имя команды: известные имена — как есть, иначе сокращение по правилам JMC.
/// `#5:10` (цикл) и неизвестное — как есть.
fn canonical_command(name: &str) -> String {
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) || is_known_command(name) {
        return name.to_string();
    }
    JMC_COMMANDS
        .iter()
        .find(|full| full.starts_with(name))
        .map(|full| full.to_string())
        .unwrap_or_else(|| name.to_string())
}

/// Имена `#`-команд внутри тела, которые клиент пока не выполняет.
fn inner_unsupported(body: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = body;
    while let Some(i) = rest.find('#') {
        rest = &rest[i + 1..];
        let name: String = rest.chars().take_while(|c| c.is_alphabetic()).collect();
        let lower = name.to_lowercase();
        if !lower.is_empty() && !is_known_command(&canonical_command(&lower)) {
            found.push(lower);
        }
    }
    found
}

/// Несколько `Changed` подряд не нужны — оставляем один в конце.
fn dedup_changed(fx: Vec<Effect>) -> Vec<Effect> {
    let changed = fx.contains(&Effect::Changed);
    let mut out: Vec<Effect> = fx.into_iter().filter(|e| *e != Effect::Changed).collect();
    if changed {
        out.push(Effect::Changed);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sends(fx: &[Effect]) -> Vec<String> {
        fx.iter()
            .filter_map(|e| match e {
                Effect::Send(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    fn line_fx(a: &mut Automation, text: &str) -> (bool, StyledLine, Vec<Effect>) {
        let mut line = StyledLine::plain(text);
        let mut fx = Vec::new();
        let keep = a.on_line(&mut line, &mut fx);
        (keep, line, fx)
    }

    #[test]
    fn алиас_с_параметрами_и_переменными() {
        let mut a = Automation::new();
        a.user_input("#alias {вх} {колд !вихрь! %1}");
        a.user_input("#var target тролль");
        assert_eq!(sends(&a.user_input("вх $target")), ["колд !вихрь! тролль"]);
        assert_eq!(sends(&a.user_input("вх")), ["колд !вихрь!"]);
    }

    #[test]
    fn алиас_из_set_файла_колдуна() {
        let mut a = Automation::new();
        a.import(
            "#alias {ц1} {#var target %0;гг Моя цель: %0;#act {^Вы поплелись}{$spell $target}} {default}\n\
             #variable {spell} {ка}",
        );
        let fx = a.user_input("ц1 огр");
        assert_eq!(sends(&fx), ["гг Моя цель: огр"]);
        assert_eq!(a.variables["target"], "огр");
        // Триггер, созданный алиасом, подставляет переменные в момент срабатывания.
        let (_, _, fx) = line_fx(&mut a, "Вы поплелись на север.");
        assert_eq!(sends(&fx), ["ка огр"]);
    }

    #[test]
    fn рекурсивный_алиас_не_вешает_клиент() {
        let mut a = Automation::new();
        a.user_input("#alias {а} {а}");
        let fx = a.user_input("а");
        assert!(fx.iter().any(|e| matches!(e, Effect::Warning(_))));
    }

    #[test]
    fn триггеры_и_приоритет() {
        let mut a = Automation::new();
        a.user_input("#action {Здесь лежит %1 %2 кун.} {вз все.кун} {5}");
        a.user_input("#action TEXT {/^(\\w+) сообщил группе : '!([^']+)'/} {%1} {3}");
        let (_, _, fx) = line_fx(&mut a, "Здесь лежит 15 больших кун.");
        assert_eq!(sends(&fx), ["вз все.кун"]);
        let (_, _, fx) = line_fx(&mut a, "Виорий сообщил группе : '!встать'");
        assert_eq!(sends(&fx), ["встать"]);
    }

    #[test]
    fn условие_и_арифметика() {
        let mut a = Automation::new();
        a.import(
            "#variable {natisk} {1}\n#variable {tank} {Вася}\n\
             #alias {нт} {#if {$natisk == 1} {гг $tank натиск вверх} {гг $tank вверх}}",
        );
        assert_eq!(sends(&a.user_input("нт")), ["гг Вася натиск вверх"]);
        a.user_input("#var natisk 0");
        assert_eq!(sends(&a.user_input("нт")), ["гг Вася вверх"]);
        a.user_input("#math {n} {$natisk + 41}");
        assert_eq!(a.variables["n"], "41");
    }

    #[test]
    fn скрытие_замена_подсветка() {
        let mut a = Automation::new();
        a.user_input("#gag {^Вы проголодались.}");
        a.user_input("#sub {%1 сильно ударил вас} {%1 ⚔ вас}");
        a.user_input("#highlight {light red} {$tank}");
        a.user_input("#var tank Вася");

        let (keep, _, _) = line_fx(&mut a, "Вы проголодались.");
        assert!(!keep);

        let (keep, line, _) = line_fx(&mut a, "Тролль сильно ударил вас.");
        assert!(keep);
        assert_eq!(line.text(), "Тролль ⚔ вас.");

        let (_, line, _) = line_fx(&mut a, "Вася и Вася");
        let red: Vec<&str> = line
            .spans
            .iter()
            .filter(|s| s.style.fg == klubok_proto::Color::Named(9))
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(red, ["Вася", "Вася"]);
    }

    #[test]
    fn группы() {
        let mut a = Automation::new();
        a.user_input("#action {бой} {спасти} {5} {бой}");
        a.user_input("#group disable бой");
        assert!(sends(&line_fx(&mut a, "бой").2).is_empty());
        a.user_input("#group enable бой");
        assert_eq!(sends(&line_fx(&mut a, "бой").2), ["спасти"]);
    }

    #[test]
    fn импорт_и_запись_туда_обратно() {
        let set = "#action TEXT {^Действие клича битвы закончилось.} {кли бит} {5} {default}\n\
                   #alias {рекол} {вз возвр $con; вз все} {default}\n\
                   #variable {ch112} {щит}\n\
                   #hot {Alt+NUM9} {гг вверх} {default}\n\
                   #highlight {white,b black} {$tank} {default}\n\
                   #message uses OFF\n\
                   #tickset\n";
        let mut a = Automation::new();
        let report = a.import(set);
        assert_eq!(
            (
                report.actions,
                report.aliases,
                report.variables,
                report.hotkeys,
                report.highlights
            ),
            (1, 1, 1, 1, 1)
        );
        assert_eq!(report.unsupported.get("#tickset"), Some(&1));
        assert!(report.login_actions.is_empty());
        let login =
            a.import("#action TEXT {^Имя и пароль через пробел : } {$name секрет} {5} {default}");
        assert_eq!(login.login_actions, ["^Имя и пароль через пробел : "]);
        a.user_input("#unaction {^Имя и пароль через пробел : }");
        assert!(!report.unsupported.contains_key("#message"));

        let script = a.to_script();
        let mut b = Automation::new();
        b.import(&script);
        assert_eq!(b.to_script(), script);
        assert!(script.contains("#message uses OFF"));
    }

    #[test]
    fn неподдерживаемое_внутри_тел() {
        let mut a = Automation::new();
        let report = a.import(
            "#alias {x} {#tolower a b; #script jmc.parse(\"см\"); #con localhost 2000} {default}",
        );
        assert_eq!(report.unsupported.get("#tolower (внутри)"), Some(&1));
        assert!(!report.unsupported.contains_key("#script (внутри)"));
        assert!(!report.unsupported.contains_key("#con (внутри)"));
        let fx = a.user_input("x");
        assert_eq!(sends(&fx), ["см"]);
        assert!(fx.contains(&Effect::Client {
            name: "connect".into(),
            args: vec!["localhost".into(), "2000".into()],
        }));
        assert!(
            fx.iter()
                .any(|e| matches!(e, Effect::Warning(w) if w.contains("#tolower")))
        );
    }

    fn echoes(fx: &[Effect]) -> Vec<String> {
        fx.iter()
            .filter_map(|e| match e {
                Effect::Echo(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    fn warnings(fx: &[Effect]) -> Vec<String> {
        fx.iter()
            .filter_map(|e| match e {
                Effect::Warning(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    // ───────────── скрипты ─────────────

    #[test]
    fn скрипт_if_else_на_переменных() {
        let mut a = Automation::new();
        a.import(
            "#variable {ch11} {щит}\n#variable {ch12} {0}\n\
             #alias {одеть} {#script if (\"$ch11\" != \"0\") {jmc.parse(\"оде $ch11; дер $ch11\")} else {jmc.showme(\"нет\")}; \
             #script if (\"$ch12\" != \"0\") {jmc.parse(\"оде $ch12\")} else {jmc.showme(\"пусто\")}} {default}",
        );
        let fx = a.user_input("одеть");
        assert_eq!(sends(&fx), ["оде щит", "дер щит"]);
        assert_eq!(echoes(&fx), ["пусто"]);
        assert!(warnings(&fx).is_empty(), "{fx:?}");
    }

    #[test]
    fn jmc_parse_с_алиасом_и_командами() {
        let mut a = Automation::new();
        a.user_input("#alias {вз} {взять %1 сумка}");
        let fx = a.user_input("#script {jmc.parse('вз меч; #var x 5'); jmc.send('вз как есть')}");
        assert_eq!(sends(&fx), ["взять меч сумка", "вз как есть"]);
        assert_eq!(a.variables["x"], "5");
        assert_eq!(fx.last(), Some(&Effect::Changed));
        // jmc.getVar видит переменные клиента, jmc.setVar их меняет.
        let fx = a.user_input(
            "#script {jmc.setVar('y', jmc.getVar('x') * 2); jmc.showme(jmc.GetVar('y'))}",
        );
        assert_eq!(echoes(&fx), ["10"]);
        assert_eq!(a.variables["y"], "10");
    }

    #[test]
    fn глобальные_переменные_js_живут_между_вызовами() {
        let mut a = Automation::new();
        a.user_input("#script {n = 1; var m = 10}");
        let fx = a.user_input("#script {n = n + 1; jmc.showme('n=' + n + ', m=' + m)}");
        assert_eq!(echoes(&fx), ["n=2, m=10"]);
        // Клон получает свой, новый движок.
        let mut b = a.clone();
        let fx = b.user_input("#script jmc.showme(typeof n)");
        assert_eq!(echoes(&fx), ["undefined"]);
    }

    #[test]
    fn вечный_цикл_прерывается() {
        let mut a = Automation::new();
        a.set_script_time_limit(ms(50));
        let fx = a.user_input("#script {jmc.send('до'); while (true) { jmc.parse('спам') }}");
        let w = warnings(&fx);
        assert!(w.iter().any(|w| w.contains("прерван")), "{w:?}");
        assert!(w.iter().any(|w| w.contains("слишком много")), "{w:?}");
        assert_eq!(sends(&fx)[0], "до");
        // Движок после прерывания жив.
        assert_eq!(sends(&a.user_input("#script jmc.send('жив')")), ["жив"]);
    }

    #[test]
    fn ошибка_синтаксиса_предупреждение() {
        let mut a = Automation::new();
        let fx = a.user_input("#script if ( {jmc.send('x')}; см");
        assert_eq!(sends(&fx), ["см"]);
        let w = warnings(&fx);
        assert_eq!(w.len(), 1);
        assert!(w[0].starts_with("#script: SyntaxError"), "{w:?}");
        let fx = a.user_input("#script нет_такого()");
        assert!(warnings(&fx)[0].contains("ReferenceError"));
    }

    #[test]
    fn рекурсия_через_скрипт_останавливается() {
        let mut a = Automation::new();
        a.user_input("#alias {р} {#script jmc.parse('р')}");
        let fx = a.user_input("р");
        assert!(
            warnings(&fx)
                .iter()
                .any(|w| w.contains("слишком глубокая вложенность"))
        );
    }

    #[test]
    fn таймер_скрипта() {
        let mut a = Automation::new();
        a.set_now(ms(1000));
        a.user_input(
            "#script {jmc.registerHandler('Timer', \"jmc.send('таймер ' + jmc.event(0))\"); jmc.setTimer(7, 20)}",
        );
        assert_eq!(a.next_deadline(), Some(ms(3000)));
        assert!(a.tick(ms(2999)).is_empty());
        assert_eq!(sends(&a.tick(ms(3000))), ["таймер 7"]);
        a.user_input("#script jmc.killTimer(7)");
        assert_eq!(a.next_deadline(), None);
    }

    // ───────────── время ─────────────

    #[test]
    fn цикл_с_задержкой() {
        let mut a = Automation::new();
        a.set_now(ms(0));
        assert!(sends(&a.user_input("#3:10 {гг %0}")).is_empty());
        assert_eq!(a.next_deadline(), Some(ms(100)));
        assert_eq!(sends(&a.tick(ms(100))), ["гг 1"]);
        assert_eq!(a.next_deadline(), Some(ms(1100)));
        assert!(a.tick(ms(1000)).is_empty());
        assert_eq!(sends(&a.tick(ms(1100))), ["гг 2"]);
        assert_eq!(sends(&a.tick(ms(2100))), ["гг 3"]);
        assert_eq!(a.next_deadline(), None);
    }

    #[test]
    fn цикл_без_задержки_и_loop() {
        let mut a = Automation::new();
        assert_eq!(sends(&a.user_input("#3 {см}")), ["см", "см", "см"]);
        assert_eq!(
            sends(&a.user_input("#loop {3,1} {гг %0}")),
            ["гг 3", "гг 2", "гг 1"]
        );
        a.set_now(ms(0));
        a.user_input("#loop {1,4|2:5} {гг %0}");
        assert_eq!(sends(&a.tick(ms(100))), ["гг 1"]);
        assert_eq!(sends(&a.tick(ms(600))), ["гг 3"]);
        assert_eq!(a.next_deadline(), None);
    }

    #[test]
    fn abort_break_resume() {
        let mut a = Automation::new();
        a.set_now(ms(0));
        // Как у владельца: длинный цикл, который останавливает триггер.
        a.user_input("#3000:15 {г да}");
        a.user_input("#action {^Вы убили} {#abort all; #bell}");
        a.user_input("#5:10 {см}");
        let list = echoes(&a.user_input("#tmlist")).join("\n");
        assert!(
            list.contains("#0: {г да}") && list.contains("#1: {см}"),
            "{list}"
        );
        assert_eq!(sends(&a.tick(ms(100))), ["г да", "см"]);

        a.user_input("#break 1");
        assert_eq!(sends(&a.tick(ms(1600))), ["г да"]);
        a.user_input("#resume 1");
        assert_eq!(sends(&a.tick(ms(1700))), ["см"]);

        let (_, _, fx) = line_fx(&mut a, "Вы убили бобра.");
        assert!(fx.contains(&Effect::Bell));
        assert_eq!(a.next_deadline(), None);
        assert!(a.tick(ms(5000)).is_empty());
    }

    #[test]
    fn wait_и_delay() {
        let mut a = Automation::new();
        a.set_now(ms(0));
        let fx = a.user_input("#wait 20; север; #output привет; юг");
        assert!(sends(&fx).is_empty());
        assert_eq!(echoes(&fx), ["привет"]);
        assert_eq!(a.next_deadline(), Some(ms(2000)));
        assert!(a.tick(ms(1999)).is_empty());
        assert_eq!(sends(&a.tick(ms(2000))), ["север", "юг"]);

        a.user_input("#wt 5; запад");
        a.user_input("#wait cancel");
        assert_eq!(a.next_deadline(), None);
        assert_eq!(sends(&a.user_input("восток")), ["восток"]);

        a.user_input("#delay {1,5} {см}");
        assert_eq!(a.next_deadline(), Some(ms(3500))); // «сейчас» — 2 с после tick
        assert_eq!(sends(&a.tick(ms(3500))), ["см"]);
        assert_eq!(a.next_deadline(), None);
    }

    #[test]
    fn тикер_и_триггер_на_тик() {
        let mut a = Automation::new();
        a.set_now(ms(0));
        a.user_input("#action {^#ТИК} {гг тик}");
        a.user_input("#action {Минул час.} {#tickset}");
        assert_eq!(a.next_deadline(), None); // тикер выключен
        assert_eq!(echoes(&a.user_input("#tickon")), ["#Тикер ВКЛЮЧЕН."]);

        a.set_now(ms(3000));
        line_fx(&mut a, "Минул час.");
        assert_eq!(a.next_deadline(), Some(ms(53_000)));
        let fx = a.tick(ms(53_000));
        assert_eq!(echoes(&fx), ["#10 СЕКУНД ДО ТИКА"]);
        a.set_now(ms(60_000));
        assert_eq!(echoes(&a.user_input("#tick")), ["#3 секунд до тика."]);
        let fx = a.tick(ms(63_000));
        assert_eq!(echoes(&fx), ["#ТИК"]);
        assert_eq!(sends(&fx), ["гг тик"]);

        a.set_connected(false);
        assert_eq!(a.next_deadline(), None);
        a.set_connected(true);
        assert_eq!(echoes(&a.user_input("#ticksize 30")).len(), 1);
        // «сейчас» — 63 с: тик в 93 с, предупреждение в 83 с.
        assert_eq!(a.next_deadline(), Some(ms(83_000)));
    }

    #[test]
    fn тикер_в_настройках() {
        let mut a = Automation::new();
        a.import("#ticksize 30\n#tickon\n#tickset");
        assert_eq!(a.ticker.size, Duration::from_secs(30));
        assert!(a.ticker.enabled);
        let script = a.to_script();
        assert!(script.contains("#ticksize 30\n#tickon\n"), "{script}");
        let mut b = Automation::new();
        b.import("#ticksize 60");
        assert!(!b.to_script().contains("ticksize"));
    }

    #[test]
    fn сокращения_команд_как_в_jmc() {
        assert_eq!(canonical_command("con"), "connect");
        assert_eq!(canonical_command("script"), "script");
        assert_eq!(canonical_command("scr"), "scriptlet");
        assert_eq!(canonical_command("ab"), "abort");
        assert_eq!(canonical_command("ali"), "ali");
        assert_eq!(canonical_command("3000:15"), "3000:15");
        assert_eq!(canonical_command("нет"), "нет");
    }
}
