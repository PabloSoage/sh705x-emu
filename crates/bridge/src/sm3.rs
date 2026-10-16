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
//! - the rest of the opening gets the answers' lengths the car's SM3 gives: 0 for
//!   0x88, 0x50, 0x51 and 0x52, 261 for 0x30, 308 for 0x12 (both zero-filled: their
//!   layout is not captured), and the 28-byte channel status for 0x1C and 0x53;
//! - op 0x20 is answered with the voltages, battery millivolts first;
//! - op 0x1C with subcommand `60 80 02` puts a CAN frame on the bus: `len u32 | id u32 |
//!   8 bytes` from data offset 3;
//! - frames from the ECU are pushed unasked, as op 0xFE blocks: a 32-byte block header,
//!   the frame count in the high nibble of byte 34, then 16-byte records `len u32 | id
//!   u32 | 8 bytes`;
//! - a channel block (op 0x50) sets which frames are pushed, J2534 style: the speed at
//!   data offset 134 (0 closes the channel), the filter count at 142 and 59-byte filters
//!   from 305, each a mask (bit 31 set) at +0, a pattern at +4 and a type at +10 (0 pass,
//!   1 block, 2 flow control). A frame goes through when a pass or flow-control filter
//!   matches and no block filter does, so an open channel without filters passes
//!   nothing. On the car the SM3 pushes the whole bus from the moment the client
//!   connects, and nothing after a channel block with no filters [MED]; this side does
//!   the same;
//! - the adapter keeps only the latest frames when the client reads slower than the bus
//!   (the car's SM3 delivered about 600 at once on connecting [MED]), so an answer never
//!   waits behind seconds of old traffic: this side keeps [`QUEUE`] and drops the oldest.
//!
//! The client signs its writes with h4 (a CRC) and h8 (a sum); a real SM3 drops the
//! connection on a wrong one. This side checks neither, and signs its own answers with
//! h8 only: the client does not check them.

use crate::link::CanLink;
use sh7059::hcan::Frame;
use std::collections::VecDeque;
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
const OP_CHANNEL: u8 = 0x50;
/// The rest of `PassThruOpen` after the greeting (protocol.md §15): the second
/// handshake word (0x88), a network-config block (0x30) and a session-config echo
/// (0x12), then the state probes (0x51, 0x52) and the physical-layer mask (0x53).
const OP_OPEN2: u8 = 0x88;
const OP_NET: u8 = 0x30;
const OP_SESSION: u8 = 0x12;
const OP_STATE_A: u8 = 0x51;
const OP_STATE_B: u8 = 0x52;
const OP_LAYERS: u8 = 0x53;
/// Length of the network-config answer to 0x30 (IP, mask, MAC); its inner layout is
/// not captured byte for byte, so this side returns the right size, zero-filled.
const LEN_NET: usize = 261;
/// Length of the session-config echo answered to 0x12; likewise size-faithful only.
const LEN_SESSION: usize = 308;

/// The 28-byte channel-status block the adapter returns to 0x1C and 0x53 and carries
/// in every 0xFE header, constant through a session once the channel is open
/// (protocol.md §6, §15). The first word is the channel id, which on the car is zero
/// until the channel opens; this side keeps it set, which opendash and a J2534 client
/// read only after connecting anyway.
const STATUS: [u8; 28] = [
    0x80, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0x01, 0x00, 0x00,
    0xFF, 0x01, 0x00, 0x00, 0xB7, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
/// Frames waiting for the client, at most; older ones are dropped [EST].
pub const QUEUE: usize = 512;
/// Bytes written but not yet taken by the socket above which no more frames are queued.
const UNSENT: usize = 16 * 1024;

/// A channel filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Filter {
    /// 0 pass, 1 block, 2 flow control.
    pub kind: u8,
    /// Identifier bits compared.
    pub mask: u32,
    /// What they must be.
    pub pattern: u32,
}

impl Filter {
    /// Lets everything through: what the adapter does before any channel block.
    pub const PASS_ALL: Filter = Filter {
        kind: 0,
        mask: 0,
        pattern: 0,
    };

    fn matches(&self, id: u32) -> bool {
        id & self.mask == self.pattern & self.mask
    }
}

