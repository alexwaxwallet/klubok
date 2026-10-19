//! Состояние персонажа по MSDP: жизнь, бодрость, мана, уровень, опыт, деньги, группа.
//!
//! Былины присылают это кусками в разных переменных (см. `docs/bylins-protocol.md`):
//! `STATE` (текущие жизнь/бодрость/мана — только по запросу `SEND`), `MAX_HIT`, `MAX_MOVE`,
//! `LEVEL`, `EXPERIENCE`, `GOLD` и `GROUP`. Здесь они собираются в одну структуру,
//! которую интерфейс просто рисует.

use klubok_proto::MsdpValue;
use serde::Serialize;

/// Переменные, которые сервер сам присылает при изменении (после `REPORT`).
pub const REPORTED: &[&str] = &[
    "MAX_HIT",
    "MAX_MOVE",
    "LEVEL",
    "EXPERIENCE",
    "GOLD",
    "GROUP",
];

/// Всё, что известно о персонаже. `None` — сервер ещё не сообщил.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CharStatus {
    pub hp: Option<i64>,
    pub max_hp: Option<i64>,
    pub moves: Option<i64>,
    pub max_moves: Option<i64>,
    /// Мана есть только у «маначных» классов; максимум сервер не сообщает.
    pub mana: Option<i64>,
    pub level: Option<i64>,
    pub exp: Option<i64>,
    pub gold: Option<i64>,
    pub bank: Option<i64>,
    pub group: Vec<GroupMember>,
}

/// Член группы (включая самого игрока и чармисов).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct GroupMember {
    pub name: String,
    /// Жизнь в процентах (сервер округляет до десятков).
    pub health: i64,
    /// Бодрость в процентах.
    pub moves: i64,
    /// В той же комнате, что и игрок.
    pub here: bool,
    /// Сколько секунд до конца запоминания заклинаний (0 — всё выучено, −1 — не учится).
    pub mem_time: i64,
    /// Буквы аффектов: О — освящение, П — призм. аура, Д — дыхание водой, Н — невидимость,
    /// С — свет, Л — полёт, В — верхом, Т — чарм скоро спадёт.
    pub affects: String,
    /// `leader`, `pc` или `npc`.
    pub role: String,
    /// Положение: «Стоит», «Сражается»…
    pub position: String,
}

impl CharStatus {
    /// Учесть переменную MSDP. Возвращает `true`, если что-то поменялось.
    pub fn update(&mut self, name: &str, value: &MsdpValue) -> bool {
        let before = self.clone();
        match name {
            "STATE" => {
                self.hp = number(value.get("CURRENT_HP")).or(self.hp);
                self.moves = number(value.get("CURRENT_MOVE")).or(self.moves);
                // Нет CURRENT_MANA — класс без маны.
                self.mana = number(value.get("CURRENT_MANA"));
            }
            "MAX_HIT" => self.max_hp = number(Some(value)),
            "MAX_MOVE" => self.max_moves = number(Some(value)),
            "LEVEL" => self.level = number(Some(value)),
            "EXPERIENCE" => self.exp = number(Some(value)),
            "GOLD" => {
                self.gold = number(value.get("POCKET")).or(self.gold);
                self.bank = number(value.get("BANK")).or(self.bank);
            }
            "GROUP" => {
                self.group = match value {
                    MsdpValue::Array(items) => items.iter().map(member).collect(),
                    _ => Vec::new(),
                }
            }
            _ => return false,
        }
        *self != before
    }
}

fn number(value: Option<&MsdpValue>) -> Option<i64> {
    value?.as_str()?.trim().parse().ok()
}

fn member(value: &MsdpValue) -> GroupMember {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(MsdpValue::as_str)
            .unwrap_or_default()
            .to_string()
    };
    GroupMember {
        name: text("NAME"),
        health: number(value.get("HEALTH")).unwrap_or(0),
        moves: number(value.get("MOVE")).unwrap_or(0),
        here: text("IS_HERE") == "1",
        mem_time: number(value.get("MEM_TIME")).unwrap_or(0),
        affects: text("AFFECTS"),
        role: text("ROLE"),
        position: text("POSITION"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(t: &str) -> MsdpValue {
        MsdpValue::String(t.to_string())
    }

    #[test]
    fn собирает_состояние_из_кусков() {
        let mut st = CharStatus::default();
        assert!(st.update(
            "STATE",
            &MsdpValue::Table(vec![
                ("CURRENT_HP".into(), s("120")),
                ("CURRENT_MOVE".into(), s("80")),
            ])
        ));
        assert!(st.update("MAX_HIT", &s("150")));
        assert!(
            !st.update("MAX_HIT", &s("150")),
            "то же значение — без изменений"
        );
        assert!(!st.update("ROOM", &s("x")));
        assert_eq!(
            (st.hp, st.max_hp, st.moves, st.mana),
            (Some(120), Some(150), Some(80), None)
        );

        st.update(
            "GROUP",
            &MsdpValue::Array(vec![MsdpValue::Table(vec![
                ("NAME".into(), s("Колдун")),
                ("HEALTH".into(), s("90")),
                ("MOVE".into(), s("100")),
                ("IS_HERE".into(), s("1")),
                ("AFFECTS".into(), s("ОЛ")),
                ("ROLE".into(), s("leader")),
            ])]),
        );
        assert_eq!(st.group.len(), 1);
        assert_eq!(st.group[0].name, "Колдун");
        assert!(st.group[0].here);
        assert_eq!(st.group[0].health, 90);
    }
}
