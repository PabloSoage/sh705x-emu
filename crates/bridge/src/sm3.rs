//! A Scanmatik SM3, as opendash talks to it, in front of the emulated ECU.
//!
//! The protocol is the one recovered in opendash (`docs/protocol.md`): TCP, messages of
//! a 16-byte header and their data, each padded to a multiple of 4 bytes.
//!
//! ```text
//! 0..1 seq u16 · 2..3 token u16 · 4..7 h4 u32 · 8..11 h8 u32 · 12 op · 13..14 len u16
//! 15 chk = (op + len_lo + len_hi + 0x55) & 0xff · 16.. data
//! ```
//!
//! What this side does:
//! - every message gets exactly one answer, with op 0x00;
//! - the greeting (op 0x83) is answered with 66 bytes: a serial number and a firmware
//!   word, which the client shows;
//! - op 0x20 is answered with the voltages, battery millivolts first;
//! - op 0x1C with subcommand `60 80 02` puts a CAN frame on the bus: `len u32 | id u32 |
//!   8 bytes` from data offset 3;
//! - frames from the ECU are pushed unasked, as op 0xFE blocks: a 32-byte block header,
//!   the frame count in the high nibble of byte 34, then 16-byte records `len u32 | id
//!   u32 | 8 bytes`.
//!
//! The client signs its writes with h4 (a CRC) and h8 (a sum); a real SM3 drops the
//! connection on a wrong one. This side checks neither, and signs its own answers with
//! h8 only: the client does not check them.

use crate::link::CanLink;
use sh7059::hcan::Frame;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

/// The answer to op 0x83: 66 bytes, a serial at 0 and a firmware word at 12.
pub const SERIAL: u32 = 0x705E_0001;
/// The firmware word of the greeting.
pub const FIRMWARE: u32 = 0x0001_0000;

const HEADER: usize = 16;
const OP_GREETING: u8 = 0x83;
const OP_VOLTAGES: u8 = 0x20;
const OP_BUS: u8 = 0x1C;
const OP_FRAMES: u8 = 0xFE;
/// Most frames in one pushed block: the count is a nibble.
const PER_BLOCK: usize = 15;

fn checksum(op: u8, len: usize) -> u8 {
    (op as usize + (len & 0xFF) + ((len >> 8) & 0xFF) + 0x55) as u8
}

fn stride(len: usize) -> usize {
    (HEADER + len + 3) & !3
}

/// A message, header and padded data, ready to send.
pub fn message(op: u8, data: &[u8]) -> Vec<u8> {
    let mut m = vec![0u8; stride(data.len())];
    m[0..2].copy_from_slice(&0xFFFFu16.to_le_bytes());
    // h8: the data as little-endian words, summed.
    let h8 = m.len().saturating_sub(HEADER);
    let mut padded = data.to_vec();
    padded.resize(h8, 0);
    let sum = padded
        .chunks(4)
        .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
        .fold(0u32, u32::wrapping_add);
    m[8..12].copy_from_slice(&sum.to_le_bytes());
    m[12] = op;
    m[13..15].copy_from_slice(&(data.len() as u16).to_le_bytes());
    m[15] = checksum(op, data.len());
    m[HEADER..HEADER + data.len()].copy_from_slice(data);
    m
}

/// Takes one whole message (op, data) off the front of `buf`, skipping bytes that do not
/// start one.
pub fn take(buf: &mut Vec<u8>) -> Option<(u8, Vec<u8>)> {
    loop {
        if buf.len() < HEADER {
            return None;
        }
        let op = buf[12];
        let len = u16::from_le_bytes([buf[13], buf[14]]) as usize;
        if buf[15] != checksum(op, len) {
            buf.remove(0);
            continue;
        }
        let size = stride(len);
        if buf.len() < size {
            return None;
        }
        let data = buf[HEADER..HEADER + len].to_vec();
        buf.drain(..size);
        return Some((op, data));
    }
}

/// An op 0xFE block carrying `frames` (at most 15).
pub fn block(frames: &[Frame]) -> Vec<u8> {
    let n = frames.len().min(PER_BLOCK);
    let mut d = vec![0u8; 36 + 16 * n];
    d[34] = (n as u8) << 4;
    for (i, f) in frames.iter().take(n).enumerate() {
        let o = 36 + 16 * i;
        d[o..o + 4].copy_from_slice(&(f.dlc.min(8) as u32).to_le_bytes());
        d[o + 4..o + 8].copy_from_slice(&f.id.to_le_bytes());
        d[o + 8..o + 16].copy_from_slice(&f.data);
    }
    message(OP_FRAMES, &d)
}

/// The adapter's state for one client.
#[derive(Clone, Debug)]
pub struct Sm3 {
    /// Battery voltage it reports, millivolts.
    pub battery_mv: u16,
    /// CAN frames written by the client.
    pub written: u64,
    /// Frames pushed to the client.
    pub pushed: u64,
}

impl Default for Sm3 {
    fn default() -> Self {
        Self {
            battery_mv: 12_600,
            written: 0,
            pushed: 0,
        }
    }
}

