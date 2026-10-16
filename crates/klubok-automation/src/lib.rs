//! Автоматизация: всё, что пользователь настраивает поверх потока от сервера.
//!
//! Триггеры (`#action`), алиасы, подсветка, переменные, горячие клавиши,
//! замены и скрытие строк, схлопывание пустых строк, импорт настроек JMC,
//! таймеры и тикер (`#5:10 {…}`, `#wait`, `#tickon`), скрипты `#script` на JavaScript.

pub mod blank;
pub mod color;
pub mod engine;
pub mod expr;
pub mod js;
pub mod merge;
pub mod pattern;
pub mod script;
pub mod timers;

pub use blank::{BlankLineFilter, BlankLineSettings};
pub use engine::{Automation, CLIENT_COMMANDS, Effect, Hotkey, ImportReport};
