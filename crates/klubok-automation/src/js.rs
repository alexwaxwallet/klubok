//! Скрипты `#script` — в JMC это JScript, у нас — JavaScript на движке QuickJS
//! (крейт `rquickjs`, сам движок написан на C и собирается вместе с программой).
//!
//! Как это устроено:
//! * Один движок на `Automation`, создаётся лениво при первом `#script`. Глобальные
//!   переменные JS живут между вызовами — как в JMC (`#script x = 1`, потом `#script jmc.showme(x)`).
//! * Объект `jmc` — методы из JMC (`jmc.parse`, `jmc.send`, `jmc.showme`, `jmc.getVar`…).
//!   Имена без учёта регистра, как в JScript/COM: `jmc.parse`, `jmc.Parse`, `jmc.showMe`.
//! * Методы `jmc` не выполняют команды сразу, а **записывают просьбу** ([`ScriptCall`]).
//!   После скрипта движок автоматизации выполняет их по порядку. Так проще: скрипту не нужен
//!   доступ к `Automation`, пока тот занят выполнением скрипта. Разница с JMC одна:
//!   `jmc.getVar` внутри того же скрипта не видит переменные, заданные через `jmc.parse("#var …")`
//!   (а заданные через `jmc.setVar` — видит).
//! * Защита от вечного цикла: ограничение по времени (прерывание QuickJS), по памяти
//!   и по числу просьб за один запуск.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rquickjs::context::EvalOptions;
use rquickjs::function::IntoJsFunc;
use rquickjs::prelude::{Coerced, Opt};
use rquickjs::{Context, Ctx, Function, Object, Runtime, Value};

/// Сколько по умолчанию может выполняться один `#script`.
pub const DEFAULT_TIME_LIMIT: Duration = Duration::from_secs(1);
/// Больше просьб за один запуск не принимаем (вечный цикл с `jmc.parse` внутри).
const MAX_CALLS: usize = 1000;
const MEMORY_LIMIT: usize = 32 * 1024 * 1024;
const STACK_LIMIT: usize = 1024 * 1024;

/// Что скрипт попросил сделать. Выполняется движком автоматизации после скрипта, по порядку.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptCall {
    /// `jmc.parse(команды)` — как будто игрок ввёл команды (алиасы, `#`-команды).
    Parse(String),
    /// `jmc.send(строка)` — отправить серверу как есть, без алиасов.
    Send(String),
    /// `jmc.showme(текст)`, `jmc.output(текст)` — показать сообщение.
    Echo(String),
    /// `jmc.beep()`.
    Bell,
    /// `jmc.setVar(имя, значение)`.
    SetVar(String, String),
    /// `jmc.connect(адрес, порт)`.
    Connect(String, String),
    /// `jmc.disconnect()`.
    Disconnect,
    /// `jmc.setTimer(id, интервал, предупреждение)` — интервалы в десятых долях секунды.
    SetTimer { id: i64, interval: i64, pre: i64 },
    /// `jmc.killTimer(id)`.
    KillTimer(i64),
    /// `jmc.registerHandler(событие, код)`.
    RegisterHandler(String, String),
    /// `jmc.setHotkey(клавиша, команды)`.
    SetHotkey(String, String),
    /// Метод JMC, которого у нас нет (имя метода) — движок предупредит один раз.
    Unsupported(String),
}

/// Итог одного запуска скрипта.
#[derive(Debug, Clone, Default)]
pub struct ScriptRun {
    pub calls: Vec<ScriptCall>,
    /// Сколько просьб отброшено сверх `MAX_CALLS`.
    pub dropped: usize,
    /// Текст ошибки (синтаксис, исключение, превышение времени).
    pub error: Option<String>,
}

/// Данные, общие для движка и методов `jmc` (методы — замыкания внутри JS, им нужен
/// `'static`-доступ, поэтому `Arc<Mutex<…>>`).
#[derive(Default)]
struct Shared {
    calls: Vec<ScriptCall>,
    dropped: usize,
    /// Снимок переменных на момент запуска + то, что скрипт задал через `jmc.setVar`.
    variables: BTreeMap<String, String>,
    /// Параметры события для `jmc.event(n)` (например, номер таймера).
    event: Vec<String>,
}

impl Shared {
    fn push(&mut self, call: ScriptCall) {
        if self.calls.len() < MAX_CALLS {
            self.calls.push(call);
        } else {
            self.dropped += 1;
        }
    }
}

