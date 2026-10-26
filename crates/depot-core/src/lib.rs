//! Depot's inventory engine: finds installed software and works out how each piece was installed.
//! Has no GTK dependency so it can be tested and benchmarked on its own.

pub mod backend;
pub mod command;
pub mod desktop;
pub mod exec;
pub(crate) mod group;
pub mod inventory;
pub mod model;
pub(crate) mod owners;
pub mod remove;
pub mod updates;
pub(crate) mod xdg;

pub use inventory::{Env, Inventory, scan};
pub use model::{Kind, Package, Role};
pub use updates::Update;
