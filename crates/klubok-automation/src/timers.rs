//! Время в автоматизации: тикер, циклы с задержкой (`#5:10 {…}`), `#wait`, таймеры скриптов.
//!
//! Здесь только данные и расчёт «что пора сделать». Время — `Duration` от начала сессии,
//! его даёт сессия (реальные часы в игре, выдуманные — в тестах).

use std::time::Duration;

/// Десятая доля секунды — единица задержек в JMC (`#wait 10` — одна секунда).
pub const DECISECOND: Duration = Duration::from_millis(100);
/// Длительность тика по умолчанию (в Былинах «Минул час.» — раз в минуту).
pub const DEFAULT_TICK_SIZE: Duration = Duration::from_secs(60);
/// За сколько до тика предупреждать.
const TICK_WARNING: Duration = Duration::from_secs(10);
/// Сколько циклов с задержкой может работать одновременно (как в JMC).
pub const MAX_TIMERS: usize = 16;

/// Тексты JMC (language.ini) — на них можно ставить триггеры.
pub const TICK_TEXT: &str = "#ТИК";
pub const TICK_WARNING_TEXT: &str = "#10 СЕКУНД ДО ТИКА";

/// Что сообщил тикер.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickEvent {
    /// Через 10 секунд тик.
    Warning,
    /// Тик.
    Tick,
}

/// Тикер JMC: отсчитывает тики (`#tickset` — «тик был только что»), по `#tickon` сообщает о них.
/// Счёт идёт и при выключенном тикере — выключение только прячет сообщения.
#[derive(Debug, Clone)]
pub struct Ticker {
    pub enabled: bool,
    pub size: Duration,
    /// Когда следующий тик.
    next_tick: Duration,
    /// Предупреждение о текущем тике уже показано.
    warned: bool,
}

impl Default for Ticker {
    fn default() -> Ticker {
        Ticker {
            enabled: false,
            size: DEFAULT_TICK_SIZE,
            next_tick: DEFAULT_TICK_SIZE,
            warned: false,
        }
    }
}

impl Ticker {
    /// `#tickset`: тик только что был.
    pub fn reset(&mut self, now: Duration) {
        self.next_tick = now + self.size;
        self.warned = false;
    }

    /// Переносит «следующий тик» в будущее, не сообщая о пропущенных.
    pub fn catch_up(&mut self, now: Duration) {
        if self.size.is_zero() {
            return;
        }
        while self.next_tick <= now {
            self.next_tick += self.size;
            self.warned = false;
        }
    }

    /// Сколько целых секунд до тика (для `#tick`), с округлением вверх.
    pub fn seconds_left(&mut self, now: Duration) -> u64 {
        self.catch_up(now);
        let left = self.next_tick.saturating_sub(now);
        left.as_secs() + u64::from(left.subsec_nanos() > 0)
    }

    /// Что произошло к моменту `now`. Тик, пропущенный больше чем на целый период
    /// (компьютер спал), не объявляется.
    pub fn poll(&mut self, now: Duration) -> Option<TickEvent> {
        if self.size.is_zero() {
            return None;
        }
        if now >= self.next_tick {
            let missed_many = now >= self.next_tick + self.size;
            self.catch_up(now);
            return (self.enabled && !missed_many).then_some(TickEvent::Tick);
        }
        if self.enabled
            && !self.warned
            && self.size > TICK_WARNING
            && now + TICK_WARNING >= self.next_tick
        {
            self.warned = true;
            return Some(TickEvent::Warning);
        }
        None
    }

    /// Когда тикеру в следующий раз есть что сказать.
    pub fn deadline(&self) -> Option<Duration> {
        if !self.enabled || self.size.is_zero() {
            return None;
        }
        if !self.warned && self.size > TICK_WARNING {
            Some(self.next_tick.saturating_sub(TICK_WARNING))
        } else {
            Some(self.next_tick)
        }
    }
}

/// Разновидность таймера: цикл JMC или наша одноразовая задержка `#delay`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerKind {
    /// `#5:10 {команда}`, `#loop {…}` — `%0` в команде заменяется счётчиком.
    Cycle,
    /// `#delay {секунды} {команда}` — выполнить один раз.
    Delay,
}

/// Цикл с задержкой или отложенная команда.
#[derive(Debug, Clone)]
pub struct Timer {
    pub kind: TimerKind,
    pub command: String,
    /// Текущее значение счётчика (`%0`) и конечное значение.
    pub counter: i64,
    pub end: i64,
    pub step: i64,
    /// Период повтора.
    pub period: Duration,
    /// Когда сработать в следующий раз.
    pub next: Duration,
    /// `false` — приостановлен (`#break`), можно продолжить `#resume`.
    pub active: bool,
}

impl Timer {
    /// Счётчик для этого срабатывания; сдвигает счётчик и время. `true` во втором значении —
    /// это было последнее срабатывание.
    pub fn fire(&mut self, now: Duration) -> (i64, bool) {
        let current = self.counter;
        let done = if self.counter <= self.end {
            self.counter += self.step;
            self.counter > self.end
        } else {
            self.counter -= self.step;
            self.counter < self.end
        };
        // Без «догоняния»: если сессия опоздала, следующее срабатывание — через период от сейчас.
        self.next += self.period;
        if self.next <= now {
            self.next = now + self.period;
        }
        (current, done)
    }

