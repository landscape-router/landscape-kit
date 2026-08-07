pub mod check;
mod existing;
pub mod install;
mod manage;
pub mod network;
pub mod reconcile;
pub mod repair;
pub mod service_manager;
pub mod switch;
pub mod update;

use clap::Subcommand;

pub use check::Check;
pub use install::Install;
pub(crate) use manage::ServiceManagerArg;
pub use network::Network;
pub use reconcile::Reconcile;
pub use repair::Repair;
pub use service_manager::ServiceManager;
pub use switch::Switch;
pub use update::Update;

#[derive(Debug, Subcommand)]
pub enum Commands {
    Check(Check),
    Install(Install),
    Network(Network),
    Switch(Switch),
    Update(Update),
    Repair(Repair),
    Reconcile(Reconcile),
    ServiceManager(ServiceManager),
}
