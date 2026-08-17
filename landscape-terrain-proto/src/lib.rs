//! Landscape proto: the shared layer-2 protocol stack.
//!
//! - `protocol`: Terrain frame encoding/decoding and the client/server session
//!   state machines;
//! - `transport`: ethernet frame parsing plus the platform link layer
//!   (Linux AF_PACKET, elsewhere libpcap), exposed as an async `Link`;
//! - `cli`: shared argument parsers for the `landscape-client` and
//!   `landscape-server` binaries.

pub mod cli;
pub mod ipstack;
pub mod protocol;
pub mod transport;
