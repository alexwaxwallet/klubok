//! Схлопывание пустых строк.
//!
//! Сервер Былин вставляет `\r\n` перед каждой порцией вывода, а в некомпактном
//! режиме — ещё и пустую строку перед промптом. Отсюда двойные пустые строки,
//! которые в JMC не убрать.
//!
//! Чтобы убрать пустую строку *перед* промптом, её приходится придержать: пока не
//! пришло, что идёт за ней, непонятно, нужна она или нет. Придержанные строки
//! отдаются, когда приходит непустая строка или по таймеру (`flush`).

use klubok_proto::StyledLine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlankLineSettings {
    /// Сколько пустых строк подряд оставлять (0 — убирать все).
    pub max_consecutive: usize,
    /// Убирать пустые строки сразу после промпта.
    pub trim_after_prompt: bool,
    /// Убирать пустые строки прямо перед промптом.
    pub trim_before_prompt: bool,
}

impl Default for BlankLineSettings {
    fn default() -> Self {
        BlankLineSettings {
            max_consecutive: 1,
            trim_after_prompt: true,
            trim_before_prompt: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct BlankLineFilter {
    pub settings: BlankLineSettings,
    /// Придержанные пустые строки (уже с учётом лимита подряд).
    pending: Vec<StyledLine>,
    /// Сколько пустых строк подряд уже выпущено или придержано.
    blank_run: usize,
    /// Предыдущим был промпт, и с тех пор не было непустой строки.
    after_prompt: bool,
}

impl BlankLineFilter {
    pub fn new(settings: BlankLineSettings) -> Self {
        BlankLineFilter {
            settings,
            ..Default::default()
        }
    }

    /// Есть придержанные строки — нужен таймер `flush`.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Обычная строка. Возвращает строки, которые можно показывать прямо сейчас.
    pub fn line(&mut self, line: StyledLine) -> Vec<StyledLine> {
        if !line.is_blank() {
            self.blank_run = 0;
            self.after_prompt = false;
            let mut out = std::mem::take(&mut self.pending);
            out.push(line);
            return out;
        }
        if self.after_prompt && self.settings.trim_after_prompt {
            return Vec::new();
        }
        if self.blank_run >= self.settings.max_consecutive {
            return Vec::new();
        }
        self.blank_run += 1;
        if self.settings.trim_before_prompt {
            self.pending.push(line);
            Vec::new()
        } else {
            vec![line]
        }
    }

    /// Пришёл промпт. Возвращает придержанные строки, которые надо показать перед ним.
    pub fn prompt(&mut self) -> Vec<StyledLine> {
        let held = std::mem::take(&mut self.pending);
        self.blank_run = 0;
        self.after_prompt = true;
        if self.settings.trim_before_prompt {
            Vec::new()
        } else {
            held
        }
    }

    /// Сервер замолчал: отдать придержанное.
    pub fn flush(&mut self) -> Vec<StyledLine> {
        std::mem::take(&mut self.pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Прогоняет строки через фильтр; строка, начинающаяся с ">", — промпт, "~" — пауза (flush).
    fn run(settings: BlankLineSettings, input: &[&str]) -> Vec<String> {
        let mut f = BlankLineFilter::new(settings);
        let mut out = Vec::new();
        for &s in input {
            let shown = if s.starts_with('>') {
                let mut v: Vec<String> = f.prompt().iter().map(|l| l.text()).collect();
                v.push(s.to_string());
                v
            } else if s == "~" {
                f.flush().iter().map(|l| l.text()).collect()
            } else {
                f.line(StyledLine::plain(s))
                    .iter()
                    .map(|l| l.text())
                    .collect()
            };
            out.extend(shown);
        }
        out
    }

    #[test]
    fn по_умолчанию() {
        let out = run(
            BlankLineSettings::default(),
            &[
                "> ",
                "",
                "",
                "Вы ударили.",
                "",
                "",
                "",
                "Он умер.",
                "",
                "> ",
            ],
        );
        assert_eq!(out, ["> ", "Вы ударили.", "", "Он умер.", "> "]);
    }

    #[test]
    fn пауза_выпускает_придержанное() {
        let out = run(BlankLineSettings::default(), &["a", "", "~", "b"]);
        assert_eq!(out, ["a", "", "b"]);
    }

    #[test]
    fn убрать_все() {
        let settings = BlankLineSettings {
            max_consecutive: 0,
            ..BlankLineSettings::default()
        };
        let out = run(settings, &["a", "", " ", "b"]);
        assert_eq!(out, ["a", "b"]);
    }

    #[test]
    fn ничего_не_трогать_у_промпта() {
        let settings = BlankLineSettings {
            max_consecutive: 1,
            trim_after_prompt: false,
            trim_before_prompt: false,
        };
        let out = run(settings, &["> ", "", "", "a", "", "> "]);
        assert_eq!(out, ["> ", "", "a", "", "> "]);
    }
}
