#![allow(dead_code, unused_imports)]

pub(crate) mod health;
pub(crate) mod manager;
pub(crate) mod openrc;
pub(crate) mod preflight;
pub(crate) mod process;
pub(crate) mod resolv;
pub(crate) mod systemd;
pub(crate) mod sysvinit;

pub(crate) use crate::deployment::{plan, root, state, transaction};
pub(crate) use crate::workflows::install as pipeline;