/// The filters a channel block (op 0x50) sets: `Some(None)` closes the channel, `None`
/// means the block is too short to read.
pub fn channel_filters(data: &[u8]) -> Option<Option<Vec<Filter>>> {
    const SPEED: usize = 134;
    const COUNT: usize = 142;
    const FILTERS: usize = 305;
    const SIZE: usize = 59;
    if data.len() <= COUNT {
        return None;
    }
    let u32_at = |o: usize| u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
    if u32_at(SPEED) == 0 {
        return Some(None);
    }
    let filters = (0..data[COUNT] as usize)
        .map(|k| FILTERS + k * SIZE)
        .take_while(|&o| o + SIZE <= data.len())
        .map(|o| Filter {
            mask: u32_at(o) & 0x1FFF_FFFF,
            pattern: u32_at(o + 4) & 0x1FFF_FFFF,
            kind: data[o + 10],
        })
        .collect();
    Some(Some(filters))
}

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
    d[0..4].copy_from_slice(&[0x13, 0x00, 0x1C, 0x00]);
    d[4..32].copy_from_slice(&STATUS);
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
    /// Frames dropped because the client read slower than the bus.
    pub dropped: u64,
    /// The open channel's filters; `None` when the channel is closed.
    pub filters: Option<Vec<Filter>>,
}

impl Default for Sm3 {
    fn default() -> Self {
        Self {
            battery_mv: 12_600,
            written: 0,
            pushed: 0,
            dropped: 0,
            filters: Some(vec![Filter::PASS_ALL]),
        }
    }
}

impl Sm3 {
    /// Whether a frame from the bus reaches the client.
    pub fn passes(&self, f: &Frame) -> bool {
        let Some(filters) = &self.filters else {
            return false;
        };
        !filters.iter().any(|x| x.kind == 1 && x.matches(f.id))
            && filters.iter().any(|x| x.kind != 1 && x.matches(f.id))
    }

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
                message(0, &STATUS)
            }
            OP_BUS | OP_LAYERS => message(0, &STATUS),
            OP_CHANNEL => {
                if let Some(f) = channel_filters(data) {
                    self.filters = f;
                }
                message(0, &[])
            }
            OP_OPEN2 | OP_STATE_A | OP_STATE_B => message(0, &[]),
            OP_NET => message(0, &[0u8; LEN_NET]),
            OP_SESSION => message(0, &[0u8; LEN_SESSION]),
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
            "SM3 client gone: {peer}; {} frames written, {} pushed, {} dropped",
            sm3.written, sm3.pushed, sm3.dropped
        ));
    }
    Ok(())
}

