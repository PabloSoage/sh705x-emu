//! Write a plugin for sh705x-emu in Rust (API 1; see `crates/plugins` for the contract).
//!
//! ```ignore
//! use sh705x_plugin::{Frame, manifest, send};
//!
//! manifest!(r#"{"name": "A node", "version": "1.0.0", "api": 1, "tick_us": 10000}"#);
//!
//! #[unsafe(no_mangle)]
//! pub extern "C" fn tick(_now_us: i64) {
//!     send(&Frame::standard(0x123, &[1, 2, 3]), 0);
//! }
//!
//! #[unsafe(no_mangle)]
//! pub extern "C" fn on_frame(id: i32, flags: i32, len: i32, data: i64) {
//!     let f = Frame::from_args(id, flags, len, data);
//!     // ...
//! }
//! ```
//!
//! Build it with `cargo build --release --target wasm32-unknown-unknown`, with
//! `crate-type = ["cdylib"]`; the `.wasm` in `target/wasm32-unknown-unknown/release` is the
//! plugin.

#[allow(missing_docs)]
mod sys {
    #[link(wasm_import_module = "sh705x")]
    unsafe extern "C" {
        pub fn send(id: i32, flags: i32, len: i32, data: i64, delay_us: i64);
        pub fn read(addr: i32) -> i32;
        pub fn write(addr: i32, value: i32);
        pub fn log(ptr: i32, len: i32);
        pub fn now_us() -> i64;
    }
}

/// A CAN frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Identifier: 11 bits, or 29 if `extended`.
    pub id: u32,
    /// 29-bit identifier.
    pub extended: bool,
    /// Remote frame.
    pub remote: bool,
    /// Bytes in use, 0 to 8.
    pub len: u8,
    /// The bytes.
    pub data: [u8; 8],
}

impl Frame {
    /// A standard data frame with up to 8 bytes.
    pub fn standard(id: u32, bytes: &[u8]) -> Self {
        let mut data = [0; 8];
        let n = bytes.len().min(8);
        data[..n].copy_from_slice(&bytes[..n]);
        Self {
            id: id & 0x7FF,
            extended: false,
            remote: false,
            len: n as u8,
            data,
        }
    }

    /// The frame `on_frame` was given.
    pub fn from_args(id: i32, flags: i32, len: i32, data: i64) -> Self {
        Self {
            id: id as u32,
            extended: flags & 1 != 0,
            remote: flags & 2 != 0,
            len: len.clamp(0, 8) as u8,
            data: (data as u64).to_le_bytes(),
        }
    }

    /// The bytes in use.
    pub fn bytes(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }
}

/// Puts `f` on the ECU's CAN bus, `delay_us` of ECU time from now.
pub fn send(f: &Frame, delay_us: u64) {
    let flags = f.extended as i32 | (f.remote as i32) << 1;
    let data = u64::from_le_bytes(f.data) as i64;
    // SAFETY: an import of the emulator, taking plain numbers.
    unsafe { sys::send(f.id as i32, flags, f.len as i32, data, delay_us as i64) }
}

/// A byte of the ECU's memory, as of this call, if the manifest's `reads` cover it.
pub fn read(addr: u32) -> Option<u8> {
    // SAFETY: as `send`.
    let v = unsafe { sys::read(addr as i32) };
    (v >= 0).then_some(v as u8)
}

/// A big-endian word of the ECU's memory, if covered.
pub fn read_u16(addr: u32) -> Option<u16> {
    Some(u16::from_be_bytes([read(addr)?, read(addr + 1)?]))
}

/// A big-endian long of the ECU's memory, if covered.
pub fn read_u32(addr: u32) -> Option<u32> {
    Some((read_u16(addr)? as u32) << 16 | read_u16(addr + 2)? as u32)
}

/// A big-endian float of the ECU's memory, if covered.
pub fn read_f32(addr: u32) -> Option<f32> {
    read_u32(addr).map(f32::from_bits)
}

/// Writes a byte of the ECU's RAM, once this call returns.
pub fn write(addr: u32, value: u8) {
    // SAFETY: as `send`.
    unsafe { sys::write(addr as i32, value as i32) }
}

/// A line for the user.
pub fn log(text: &str) {
    // SAFETY: the emulator reads `len` bytes at `ptr` of this module's memory, during the
    // call; `text` lives through it.
    unsafe { sys::log(text.as_ptr() as i32, text.len() as i32) }
}

/// ECU time, microseconds since power-on.
pub fn now_us() -> u64 {
    // SAFETY: as `send`.
    unsafe { sys::now_us() as u64 }
}

/// The plugin's manifest, from a JSON string literal, in the section the emulator reads.
#[macro_export]
macro_rules! manifest {
    ($json:expr) => {
        #[unsafe(link_section = "sh705x-plugin")]
        #[used]
        static SH705X_PLUGIN_MANIFEST: [u8; $json.len()] = {
            const S: &[u8] = $json.as_bytes();
            let mut a = [0u8; S.len()];
            let mut i = 0;
            while i < S.len() {
                a[i] = S[i];
                i += 1;
            }
            a
        };
    };
}
