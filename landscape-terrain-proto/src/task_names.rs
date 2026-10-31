//! All dedicated OS-thread names for this crate.

pub const TRANSPORT_RX: &str = "terrain-rx";

#[cfg(test)]
pub mod tests {
    pub const THREAD: &str = "terrain-test";
    pub const RUNTIME: &str = "terrain-test";
    pub const RESULT: &str = "terrain.test.result";
    pub const PENDING: &str = "terrain.test.pending";
}