/// `lock()`, который не паникует, если кто-то когда-то запаниковал с захваченным мьютексом.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Движок JS. `Send` (фича `parallel` у rquickjs), поэтому `Automation` можно отдавать
/// в другую задачу/поток.
pub struct ScriptEngine {
    // Порядок полей важен: контекст должен освобождаться раньше рантайма.
    context: Context,
    _runtime: Runtime,
    shared: Arc<Mutex<Shared>>,
    /// До какого момента скрипту можно работать; `None` — скрипт не выполняется.
    deadline: Arc<Mutex<Option<Instant>>>,
}

impl ScriptEngine {
    pub fn new() -> Result<ScriptEngine, String> {
        let runtime = Runtime::new().map_err(|e| e.to_string())?;
        runtime.set_memory_limit(MEMORY_LIMIT);
        runtime.set_max_stack_size(STACK_LIMIT);

        // QuickJS время от времени спрашивает «не пора ли прервать?» — отвечаем по часам.
        let deadline: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let watched = deadline.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            lock(&watched).is_some_and(|d| Instant::now() > d)
        })));

        let context = Context::full(&runtime).map_err(|e| e.to_string())?;
        let shared = Arc::new(Mutex::new(Shared::default()));
        context
            .with(|ctx| install_jmc(&ctx, &shared))
            .map_err(|e| format!("не удалось создать объект jmc: {e}"))?;
        Ok(ScriptEngine {
            context,
            _runtime: runtime,
            shared,
            deadline,
        })
    }

    /// Выполняет код. `variables` — переменные клиента для `jmc.getVar`,
    /// `event` — параметры события для `jmc.event(n)`.
    pub fn run(
        &mut self,
        code: &str,
        variables: &BTreeMap<String, String>,
        event: Vec<String>,
        connected: bool,
        limit: Duration,
    ) -> ScriptRun {
        {
            let mut shared = lock(&self.shared);
            shared.calls.clear();
            shared.dropped = 0;
            shared.variables = variables.clone();
            shared.event = event;
        }
        *lock(&self.deadline) = Some(Instant::now() + limit);

        let error = self.context.with(|ctx| {
            // Свойство только для чтения в JMC; у нас просто обновляем перед запуском.
            if let Ok(jmc) = ctx.globals().get::<_, Object>("jmc") {
                for name in ["IsConnected", "isConnected", "isconnected"] {
                    let _ = jmc.set(name, connected);
                }
            }
            match eval_sloppy(&ctx, code) {
                Ok(_) => None,
                Err(e) => Some(describe_error(&ctx, e)),
            }
        });

        let timed_out = lock(&self.deadline).is_some_and(|d| Instant::now() > d);
        *lock(&self.deadline) = None;
        let error = error.map(|e| {
            if timed_out {
                format!(
                    "прерван: выполнялся дольше {} мс (вечный цикл?)",
                    limit.as_millis()
                )
            } else {
                e
            }
        });

        let mut shared = lock(&self.shared);
        ScriptRun {
            calls: std::mem::take(&mut shared.calls),
            dropped: shared.dropped,
            error,
        }
    }
}

/// Выполняет код как обычный (не строгий) скрипт: JScript в JMC — не строгий,
/// там можно писать `x = 1` без `var`.
fn eval_sloppy<'js>(ctx: &Ctx<'js>, code: impl Into<Vec<u8>>) -> rquickjs::Result<Value<'js>> {
    let mut options = EvalOptions::default();
    options.strict = false;
    options.filename = Some("script".to_string());
    ctx.eval_with_options(code, options)
}

/// Понятный текст ошибки JS: «SyntaxError: … (строка 1)».
fn describe_error(ctx: &Ctx, error: rquickjs::Error) -> String {
    if !matches!(error, rquickjs::Error::Exception) {
        return error.to_string();
    }
    let value = ctx.catch();
    let Some(exception) = value.as_exception() else {
        // `throw "текст"` или `throw 42` — бросили не объект ошибки.
        let text = value
            .get::<Coerced<String>>()
            .map(|c| c.0)
            .unwrap_or_else(|_| "?".into());
        return format!("исключение: {text}");
    };
    let name: String = exception
        .as_object()
        .get("name")
        .unwrap_or_else(|_| "Error".into());
    let message = exception.message().unwrap_or_default();
    // Стек вида «    at <eval> (script:1:12)» — берём из него место ошибки.
    let place = exception
        .stack()
        .and_then(|s| {
            s.lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(str::to_string)
        })
        .map(|l| format!(" ({l})"))
        .unwrap_or_default();
    format!("{name}: {message}{place}")
}

