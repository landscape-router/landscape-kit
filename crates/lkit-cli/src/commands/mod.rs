pub mod check;
mod existing;
pub mod install;
mod manage;
pub mod reconcile;
pub mod repair;
pub mod service_manager;
pub mod switch;

use clap::Subcommand;

pub use check::Check;
pub use install::Install;
pub(crate) use manage::ServiceManagerArg;
pub use reconcile::Reconcile;
pub use repair::Repair;
pub use service_manager::ServiceManager;
pub use switch::Switch;

#[derive(Debug, Subcommand)]
pub enum Commands {
    Check(Check),
    Install(Install),
    Switch(Switch),
    Repair(Repair),
    Reconcile(Reconcile),
    ServiceManager(ServiceManager),
}
