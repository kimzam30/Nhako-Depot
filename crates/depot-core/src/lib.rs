//! Depot's inventory engine: finds installed software and works out how each piece was installed.
//! Has no GTK dependency so it can be tested and benchmarked on its own.

pub mod backend;
pub mod desktop;
pub mod exec;
pub mod inventory;
pub mod model;
pub mod updates;
pub mod xdg;

pub use inventory::{Env, Inventory, scan};
pub use model::{Kind, ManualPlan, Package, Role};
pub use updates::Update;
