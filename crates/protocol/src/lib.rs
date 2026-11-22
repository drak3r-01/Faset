//! Wire format: bit writer/reader, quantizers and the message enum.
#![forbid(unsafe_code)]

pub mod bits;
pub mod message;
pub mod quant;

pub use bits::{BitReader, BitWriter};
pub use message::{
    DecodeError, EncodeError, Event, InputCommand, Message, ProjectileSpawn, BUTTON_CROUCH,
    BUTTON_FIRE, BUTTON_JUMP, BUTTON_RELOAD, BUTTON_SIGHT, MAX_EVENT_BYTES, MAX_PACKET_BYTES,
    PROTOCOL_VERSION,
};
