//! Разбор текста команд в стиле JMC.
//!
//! * Команды разделяются `;`, но только вне фигурных скобок: `#alias {a} {b;c}` — одна команда.
//! * Аргументы `#`-команд — группы в `{}` или отдельные слова.
//! * `%0`…`%9` — аргументы алиаса или куски строки, пойманные триггером.
//! * `$имя` — значение переменной.

use std::collections::BTreeMap;

/// Делит текст на команды по `;` верхнего уровня.
pub fn split_commands(text: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '{' => {
                depth += 1;
                current.push(c);
            }
            '}' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            ';' if depth == 0 => commands.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    commands.push(current);
    commands
        .into_iter()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect()
}

/// `#имя аргументы` → ("имя" в нижнем регистре, аргументы).
pub fn parse_hash_command(command: &str) -> Option<(String, Vec<String>)> {
    let rest = command.strip_prefix('#')?;
    let name_end = rest
        .find(|c: char| c.is_whitespace() || c == '{')
        .unwrap_or(rest.len());
    let name = rest[..name_end].to_lowercase();
    Some((name, parse_args(&rest[name_end..])))
}

/// Всё, что после имени `#`-команды, как есть (для `#script`, где аргумент — код).
pub fn hash_command_rest(command: &str) -> &str {
    let rest = command.strip_prefix('#').unwrap_or(command);
    let name_end = rest
        .find(|c: char| c.is_whitespace() || c == '{')
        .unwrap_or(rest.len());
    rest[name_end..].trim()
}

/// Аргументы: `{группа со {вложенными} скобками}` или слово до пробела.
pub fn parse_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let Some(&first) = chars.peek() else { break };
        let mut arg = String::new();
        if first == '{' {
            chars.next();
            let mut depth = 1;
            for c in chars.by_ref() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                arg.push(c);
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || c == '{' {
                    break;
                }
                arg.push(c);
                chars.next();
            }
        }
        args.push(arg);
    }
    args
}

/// Заменяет `%0`…`%9` значениями из `params` (отсутствующие — пустая строка).
pub fn substitute_params(text: &str, params: &[String]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%'
            && let Some(d) = chars.peek().and_then(|d| d.to_digit(10))
        {
            chars.next();
            out.push_str(params.get(d as usize).map(String::as_str).unwrap_or(""));
            continue;
        }
        out.push(c);
    }
    out
}

/// Заменяет `$имя` значениями переменных. Неизвестные переменные остаются как есть.
/// Имя — буквы, цифры и `_`; `${имя}` — явная граница имени.
pub fn substitute_vars(text: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let mut name = String::new();
        let braced = chars.peek() == Some(&'{');
        if braced {
            chars.next();
            for n in chars.by_ref() {
                if n == '}' {
                    break;
                }
                name.push(n);
            }
        } else {
            while let Some(&n) = chars.peek() {
                if n.is_alphanumeric() || n == '_' {
                    name.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
        }
        match vars.get(&name) {
            Some(value) => out.push_str(value),
            None if braced => out.push_str(&format!("${{{name}}}")),
            None => {
                out.push('$');
                out.push_str(&name);
            }
        }
    }
    out
}

/// Оборачивает аргумент в `{}` для записи скрипта обратно в текст.
pub fn braced(text: &str) -> String {
    format!("{{{text}}}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn разбиение_по_точке_с_запятой() {
        assert_eq!(
            split_commands("вз все; #alias {a} {b;c} ;; см"),
            ["вз все", "#alias {a} {b;c}", "см"]
        );
    }

    #[test]
    fn аргументы() {
        let (name, args) =
            parse_hash_command("#action TEXT {^Вы {не} упали} {встать;см} {5} {default}").unwrap();
        assert_eq!(name, "action");
        assert_eq!(
            args,
            ["TEXT", "^Вы {не} упали", "встать;см", "5", "default"]
        );

        let (name, args) = parse_hash_command("#var target %0").unwrap();
        assert_eq!(name, "var");
        assert_eq!(args, ["target", "%0"]);

        let (name, args) = parse_hash_command("#if{$a == 1}{x}").unwrap();
        assert_eq!(name, "if");
        assert_eq!(args, ["$a == 1", "x"]);
    }

    #[test]
    fn параметры_и_переменные() {
        let params = vec!["все".to_string(), "меч".to_string()];
        assert_eq!(
            substitute_params("взять %1 (%0) %5", &params),
            "взять меч (все) "
        );

        let mut vars = BTreeMap::new();
        vars.insert("tank".to_string(), "Вася".to_string());
        assert_eq!(
            substitute_vars("гг $tank, ${tank}ну $нет", &vars),
            "гг Вася, Васяну $нет"
        );
    }
}