    /// Сколько срабатываний осталось.
    pub fn remaining(&self) -> i64 {
        (self.end - self.counter).abs() / self.step.max(1) + 1
    }
}

/// Таймер скрипта (`jmc.setTimer`): событие `Timer` каждые `interval`,
/// событие `PreTimer` за `pre` до него.
#[derive(Debug, Clone)]
pub struct ScriptTimer {
    pub id: i64,
    pub interval: Duration,
    pub pre: Duration,
    /// Когда сработал в прошлый раз (или когда заведён).
    pub last: Duration,
    pub pre_done: bool,
}

impl ScriptTimer {
    pub fn due(&self) -> Duration {
        self.last + self.interval
    }

    pub fn pre_due(&self) -> Option<Duration> {
        (!self.pre.is_zero() && !self.pre_done).then(|| self.due().saturating_sub(self.pre))
    }
}

/// Разбор `#loop {от[,до][|шаг][:задержка]}` → (от, до, шаг, задержка в десятых секунды).
/// Без «до» JMC считает до нуля.
pub fn parse_loop_spec(spec: &str) -> Option<(i64, i64, i64, u64)> {
    let mut parts: Vec<(char, String)> = vec![(' ', String::new())];
    for c in spec.trim().chars() {
        if matches!(c, ',' | '|' | ':') {
            parts.push((c, String::new()));
        } else if let Some(last) = parts.last_mut() {
            last.1.push(c);
        }
    }
    let number = |key: char| -> Option<Option<i64>> {
        match parts.iter().find(|(k, _)| *k == key) {
            Some((_, v)) => v.trim().parse().ok().map(Some),
            None => Some(None),
        }
    };
    let from = number(' ')??;
    let to = number(',')?.unwrap_or(0);
    let step = number('|')?.unwrap_or(1).max(1);
    let delay = number(':')?.unwrap_or(0).max(0) as u64;
    Some((from, to, step, delay))
}

/// `#5:10` → (5, 10): сколько раз и задержка в десятых секунды.
pub fn parse_repeat(name: &str) -> Option<(i64, u64)> {
    let (count, delay) = match name.split_once(':') {
        Some((c, d)) => (c, d),
        None => (name, "0"),
    };
    Some((count.parse().ok()?, delay.parse().ok()?))
}

/// Секунды (дробные, с точкой или запятой) → `Duration`.
pub fn parse_seconds(text: &str) -> Option<Duration> {
    let secs: f64 = text.trim().replace(',', ".").parse().ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    #[test]
    fn тикер() {
        let mut t = Ticker {
            enabled: true,
            ..Ticker::default()
        };
        t.reset(s(5)); // тик в 65 с, предупреждение в 55 с
        assert_eq!(t.deadline(), Some(s(55)));
        assert_eq!(t.poll(s(54)), None);
        assert_eq!(t.poll(s(55)), Some(TickEvent::Warning));
        assert_eq!(t.deadline(), Some(s(65)));
        assert_eq!(t.seconds_left(Duration::from_millis(60_500)), 5);
        assert_eq!(t.poll(s(65)), Some(TickEvent::Tick));
        assert_eq!(t.deadline(), Some(s(115)));
        // Компьютер «спал» три минуты — о пропущенных тиках не сообщаем.
        assert_eq!(t.poll(s(300)), None);
        assert_eq!(t.deadline(), Some(s(295))); // тик в 305, предупреждение в 295
        t.enabled = false;
        assert_eq!(t.deadline(), None);
    }

    #[test]
    fn разбор_циклов() {
        assert_eq!(parse_repeat("3000:15"), Some((3000, 15)));
        assert_eq!(parse_repeat("3"), Some((3, 0)));
        assert_eq!(parse_repeat("3:x"), None);
        assert_eq!(parse_loop_spec("1,5:10"), Some((1, 5, 1, 10)));
        assert_eq!(parse_loop_spec("10,1|3"), Some((10, 1, 3, 0)));
        assert_eq!(parse_loop_spec("3:5|2"), Some((3, 0, 2, 5)));
        assert_eq!(parse_loop_spec("x"), None);
        assert_eq!(parse_seconds("1,5"), Some(Duration::from_millis(1500)));
    }

    #[test]
    fn счётчик_цикла() {
        let mut t = Timer {
            kind: TimerKind::Cycle,
            command: String::new(),
            counter: 3,
            end: 1,
            step: 1,
            period: s(1),
            next: s(0),
            active: true,
        };
        assert_eq!(t.remaining(), 3);
        assert_eq!(t.fire(s(0)), (3, false));
        assert_eq!(t.fire(s(1)), (2, false));
        assert_eq!(t.fire(s(5)), (1, true));
        assert_eq!(t.next, s(6));
    }
}
