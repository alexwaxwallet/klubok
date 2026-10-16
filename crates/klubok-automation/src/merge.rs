//! Склейка нескольких строк в одну (`#merge`).
//!
//! Правило: строка, подходящая под шаблон начала, открывает склейку; следующие строки
//! копятся, пока не придёт строка под шаблон конца (она тоже входит) или не наберётся
//! нужное число строк. Готовая склейка — одна строка: куски через разделитель, цвета сохраняются.
//!
//! Промпт закрывает незаконченную склейку: всё, что набралось, показывается сразу.
//! Пустые строки внутри склейки выбрасываются.

use std::collections::BTreeMap;

use klubok_proto::{Style, StyledLine};

use crate::pattern::Pattern;

/// Больше стольких строк в одну не склеиваем — защита от шаблона конца, который не сработал.
pub const MAX_MERGED_LINES: usize = 50;

/// Где кончается склейка.
#[derive(Debug, Clone)]
pub enum MergeEnd {
    /// Строка под этот шаблон (она входит в склейку).
    Pattern(Pattern),
    /// Всего столько строк, считая первую.
    Lines(usize),
}

#[derive(Debug, Clone)]
pub struct MergeRule {
    pub start: Pattern,
    pub end: MergeEnd,
    /// Как в записи: шаблон или число.
    pub end_source: String,
    pub separator: String,
    pub group: String,
}

impl MergeRule {
    /// `end` — шаблон конца или число строк.
    pub fn new(start: &str, end: &str, separator: &str, group: &str) -> MergeRule {
        let end_trimmed = end.trim();
        let end_rule = match end_trimmed.parse::<usize>() {
            Ok(n) if n >= 1 => MergeEnd::Lines(n.min(MAX_MERGED_LINES)),
            _ => MergeEnd::Pattern(Pattern::new(end)),
        };
        MergeRule {
            start: Pattern::new(start),
            end: end_rule,
            end_source: end.to_string(),
            separator: separator.to_string(),
            group: group.to_string(),
        }
    }
}

/// Открытая склейка.
#[derive(Debug, Clone)]
struct Open {
    rule: usize,
    separator: String,
    lines: Vec<StyledLine>,
}

/// Состояние склейки во время игры.
#[derive(Debug, Clone, Default)]
pub struct Merger {
    open: Option<Open>,
}

impl Merger {
    /// Строка с сервера. Возвращает строки, которые пора показывать (0, 1 или несколько).
    /// `enabled` — включена ли группа правила.
    pub fn line(
        &mut self,
        rules: &mut [MergeRule],
        line: StyledLine,
        vars: &BTreeMap<String, String>,
        enabled: impl Fn(&str) -> bool,
    ) -> Vec<StyledLine> {
        let text = line.text();
        if let Some(open) = self.open.as_mut() {
            // Правило могли удалить, пока склейка открыта.
            let Some(rule) = rules.get_mut(open.rule) else {
                return self.close().into_iter().chain([line]).collect();
            };
            if !line.is_blank() {
                open.lines.push(line);
            }
            let done = match &mut rule.end {
                MergeEnd::Pattern(p) => p.find(&text, vars).is_some(),
                MergeEnd::Lines(n) => open.lines.len() >= *n,
            };
            if done || open.lines.len() >= MAX_MERGED_LINES {
                return self.close().into_iter().collect();
            }
            return Vec::new();
        }

        for (i, rule) in rules.iter_mut().enumerate() {
            if !enabled(&rule.group) || rule.start.find(&text, vars).is_none() {
                continue;
            }
            // Склейка из одной строки — сразу готова.
            if matches!(rule.end, MergeEnd::Lines(1)) {
                return vec![line];
            }
            self.open = Some(Open {
                rule: i,
                separator: rule.separator.clone(),
                lines: vec![line],
            });
            return Vec::new();
        }
        vec![line]
    }

    /// Промпт или конец соединения: показать то, что набралось.
    pub fn flush(&mut self) -> Option<StyledLine> {
        self.close()
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    fn close(&mut self) -> Option<StyledLine> {
        let open = self.open.take()?;
        Some(join(open.lines, &open.separator))
    }
}

/// Склеивает строки: куски через `separator`, у каждой строки срезаются пробелы по краям.
pub fn join(lines: Vec<StyledLine>, separator: &str) -> StyledLine {
    let mut out = StyledLine::default();
    for (i, line) in lines.into_iter().enumerate() {
        if i > 0 && !separator.is_empty() {
            out.push(separator, Style::default());
        }
        let text = line.text();
        let start = text.len() - text.trim_start().len();
        let end = text.trim_end().len();
        let mut pos = 0;
        for span in line.spans {
            let span_end = pos + span.text.len();
            let from = start.max(pos);
            let to = end.min(span_end);
            if from < to {
                out.push(&span.text[from - pos..to - pos], span.style);
            }
            pos = span_end;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rules: &mut [MergeRule], lines: &[&str]) -> Vec<String> {
        let mut m = Merger::default();
        let vars = BTreeMap::new();
        let mut out: Vec<String> = Vec::new();
        for l in lines {
            out.extend(
                m.line(rules, StyledLine::plain(l), &vars, |_| true)
                    .iter()
                    .map(StyledLine::text),
            );
        }
        out.extend(m.flush().map(|l| l.text()));
        out
    }

    #[test]
    fn по_шаблону_конца() {
        let mut rules = [MergeRule::new("^Вы несете:", "^%0:", ", ", "default")];
        // Пустая строка внутри выбрасывается, строка конца входит в склейку.
        let out = run(
            &mut rules,
            &[
                "до",
                "Вы несете:",
                "  меч",
                "",
                "  щит  ",
                "конец: да",
                "после",
            ],
        );
        assert_eq!(out, ["до", "Вы несете:, меч, щит, конец: да", "после"]);
    }

    #[test]
    fn по_числу_строк_и_промпт() {
        let mut rules = [MergeRule::new("^Аффекты", "3", " | ", "default")];
        assert_eq!(
            run(&mut rules, &["Аффекты", "а", "б", "в"]),
            ["Аффекты | а | б", "в"]
        );
        assert_eq!(run(&mut rules, &["Аффекты", "а"]), ["Аффекты | а"]);
    }

    #[test]
    fn цвета_сохраняются() {
        let mut first = StyledLine::default();
        let red = Style {
            fg: klubok_proto::Color::Named(1),
            ..Style::default()
        };
        first.push("  ", Style::default());
        first.push("красный", red);
        let joined = join(vec![first, StyledLine::plain("б ")], "+");
        assert_eq!(joined.text(), "красный+б");
        assert_eq!(joined.spans[0].style, red);
    }
}
