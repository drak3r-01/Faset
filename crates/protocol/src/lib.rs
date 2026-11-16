//! Wire format: bit writer/reader, quantizers and the message enum.
#![forbid(unsafe_code)]

pub mod bits;
pub mod message;
pub mod quant;

pub use bits::{BitReader, BitWriter};
pub use message::{
    DecodeError, EncodeError, Event, InputCommand, Message, MAX_PACKET_BYTES, PROTOCOL_VERSION,
};
