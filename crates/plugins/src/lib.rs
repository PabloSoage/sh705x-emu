//! Plugins: WebAssembly modules the emulator runs beside the ECU, each in its own sandbox.
//!
//! A plugin is one `.wasm` file. Its manifest (name, version, the memory it reads) is JSON
//! in a custom section called `sh705x-plugin` ([`manifest`]), so there is nothing to keep
//! beside it. It is run by an interpreter ([`host`]) with a budget of work per call: a
//! plugin that loops forever is stopped and set aside, not the emulator.
//!
//! What a plugin can do is what it imports from the module `sh705x` (API version 1):
//!
//! | import | does |
//! |---|---|
//! | `send(id: i32, flags: i32, len: i32, data: i64, delay_us: i64)` | puts a frame on the ECU's CAN bus (HCAN0), `delay_us` from now |
//! | `read(addr: i32) -> i32` | a byte of the memory its manifest says it reads, as of this call; -1 elsewhere |
//! | `write(addr: i32, value: i32)` | writes a byte of RAM, after this call |
//! | `log(ptr: i32, len: i32)` | a line of UTF-8 text from its memory, for the user |
//! | `now_us() -> i64` | ECU time, microseconds since power-on |
//!
//! and what it exports, each optional:
//!
//! | export | called |
//! |---|---|
//! | `init()` | once, when loaded |
//! | `on_frame(id: i32, flags: i32, len: i32, data: i64)` | for every frame the ECU sends |
//! | `tick(now_us: i64)` | every `tick_us` of ECU time (its manifest; 1000 if not said) |
//! | `memory` | needed for `log` |
//!
//! A frame's `flags` are bit 0 extended (29-bit) identifier and bit 1 remote frame; its
//! `data` holds the bytes little-endian, byte 0 in the low 8 bits.
//!
//! [`library`] keeps the plugins installed and the places they come from: a forge
//! (GitHub, GitLab, Gitea) over HTTPS with a token, or any HTTPS server, each with its own
//! credential, as opendash does with its catalogues.

pub mod host;
pub mod library;
pub mod manifest;

pub use host::{Host, Status};
pub use library::{Entry, Kind, Library, Source};
pub use manifest::Manifest;

/// The API version this emulator offers.
pub const API: u32 = 1;
