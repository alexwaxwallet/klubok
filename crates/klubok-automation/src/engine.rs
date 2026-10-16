//! Интерпретатор команд в стиле JMC и обработка строк от сервера.
//!
//! `Automation` хранит алиасы, триггеры, переменные и т.д. и ничего не знает о сети:
//! на вход — текст команд или строка от сервера, на выход — список `Effect`
//! (что отправить, что показать). Сессия решает, что с этим делать.

use std::collections::{BTreeMap, BTreeSet};

use klubok_proto::StyledLine;

use crate::color::{HighlightColor, parse_highlight};
use crate::expr;
use crate::pattern::Pattern;
use crate::script::{
    braced, parse_hash_command, split_commands, substitute_params, substitute_vars,
};

/// Насколько глубоко алиасы и триггеры могут вызывать друг друга (защита от зацикливания).
const MAX_DEPTH: u8 = 16;
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
}

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
    /// Выключенные группы (`#group disable имя`).
    pub disabled_groups: BTreeSet<String>,
    /// Строки настроек, которые клиент пока не понимает, — сохраняются при записи.
    pub preserved: Vec<String>,
    /// О каких неподдерживаемых командах уже предупредили (чтобы не спамить).
    warned: BTreeSet<String>,
}

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
    "ticksize",
    "secure",
];

impl Automation {
    pub fn new() -> Automation {
        Automation::default()
    }

    fn enabled(&self, group: &str) -> bool {
        !self.disabled_groups.contains(group)
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
        let arg = |i: usize| args.get(i).cloned().unwrap_or_default();

        match name.as_str() {
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
            let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
            match name.as_str() {
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

/// Имена `#`-команд внутри тела, которые клиент пока не выполняет.
fn inner_unsupported(body: &str) -> Vec<String> {
    const SUPPORTED: &[&str] = &[
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
    ];
    let mut found = Vec::new();
    let mut rest = body;
    while let Some(i) = rest.find('#') {
        rest = &rest[i + 1..];
        let name: String = rest.chars().take_while(|c| c.is_alphabetic()).collect();
        let lower = name.to_lowercase();
        if !lower.is_empty()
            && !SUPPORTED.contains(&lower.as_str())
            && !SETTING_COMMANDS.contains(&lower.as_str())
        {
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
        let report = a.import("#alias {x} {#script if (1) {jmc.parse(\"см\")}; см} {default}");
        assert_eq!(report.unsupported.get("#script (внутри)"), Some(&1));
        let fx = a.user_input("x");
        assert_eq!(sends(&fx), ["см"]);
        assert!(
            fx.iter()
                .any(|e| matches!(e, Effect::Warning(w) if w.contains("#script")))
        );
    }
}
