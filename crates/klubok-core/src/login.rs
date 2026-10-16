//! Автовход персонажем.
//!
//! Былины на запросе имени принимают сразу «Имя Пароль» одной строкой.
//! После входа сервер просит «нажмите ENTER» — нажимаем сами.

/// Учётные данные персонажа.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub name: String,
    pub password: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Ждём запрос имени.
    WaitName,
    /// Имя и пароль отправлены, ждём «нажмите ENTER» или ошибку.
    Sent,
    /// Вход завершён (или не удался) — больше не вмешиваемся.
    Done,
}

/// Что сделать в ответ на строку от сервера.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginStep {
    /// Отправить текст как есть (без алиасов и `#`-команд) и не показывать его.
    SendSecret(String),
    /// Отправить текст как есть.
    Send(String),
    /// Сообщить игроку.
    Notice(String),
}

#[derive(Debug, Clone)]
pub struct AutoLogin {
    credentials: Credentials,
    stage: Stage,
}

impl AutoLogin {
    pub fn new(credentials: Credentials) -> AutoLogin {
        AutoLogin {
            credentials,
            stage: Stage::WaitName,
        }
    }

    /// Вход завершён — можно запускать триггеры.
    pub fn is_done(&self) -> bool {
        self.stage == Stage::Done
    }

    /// Проверяет текст строки или промпта.
    pub fn check(&mut self, text: &str) -> Vec<LoginStep> {
        let lower = text.to_lowercase();
        match self.stage {
            Stage::WaitName if lower.contains("введите имя") => {
                self.stage = Stage::Sent;
                vec![
                    LoginStep::SendSecret(format!(
                        "{} {}",
                        self.credentials.name, self.credentials.password
                    )),
                    LoginStep::Notice(format!("Автовход: {}", self.credentials.name)),
                ]
            }
            Stage::Sent
                if lower.contains("неверный пароль") || lower.contains("неправильный пароль") =>
            {
                self.stage = Stage::Done;
                vec![LoginStep::Notice(
                    "Автовход: сервер не принял пароль. Проверьте его в профиле.".into(),
                )]
            }
            // Сервер спросил пароль отдельно — значит, вход одной строкой не сработал.
            Stage::Sent if lower.contains("пароль") && lower.trim_end().ends_with(':') => {
                self.stage = Stage::Done;
                vec![LoginStep::SendSecret(self.credentials.password.clone())]
            }
            Stage::Sent if lower.contains("нажмите enter") || lower.contains("anykey") => {
                self.stage = Stage::Done;
                vec![LoginStep::Send(String::new())]
            }
            // Уже в игре: переподключение без «нажмите ENTER» или игровой промпт («…Вых:СВ>»).
            Stage::Sent if lower.contains("пересоединяемся") || lower.trim_end().ends_with('>') =>
            {
                self.stage = Stage::Done;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login() -> AutoLogin {
        AutoLogin::new(Credentials {
            name: "Тестер".into(),
            password: "tester2026".into(),
        })
    }

    #[test]
    fn обычный_вход() {
        let mut l = login();
        assert!(l.check("ПО ПРЕДАНИЯМ РУССКИХ НАРОДНЫХ СКАЗОК").is_empty());
        let steps = l.check("Введите имя персонажа (или \"новый\" для создания нового): ");
        assert_eq!(steps[0], LoginStep::SendSecret("Тестер tester2026".into()));
        let steps = l.check("* В связи с проблемами перевода фразы ANYKEY нажмите ENTER *");
        assert_eq!(steps, [LoginStep::Send(String::new())]);
        // Дальше не вмешиваемся.
        assert!(l.is_done());
        assert!(l.check("Введите имя").is_empty());
    }

    #[test]
    fn игровой_промпт_завершает_вход() {
        let mut l = login();
        l.check("Введите имя персонажа: ");
        assert!(!l.is_done());
        l.check("262H 82M ??? Зауч:0 Вых:СВЮv> ");
        assert!(l.is_done());
    }

    #[test]
    fn отдельный_запрос_пароля() {
        let mut l = login();
        l.check("Введите имя персонажа: ");
        assert_eq!(
            l.check("Пароль: "),
            [LoginStep::SendSecret("tester2026".into())]
        );
    }

    #[test]
    fn неверный_пароль() {
        let mut l = login();
        l.check("Введите имя персонажа: ");
        let steps = l.check("Неверный пароль.");
        assert!(matches!(&steps[0], LoginStep::Notice(_)));
        assert!(l.check("Пароль: ").is_empty());
    }
}
