//! Где сейчас находится игрок.

use crate::map::RoomInfo;

/// Отслеживает текущую и предыдущую комнату игрока по сообщениям MSDP `ROOM`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tracker {
    current: Option<u32>,
    previous: Option<u32>,
}

impl Tracker {
    /// Трекер, который пока не знает, где игрок.
    pub fn new() -> Tracker {
        Tracker::default()
    }

    /// Текущая комната.
    pub fn current(&self) -> Option<u32> {
        self.current
    }

    /// Комната, где игрок был до текущей.
    pub fn previous(&self) -> Option<u32> {
        self.previous
    }

    /// Учесть сообщение `ROOM`. Возвращает `true`, если игрок сменил комнату.
    ///
    /// Повтор той же комнаты подряд игнорируется: Былины шлют `ROOM` дважды
    /// на «смотреть». Побочный эффект: если выход ведёт в ту же комнату,
    /// такой «шаг на месте» тоже не считается переходом.
    pub fn update(&mut self, info: &RoomInfo) -> bool {
        if self.current == Some(info.vnum) {
            return false;
        }
        self.previous = self.current;
        self.current = Some(info.vnum);
        true
    }

    /// Забыть положение (например, при разрыве связи).
    pub fn reset(&mut self) {
        self.current = None;
        self.previous = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(vnum: u32) -> RoomInfo {
        RoomInfo {
            vnum,
            ..RoomInfo::default()
        }
    }

    #[test]
    fn ignores_duplicate_room_messages() {
        let mut tracker = Tracker::new();
        assert!(tracker.update(&room(4014)));
        assert!(!tracker.update(&room(4014)));
        assert_eq!(tracker.current(), Some(4014));
        assert_eq!(tracker.previous(), None);

        assert!(tracker.update(&room(4011)));
        assert!(!tracker.update(&room(4011)));
        assert_eq!(tracker.current(), Some(4011));
        assert_eq!(tracker.previous(), Some(4014));

        tracker.reset();
        assert_eq!(tracker.current(), None);
    }
}