impl Sm3 {
    /// The answer to one message; a CAN write goes to `link` first.
    pub fn answer(&mut self, op: u8, data: &[u8], link: &mut dyn CanLink) -> Vec<u8> {
        match op {
            OP_GREETING => {
                let mut d = vec![0u8; 66];
                d[0..4].copy_from_slice(&SERIAL.to_le_bytes());
                d[12..16].copy_from_slice(&FIRMWARE.to_le_bytes());
                message(0, &d)
            }
            OP_VOLTAGES => {
                let mut d = [0u8; 6];
                d[0..2].copy_from_slice(&self.battery_mv.to_le_bytes());
                message(0, &d)
            }
            OP_BUS if data.len() >= 19 && data[0] == 0x60 => {
                let len = u32::from_le_bytes([data[3], data[4], data[5], data[6]]).min(8);
                let id = u32::from_le_bytes([data[7], data[8], data[9], data[10]]);
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&data[11..19]);
                link.send(Frame {
                    id: id & 0x1FFF_FFFF,
                    extended: id > 0x7FF,
                    remote: false,
                    dlc: len as u8,
                    data: bytes,
                });
                self.written += 1;
                message(0, &[0u8; 28])
            }
            OP_BUS => message(0, &[0u8; 28]),
            _ => message(0, &[0u8; 4]),
        }
    }
}

/// Serves SM3 clients on `listener`, one at a time, until it fails.
pub fn serve(
    listener: TcpListener,
    link: &mut dyn CanLink,
    log: &mut dyn FnMut(&str),
) -> std::io::Result<()> {
    for stream in listener.incoming() {
        let stream = stream?;
        let peer = stream
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_default();
        log(&format!("SM3 client connected: {peer}"));
        let mut sm3 = Sm3::default();
        match session(stream, &mut sm3, link) {
            Ok(()) => {}
            Err(e) => log(&format!("{peer}: {e}")),
        }
        log(&format!(
            "SM3 client gone: {peer}; {} frames written, {} pushed",
            sm3.written, sm3.pushed
        ));
    }
    Ok(())
}

/// One client: answers its messages and pushes the ECU's frames, until it hangs up.
pub fn session(
    mut stream: TcpStream,
    sm3: &mut Sm3,
    link: &mut dyn CanLink,
) -> std::io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_millis(2)))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(()),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Err(e),
        }
        while let Some((op, data)) = take(&mut buf) {
            let a = sm3.answer(op, &data, link);
            stream.write_all(&a)?;
        }
        let mut frames = Vec::new();
        while frames.len() < PER_BLOCK {
            match link.recv(0) {
                Some(f) => frames.push(f),
                None => break,
            }
        }
        if !frames.is_empty() {
            sm3.pushed += frames.len() as u64;
            stream.write_all(&block(&frames))?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// The recorded opendash messages this side has to take.
    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[derive(Default)]
    struct Bus {
        sent: Vec<Frame>,
        out: VecDeque<Frame>,
    }

    impl CanLink for Bus {
        fn send(&mut self, f: Frame) {
            self.sent.push(f);
        }
        fn recv(&mut self, _: u64) -> Option<Frame> {
            self.out.pop_front()
        }
        fn now_us(&self) -> u64 {
            0
        }
    }

    #[test]
    fn opendash_messages_parse_and_get_their_answers() {
        // The greeting, the bus poll and the write template, as opendash sends them.
        let mut wire = hex("ffff0000ffff000000000000830000d8");
        wire.extend(hex("ffff00005490d54a408002001c03007440800200"));
        wire.extend(hex(
            "ffff0000c69b6d40688002e91c13008460800208000000df070000020100000000000000",
        ));
        wire.extend([0xAA]); // a stray byte, then half a message
        wire.extend(&hex("ffff0000ffff00000000000020")[..13]);
        let mut sm3 = Sm3::default();
        let mut bus = Bus::default();
        let mut answers = Vec::new();
        while let Some((op, data)) = take(&mut wire) {
            answers.push(sm3.answer(op, &data, &mut bus));
        }
        assert_eq!(answers.len(), 3);
        assert_eq!(
            wire.len(),
            14,
            "the stray byte and the half message wait for more"
        );
        let greeting = take(&mut answers[0].clone()).unwrap();
        assert_eq!(greeting.0, 0, "every answer has op 0x00");
        assert_eq!(greeting.1.len(), 66);
        assert_eq!(&greeting.1[0..4], &SERIAL.to_le_bytes());
        assert_eq!(bus.sent.len(), 1, "the write went on the bus");
        let f = bus.sent[0];
        assert_eq!((f.id, f.dlc, f.extended), (0x7DF, 8, false));
        assert_eq!(&f.data[..3], &[0x02, 0x01, 0x00]);
        assert!(answers.iter().all(|a| a.len() % 4 == 0), "padded to 4");
    }

    #[test]
    fn voltages_and_pushed_blocks() {
        let mut sm3 = Sm3::default();
        let mut bus = Bus::default();
        let mut v = sm3.answer(0x20, &[], &mut bus);
        let (op, d) = take(&mut v).unwrap();
        assert_eq!((op, d.len()), (0, 6));
        assert_eq!(u16::from_le_bytes([d[0], d[1]]), 12_600);
        let frames: Vec<Frame> = (0..20)
            .map(|k| Frame::standard(0x7E8, &[k as u8, 0x41]))
            .collect();
        let mut b = block(&frames);
        let (op, d) = take(&mut b).unwrap();
        assert_eq!(op, 0xFE);
        assert_eq!(d[34] >> 4, 15, "at most 15 a block");
        assert_eq!(d.len(), 36 + 15 * 16);
        let o = 36 + 16 * 3;
        assert_eq!(u32::from_le_bytes(d[o..o + 4].try_into().unwrap()), 2);
        assert_eq!(
            u32::from_le_bytes(d[o + 4..o + 8].try_into().unwrap()),
            0x7E8
        );
        assert_eq!(d[o + 8], 3);
    }
}