/// Регистрирует метод под несколькими именами: `ShowMe`, `showMe`, `showme`
/// (в JScript имена методов COM-объекта не зависят от регистра).
fn method<'js, P, F>(ctx: &Ctx<'js>, obj: &Object<'js>, name: &str, f: F) -> rquickjs::Result<()>
where
    F: IntoJsFunc<'js, P> + 'js,
{
    let func = Function::new(ctx.clone(), f)?;
    let mut camel = name.to_string();
    if let Some(first) = camel.get(..1) {
        camel.replace_range(..1, &first.to_lowercase());
    }
    let mut names = vec![name.to_string(), camel, name.to_lowercase()];
    names.dedup();
    for n in names {
        obj.set(n, func.clone())?;
    }
    Ok(())
}

/// Создаёт глобальный объект `jmc`. Список методов — из `ttcoreex.idl` JMC 3.6.
fn install_jmc(ctx: &Ctx, shared: &Arc<Mutex<Shared>>) -> rquickjs::Result<()> {
    let jmc = Object::new(ctx.clone())?;

    // Маленькие помощники: замыкание, которое просто записывает просьбу.
    let s = shared.clone();
    method(ctx, &jmc, "Parse", move |cmd: Coerced<String>| {
        lock(&s).push(ScriptCall::Parse(cmd.0));
    })?;
    let s = shared.clone();
    method(ctx, &jmc, "Send", move |line: Coerced<String>| {
        lock(&s).push(ScriptCall::Send(line.0));
    })?;
    // Цвет (второй аргумент) пока игнорируется.
    for name in ["ShowMe", "Output"] {
        let s = shared.clone();
        method(
            ctx,
            &jmc,
            name,
            move |text: Coerced<String>, _color: Opt<Coerced<String>>| {
                lock(&s).push(ScriptCall::Echo(text.0));
            },
        )?;
    }
    // Отдельных окон вывода пока нет — показываем в основном.
    let s = shared.clone();
    method(
        ctx,
        &jmc,
        "wOutput",
        move |_window: Coerced<i32>, text: Coerced<String>, _color: Opt<Coerced<String>>| {
            lock(&s).push(ScriptCall::Echo(text.0));
        },
    )?;
    let s = shared.clone();
    method(ctx, &jmc, "Beep", move || {
        lock(&s).push(ScriptCall::Bell);
    })?;
    let s = shared.clone();
    method(
        ctx,
        &jmc,
        "SetVar",
        move |name: Coerced<String>, value: Coerced<String>, _global: Opt<Value>| {
            let mut shared = lock(&s);
            shared.variables.insert(name.0.clone(), value.0.clone());
            shared.push(ScriptCall::SetVar(name.0, value.0));
        },
    )?;
    let s = shared.clone();
    method(
        ctx,
        &jmc,
        "GetVar",
        move |name: Coerced<String>| -> String {
            lock(&s).variables.get(&name.0).cloned().unwrap_or_default()
        },
    )?;
    // jmc.eval(код) — выполнить ещё кусок JS (в JMC — тоже скрипт, а не команда клиента).
    method(
        ctx,
        &jmc,
        "Eval",
        move |ctx: Ctx<'_>, code: Coerced<String>| -> rquickjs::Result<()> {
            eval_sloppy(&ctx, code.0)?;
            Ok(())
        },
    )?;
    let s = shared.clone();
    method(
        ctx,
        &jmc,
        "Connect",
        move |address: Coerced<String>, port: Coerced<String>| {
            lock(&s).push(ScriptCall::Connect(address.0, port.0));
        },
    )?;
    let s = shared.clone();
    method(ctx, &jmc, "Disconnect", move || {
        lock(&s).push(ScriptCall::Disconnect);
    })?;
    let s = shared.clone();
    method(
        ctx,
        &jmc,
        "SetTimer",
        move |id: Coerced<i64>, interval: Coerced<i64>, pre: Opt<Coerced<i64>>| {
            lock(&s).push(ScriptCall::SetTimer {
                id: id.0,
                interval: interval.0,
                pre: pre.0.map(|p| p.0).unwrap_or(0),
            });
        },
    )?;
    let s = shared.clone();
    method(ctx, &jmc, "KillTimer", move |id: Coerced<i64>| {
        lock(&s).push(ScriptCall::KillTimer(id.0));
    })?;
    let s = shared.clone();
    method(
        ctx,
        &jmc,
        "RegisterHandler",
        move |event: Coerced<String>, code: Coerced<String>| {
            lock(&s).push(ScriptCall::RegisterHandler(event.0, code.0));
        },
    )?;
    let s = shared.clone();
    method(
        ctx,
        &jmc,
        "Event",
        move |index: Opt<Coerced<i64>>| -> String {
            let i = index.0.map(|i| i.0).unwrap_or(0);
            let shared = lock(&s);
            usize::try_from(i)
                .ok()
                .and_then(|i| shared.event.get(i).cloned())
                .unwrap_or_default()
        },
    )?;
    // События не отменяются (поддержан только Timer/PreTimer) — ничего не делаем.
    method(ctx, &jmc, "DropEvent", move || {})?;
    let s = shared.clone();
    method(
        ctx,
        &jmc,
        "SetHotkey",
        move |key: Coerced<String>, command: Coerced<String>| {
            lock(&s).push(ScriptCall::SetHotkey(key.0, command.0));
        },
    )?;

    // Нереализованное: записываем «не поддерживается», возвращаем что-то безобидное.
    for name in [
        "SetStatus",
        "Quit",
        "Run",
        "Play",
        "DoTelnet",
        "MSDP2GMCP",
        "GMCP2MSDP",
        "MSSP2GMCP",
    ] {
        let s = shared.clone();
        let label = name.to_string();
        method(
            ctx,
            &jmc,
            name,
            move |_args: rquickjs::prelude::Rest<Value>| {
                lock(&s).push(ScriptCall::Unsupported(label.clone()));
            },
        )?;
    }
    // Преобразования цвета: пока возвращаем текст как есть.
    for name in ["ToText", "ToColored", "FromColored"] {
        method(ctx, &jmc, name, move |text: Coerced<String>| -> String {
            text.0
        })?;
    }
    for name in ["wGetWidth", "wGetHeight"] {
        let size = if name == "wGetWidth" { 80 } else { 25 };
        method(ctx, &jmc, name, move |_window: Opt<Value>| -> i32 { size })?;
    }

    for (name, value) in [("Profile", ""), ("CommandChar", "#")] {
        jmc.set(name, value)?;
        jmc.set(name.to_lowercase(), value)?;
    }
    ctx.globals().set("jmc", jmc)?;
    Ok(())
}

