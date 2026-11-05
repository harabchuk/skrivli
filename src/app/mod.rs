//! Application composition. The GPUI thread does not scan disks or join workers.

mod bootstrap;
pub(crate) mod bridge;
mod menus;
mod runtime;
pub(crate) mod settings_writer;
pub(crate) mod shutdown;

pub(crate) use bootstrap::run;
