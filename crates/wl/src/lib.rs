//! Wayland wire protocol (both directions) + generated protocol bindings.
pub mod wire;

#[allow(clippy::all)]
pub mod protocol {
    include!(concat!(env!("OUT_DIR"), "/protocol.rs"));
}

pub use protocol::Iface;
pub use wire::{Args, Conn, Fixed, NewIdDyn, ObjId, WireError};
