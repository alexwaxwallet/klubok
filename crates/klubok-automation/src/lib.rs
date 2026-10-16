//! Автоматизация: всё, что пользователь настраивает поверх потока от сервера.
//!
//! Триггеры (`#action`), алиасы, подсветка, переменные, таймеры,
//! rewrite-правила (gag / sub / merge / collapse, схлопывание пустых строк)
//! и импорт настроек JMC.

pub mod blank;

pub use blank::{BlankLineFilter, BlankLineSettings};
