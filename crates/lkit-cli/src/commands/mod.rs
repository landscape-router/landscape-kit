pub mod backup;
pub mod check;
mod existing;
pub mod install;
mod manage;
pub mod migrate;
pub mod network;
pub mod reconcile;
pub mod reinit;
pub mod repair;
pub mod restore;
pub mod self_service;
pub mod service_manager;
pub mod set_mirror;
pub mod software;
pub mod switch;
pub mod uninstall;
pub mod update;

use clap::Subcommand;

pub use backup::Backup;
pub use check::Check;
pub use install::Install;
pub(crate) use manage::ServiceManagerArg;
pub use migrate::Migrate;
pub use network::Network;
pub use reconcile::Reconcile;
pub use reinit::Reinit;
pub use repair::Repair;
pub use restore::Restore;
pub use self_service::SelfService;
pub use service_manager::ServiceManager;
pub use set_mirror::SetMirror;
pub use software::Software;
pub use switch::Switch;
pub use uninstall::Uninstall;
pub use update::Update;

#[derive(Debug, Subcommand)]
pub enum Commands {
    Check(Check),
    Install(Install),
    Migrate(Migrate),
    Network(Network),
    Switch(Switch),
    Update(Update),
    Repair(Repair),
    Restore(Restore),
    Reinit(Reinit),
    Backup(Backup),
    Reconcile(Reconcile),
    ServiceManager(ServiceManager),
    SetMirror(SetMirror),
    Software(Software),
    Uninstall(Uninstall),
    SelfService(SelfService),
    Daemon(crate::daemon::Daemon),
}