/// Обёртка для поля `Automation`: движок создаётся лениво; при клонировании `Automation`
/// копия получает новый (пустой) движок — состояние JS не копируется.
pub struct ScriptSlot {
    pub engine: Option<ScriptEngine>,
    pub time_limit: Duration,
}

impl Default for ScriptSlot {
    fn default() -> ScriptSlot {
        ScriptSlot {
            engine: None,
            time_limit: DEFAULT_TIME_LIMIT,
        }
    }
}

impl Clone for ScriptSlot {
    fn clone(&self) -> ScriptSlot {
        ScriptSlot {
            engine: None,
            time_limit: self.time_limit,
        }
    }
}

impl std::fmt::Debug for ScriptSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptSlot")
            .field("started", &self.engine.is_some())
            .field("time_limit", &self.time_limit)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(engine: &mut ScriptEngine, code: &str) -> ScriptRun {
        engine.run(
            code,
            &BTreeMap::new(),
            Vec::new(),
            true,
            Duration::from_millis(200),
        )
    }

    #[test]
    fn просьбы_по_порядку_и_регистр_имён() {
        let mut e = ScriptEngine::new().unwrap();
        let r = run(
            &mut e,
            "jmc.Parse('a'); jmc.send('b'); jmc.showMe('c'); jmc.setVar('x', 5); jmc.showme(jmc.getVar('x'))",
        );
        assert_eq!(r.error, None);
        assert_eq!(
            r.calls,
            [
                ScriptCall::Parse("a".into()),
                ScriptCall::Send("b".into()),
                ScriptCall::Echo("c".into()),
                ScriptCall::SetVar("x".into(), "5".into()),
                ScriptCall::Echo("5".into()),
            ]
        );
    }

    #[test]
    fn eval_и_исключение() {
        let mut e = ScriptEngine::new().unwrap();
        let r = run(&mut e, "jmc.eval('y = 2'); jmc.showme(y)");
        assert_eq!(r.error, None);
        assert_eq!(r.calls, [ScriptCall::Echo("2".into())]);
        let r = run(&mut e, "throw new TypeError('плохо')");
        assert!(r.error.unwrap().starts_with("TypeError: плохо"));
        let r = run(&mut e, "throw 'просто текст'");
        assert_eq!(r.error.unwrap(), "исключение: просто текст");
    }

    #[test]
    fn движок_send() {
        fn assert_send<T: Send>() {}
        assert_send::<ScriptEngine>();
    }
}
