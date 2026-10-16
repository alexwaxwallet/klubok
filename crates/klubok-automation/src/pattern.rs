//! Шаблоны триггеров, подсветок и замен.
//!
//! Два вида, как в JMC:
//! * обычный: `^Вы ударили %1.` — `^` в начале привязывает к началу строки,
//!   `%0`…`%9` ловят любой текст, остальное сравнивается буквально;
//! * регулярное выражение: `/^(\w+) ударил вас/i` (флаг `i` — без учёта регистра).
//!
//! `$переменные` подставляются в момент проверки, поэтому шаблон компилируется
//! заново, только когда изменился итоговый текст.

use std::collections::BTreeMap;

use regex::Regex;

use crate::script::substitute_vars;

#[derive(Debug, Clone)]
pub struct Pattern {
    /// Шаблон как его написал пользователь.
    pub source: String,
    /// Текст после подстановки переменных и скомпилированная регулярка (`None` — ошибка в шаблоне).
    cache: Option<(String, Option<Regex>)>,
}

/// Результат совпадения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Где совпало (байтовые смещения).
    pub range: std::ops::Range<usize>,
    /// `%0`…`%9`.
    pub params: Vec<String>,
}

impl Pattern {
    pub fn new(source: &str) -> Pattern {
        Pattern {
            source: source.to_string(),
            cache: None,
        }
    }

    pub fn is_regex(&self) -> bool {
        split_regex(&self.source).is_some()
    }

    /// Проверяет строку. `None` — не совпало (или шаблон с ошибкой).
    pub fn find(&mut self, text: &str, vars: &BTreeMap<String, String>) -> Option<Match> {
        let resolved = substitute_vars(&self.source, vars);
        let stale = self.cache.as_ref().is_none_or(|(src, _)| *src != resolved);
        if stale {
            let regex = compile(&resolved);
            self.cache = Some((resolved, regex));
        }
        let regex = self.cache.as_ref()?.1.as_ref()?;
        let caps = regex.captures(text)?;
        let whole = caps.get(0)?;
        let text_of =
            |m: Option<regex::Match>| m.map(|m| m.as_str().to_string()).unwrap_or_default();

        let params = if self.is_regex() {
            // В регулярке %0 — первая группа, %1 — вторая и т.д. Так ведёт себя JMC:
            // «{/сообщил группе : '([^']+)/} {%0}» выполняет пойманный текст.
            (1..=10).map(|i| text_of(caps.get(i))).collect()
        } else {
            (0..10)
                .map(|n| text_of(caps.name(&format!("p{n}"))))
                .collect()
        };
        Some(Match {
            range: whole.range(),
            params,
        })
    }
}

/// `/тело/флаги` → (тело, флаги).
fn split_regex(source: &str) -> Option<(&str, &str)> {
    let rest = source.strip_prefix('/')?;
    let end = rest.rfind('/')?;
    let flags = &rest[end + 1..];
    flags
        .chars()
        .all(|c| "imgs".contains(c))
        .then_some((&rest[..end], flags))
}

fn compile(source: &str) -> Option<Regex> {
    let expr = match split_regex(source) {
        Some((body, flags)) => {
            let inline: String = flags.chars().filter(|c| "is".contains(*c)).collect();
            if inline.is_empty() {
                body.to_string()
            } else {
                format!("(?{inline}){body}")
            }
        }
        None => jmc_to_regex(source),
    };
    Regex::new(&expr).ok()
}

/// Обычный шаблон JMC → регулярное выражение с группами `p0`…`p9`.
fn jmc_to_regex(source: &str) -> String {
    let (anchor, body) = match source.strip_prefix('^') {
        Some(rest) => ("^", rest),
        None => ("", source),
    };
    let mut expr = String::from(anchor);
    let mut literal = String::new();
    let mut used = [false; 10];
    let mut ends_with_param = false;
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%'
            && let Some(d) = chars.peek().and_then(|d| d.to_digit(10))
        {
            chars.next();
            expr.push_str(&regex::escape(&std::mem::take(&mut literal)));
            let d = d as usize;
            if used[d] {
                // Повтор того же %N — просто «любой текст» (имя группы не может повторяться).
                expr.push_str("(?:.*?)");
            } else {
                expr.push_str(&format!("(?P<p{d}>.*?)"));
                used[d] = true;
            }
            ends_with_param = true;
            continue;
        }
        literal.push(c);
        ends_with_param = false;
    }
    expr.push_str(&regex::escape(&literal));

    // %N в самом конце шаблона забирает всё до конца строки, а не пустоту.
    if ends_with_param && expr.ends_with(".*?)") {
        expr.truncate(expr.len() - ".*?)".len());
        expr.push_str(".*)");
    }
    expr
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(pattern: &str, text: &str) -> Option<Match> {
        Pattern::new(pattern).find(text, &BTreeMap::new())
    }

    #[test]
    fn буквальный_шаблон() {
        let m = find(
            "Действие клича битвы закончилось.",
            "x Действие клича битвы закончилось.",
        )
        .unwrap();
        assert_eq!(m.range.start, 2);
        assert!(find("^Действие клича", "x Действие клича").is_none());
        // Точка — это точка, а не «любой символ».
        assert!(find("закончилось.", "закончилосьX").is_none());
    }

    #[test]
    fn параметры_шаблона() {
        let m = find("Здесь лежит %1 %2 кун.", "Здесь лежит 15 больших кун.").unwrap();
        assert_eq!(m.params[1], "15");
        assert_eq!(m.params[2], "больших");

        let m = find("^%0 ударил вас%1", "Тролль ударил вас очень больно").unwrap();
        assert_eq!(m.params[0], "Тролль");
        assert_eq!(m.params[1], " очень больно");

        // Номера не по порядку и повтор.
        let m = find("%2 дал %1 %2", "Вася дал меч Вася").unwrap();
        assert_eq!(m.params[2], "Вася");
        assert_eq!(m.params[1], "меч");
    }

    #[test]
    fn регулярка_с_переменной() {
        let mut vars = BTreeMap::new();
        vars.insert("name".to_string(), "Колдун".to_string());
        let mut p = Pattern::new("/^Криск сообщил группе : '$name ([^']+)/");
        let m = p
            .find("Криск сообщил группе : 'Колдун вст'", &vars)
            .unwrap();
        assert_eq!(m.params[0], "вст");
        assert!(p.find("Криск сообщил группе : 'Вася вст'", &vars).is_none());
    }

    #[test]
    fn регулярка_без_учёта_регистра_и_битая() {
        assert!(find("/^вы УПАЛИ/i", "Вы упали").is_some());
        assert!(find("/([/", "что угодно").is_none());
    }
}