/// One client: answers its messages and pushes the ECU's frames, until it hangs up.
///
/// The socket never blocks this side: answers always go out, frames only while the
/// client keeps up, and the bus is read all the time into a queue of the latest
/// [`QUEUE`] frames.
pub fn session(
    mut stream: TcpStream,
    sm3: &mut Sm3,
    link: &mut dyn CanLink,
) -> std::io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_nonblocking(true)?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut out: Vec<u8> = Vec::new();
    let mut sent = 0usize;
    let mut queue: VecDeque<Frame> = VecDeque::new();
    loop {
        let mut busy = false;
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                busy = true;
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
        while let Some((op, data)) = take(&mut buf) {
            out.extend(sm3.answer(op, &data, link));
        }
        while let Some(f) = link.recv(0) {
            if sm3.passes(&f) {
                if queue.len() == QUEUE {
                    queue.pop_front();
                    sm3.dropped += 1;
                }
                queue.push_back(f);
            }
        }
        while out.len() - sent < UNSENT && !queue.is_empty() {
            let n = queue.len().min(PER_BLOCK);
            let frames: Vec<Frame> = queue.drain(..n).collect();
            sm3.pushed += n as u64;
            out.extend(block(&frames));
        }
        if sent < out.len() {
            match stream.write(&out[sent..]) {
                Ok(n) => {
                    sent += n;
                    busy |= n > 0;
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
            if sent == out.len() {
                out.clear();
                sent = 0;
            }
        }
        if !busy {
            std::thread::sleep(Duration::from_micros(500));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_opening_gets_the_answer_lengths_of_the_car() {
        // op -> length of the 0x00 answer, as the car's SM3 gave them (protocol.md §15).
        let mut sm3 = Sm3::default();
        let mut bus = Bus::default();
        for (op, len) in [
            (0x83u8, 66usize),
            (0x88, 0),
            (0x30, 261),
            (0x12, 308),
            (0x50, 0),
            (0x51, 0),
            (0x52, 0),
            (0x53, 28),
            (0x1C, 28),
            (0x20, 6),
        ] {
            let mut a = sm3.answer(op, &[], &mut bus);
            let (got, d) = take(&mut a).unwrap();
            assert_eq!((got, d.len()), (0, len), "op {op:#04x}");
            assert!(a.is_empty(), "op {op:#04x}: one message");
        }
        let mut s = sm3.answer(0x53, &[0x30, 0x00], &mut bus);
        let (_, d) = take(&mut s).unwrap();
        let mut b = block(&[]);
        let (_, h) = take(&mut b).unwrap();
        assert_eq!(&h[4..32], &d[..], "the status is the 0xFE header's");
        assert_eq!(&h[0..4], &[0x13, 0x00, 0x1C, 0x00]);
    }

    #[test]
    fn channel_blocks_set_the_filters() {
        // The layout of a channel block, as `79-filtros-de-canal.mjs` builds it.
        let mut d = vec![0u8; 305 + 2 * 59];
        d[134..138].copy_from_slice(&500_000u32.to_le_bytes());
        d[142] = 2;
        for (k, (kind, mask, pattern)) in [(0u8, 0x7F8u32, 0x7E8u32), (1, 0x7FF, 0x7EA)]
            .into_iter()
            .enumerate()
        {
            let o = 305 + 59 * k;
            d[o..o + 4].copy_from_slice(&(mask | 0x8000_0000).to_le_bytes());
            d[o + 4..o + 8].copy_from_slice(&pattern.to_le_bytes());
            d[o + 10] = kind;
        }
        let mut sm3 = Sm3::default();
        assert!(
            sm3.passes(&Frame::standard(0x0C9, &[0])),
            "before any channel: everything"
        );
        let mut bus = Bus::default();
        sm3.answer(0x50, &d, &mut bus);
        let ids: Vec<u32> = [0x0C9, 0x7E8, 0x7E9, 0x7EA, 0x7F0]
            .into_iter()
            .filter(|&id| sm3.passes(&Frame::standard(id, &[0])))
            .collect();
        assert_eq!(ids, [0x7E8, 0x7E9], "pass 0x7E8-0x7EF but block 0x7EA");
        // No filters: an open channel that passes nothing; speed 0 closes it.
        d[142] = 0;
        sm3.answer(0x50, &d, &mut bus);
        assert_eq!(sm3.filters, Some(vec![]));
        assert!(!sm3.passes(&Frame::standard(0x7E8, &[0])));
        d[134..138].fill(0);
        sm3.answer(0x50, &d, &mut bus);
        assert_eq!(sm3.filters, None);
        assert!(
            sm3.answer(0x50, &d[..100], &mut bus).len() % 4 == 0,
            "short: answered"
        );
        assert_eq!(sm3.filters, None, "and ignored");
    }

    #[test]
    fn a_slow_client_gets_the_latest_frames() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let mut bus = Bus::default();
        // Ten seconds of a 1000 frame/s bus waiting before the client comes.
        for k in 0..10_000u32 {
            bus.out.push_back(Frame::standard(0x100, &k.to_be_bytes()));
        }
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut sm3 = Sm3::default();
            let _ = session(s, &mut sm3, &mut bus); // the client hangs up mid-block
            sm3
        });
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let mut got = Vec::new();
        let mut chunk = [0u8; 65536];
        let mut frames = Vec::new();
        while frames.len() < QUEUE {
            let n = c.read(&mut chunk).unwrap();
            got.extend_from_slice(&chunk[..n]);
            while let Some((op, d)) = take(&mut got) {
                assert_eq!(op, 0xFE);
                for i in 0..(d[34] >> 4) as usize {
                    let o = 36 + 16 * i;
                    frames.push(u32::from_be_bytes(d[o + 8..o + 12].try_into().unwrap()));
                }
            }
        }
        drop(c);
        let sm3 = server.join().unwrap();
        assert_eq!(
            frames.first(),
            Some(&(10_000 - QUEUE as u32)),
            "the oldest are gone"
        );
        assert_eq!(frames.last(), Some(&9_999));
        assert_eq!(sm3.dropped, 10_000 - QUEUE as u64);
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
