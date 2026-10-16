//! Автоматизация: всё, что пользователь настраивает поверх потока от сервера.
//!
//! Триггеры (`#action`), алиасы, подсветка, переменные, горячие клавиши,
//! замены и скрытие строк, схлопывание пустых строк, импорт настроек JMC.

pub mod blank;
pub mod color;
pub mod engine;
pub mod expr;
pub mod pattern;
pub mod script;

pub use blank::{BlankLineFilter, BlankLineSettings};
pub use engine::{Automation, Effect, Hotkey, ImportReport};
