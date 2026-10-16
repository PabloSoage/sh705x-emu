//! The adapter behind the J2534 calls: one TCP connection to an emulated SM3
//! (`sh705x sm3`), the channels open on it, and ISO 15765-2 done on this side.
//!
//! The SM3 frame carries raw CAN both ways (`bridge::sm3`): writes go as op 0x1C
//! `60 80 02` records, the bus comes back unasked in op 0xFE blocks. Everything above
//! that, filters, segmentation, flow control, periodic messages, lives here, so any
//! J2534 application (GDS2, PCMflash…) can run against the emulator without the
//! vendor's DLL, which only reaches a physical adapter.
//!
//! A message's data is J2534's: the CAN identifier as 4 big-endian bytes, then the
//! payload; for ISO 15765 with extended addressing the address byte comes after the
//! identifier.

use bridge::sm3::{message, take};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Raw CAN.
pub const CAN: u32 = 5;
/// ISO 15765-2 on CAN.
pub const ISO15765: u32 = 6;

/// Rx status: a message this side sent, given back because loopback is on.
pub const TX_MSG_TYPE: u32 = 0x01;
/// Rx status: a first frame came in; the rest of the message follows.
pub const ISO15765_FIRST_FRAME: u32 = 0x02;
/// Tx flag: pad every frame to 8 bytes.
pub const ISO15765_FRAME_PAD: u32 = 0x40;
/// Tx flag: extended addressing, an address byte after the identifier.
pub const ISO15765_ADDR_TYPE: u32 = 0x80;
/// Tx flag and rx status: 29-bit identifier.
pub const CAN_29BIT_ID: u32 = 0x100;

/// Filter kinds.
pub const PASS_FILTER: u32 = 1;
/// Drops what it matches.
pub const BLOCK_FILTER: u32 = 2;
/// ISO 15765: pairs a receive identifier with the one flow control goes to.
pub const FLOW_CONTROL_FILTER: u32 = 3;

/// Configuration parameters this side keeps.
pub const DATA_RATE: u32 = 0x01;
/// Gives sent messages back as received ones.
pub const LOOPBACK: u32 = 0x03;
/// Block size this side asks for in its flow control.
pub const ISO15765_BS: u32 = 0x1E;
/// Separation time this side asks for in its flow control.
pub const ISO15765_STMIN: u32 = 0x1F;

/// How long a sender waits for the ECU's flow control.
const FC_WAIT: Duration = Duration::from_millis(1000);
/// How long to wait for the adapter's answer to a request.
const ANSWER_WAIT: Duration = Duration::from_millis(2000);
/// Messages kept per channel before the oldest go.
const RX_LIMIT: usize = 4096;

/// J2534 result codes this side uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Error {
    /// The call is not supported.
    NotSupported = 0x01,
    /// No such channel.
    InvalidChannelId = 0x02,
    /// Unknown protocol.
    InvalidProtocolId = 0x03,
    /// A null pointer where one was needed.
    NullParameter = 0x04,
    /// Something else went wrong.
    Failed = 0x07,
    /// The adapter is not there.
    DeviceNotConnected = 0x08,
    /// Fewer messages than asked for in the time given.
    Timeout = 0x09,
    /// A message that cannot be sent.
    InvalidMsg = 0x0A,
    /// Too many of something.
    ExceededLimit = 0x0C,
    /// No such periodic message.
    InvalidMsgId = 0x0D,
    /// Already open.
    DeviceInUse = 0x0E,
    /// Unknown ioctl.
    InvalidIoctlId = 0x0F,
    /// Nothing to read.
    BufferEmpty = 0x10,
    /// No such filter.
    InvalidFilterId = 0x16,
    /// An ISO 15765 send with no flow-control filter for its identifier.
    NoFlowControl = 0x17,
    /// No such device.
    InvalidDeviceId = 0x1A,
}

/// A J2534 message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Msg {
    /// Protocol it belongs to.
    pub protocol: u32,
    /// Receive status bits.
    pub rx_status: u32,
    /// Transmit flags.
    pub tx_flags: u32,
    /// Microseconds since the adapter was opened.
    pub timestamp: u32,
    /// Identifier (4 bytes, big-endian), optional address byte, payload.
    pub data: Vec<u8>,
}

/// A frame on the bus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanFrame {
    /// Identifier.
    pub id: u32,
    /// Payload, at most 8 bytes.
    pub data: Vec<u8>,
}

/// A message filter.
#[derive(Clone, Debug)]
pub struct Filter {
    /// [`PASS_FILTER`], [`BLOCK_FILTER`] or [`FLOW_CONTROL_FILTER`].
    pub kind: u32,
    /// Bits compared.
    pub mask: Vec<u8>,
    /// What they must be.
    pub pattern: Vec<u8>,
    /// Flow control: where this side's flow control goes (identifier and address).
    pub flow: Vec<u8>,
    /// The flags of the filter's messages (extended addressing, 29-bit).
    pub tx_flags: u32,
}

impl Filter {
    fn matches(&self, data: &[u8]) -> bool {
        data.len() >= self.mask.len()
            && self
                .mask
                .iter()
                .zip(&self.pattern)
                .zip(data)
                .all(|((m, p), d)| d & m == p & m)
    }

    fn extended_addressing(&self) -> bool {
        self.tx_flags & ISO15765_ADDR_TYPE != 0
    }
}

/// A long message coming in.
#[derive(Debug)]
struct Incoming {
    /// The receive identifier and address, as in the message data.
    head: Vec<u8>,
    total: usize,
    payload: Vec<u8>,
    next_sn: u8,
    tx_flags: u32,
}

/// One channel.
#[derive(Debug)]
struct Channel {
    id: u32,
    protocol: u32,
    baud: u32,
    filters: Vec<(u32, Filter)>,
    next_filter: u32,
    rx: VecDeque<Msg>,
    incoming: Vec<Incoming>,
    loopback: bool,
    block_size: u8,
    st_min: u8,
}

/// A periodic message.
#[derive(Debug)]
struct Periodic {
    id: u32,
    channel: u32,
    msg: Msg,
    every: Duration,
    due: Instant,
}

/// What the reader thread and the callers share.
#[derive(Debug, Default)]
struct State {
    channels: Vec<Channel>,
    next_channel: u32,
    /// Flow-control frames for senders waiting on one: (identifier, frame).
    flow: VecDeque<CanFrame>,
    /// The adapter's answers (op 0x00), newest last.
    answers: VecDeque<Vec<u8>>,
    periodic: Vec<Periodic>,
    next_periodic: u32,
    gone: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    out: Mutex<TcpStream>,
    t0: Instant,
    stop: AtomicBool,
    log: Option<Mutex<std::fs::File>>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn now(&self) -> u32 {
        self.t0.elapsed().as_micros() as u32
    }

    fn log(&self, line: &str) {
        if let Some(f) = &self.log {
            let mut f = f.lock().unwrap_or_else(|e| e.into_inner());
            let _ = writeln!(f, "{:>10} {line}", self.now());
        }
    }

    fn send_frame(&self, f: &CanFrame) -> Result<(), Error> {
        let mut d = vec![0x60, 0x80, 0x02];
        d.extend((f.data.len() as u32).to_le_bytes());
        d.extend(f.id.to_le_bytes());
        let mut bytes = [0u8; 8];
        bytes[..f.data.len().min(8)].copy_from_slice(&f.data[..f.data.len().min(8)]);
        d.extend(bytes);
        self.log(&format!("tx {:08X} {}", f.id, hex(&f.data)));
        self.out
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .write_all(&message(0x1C, &d))
            .map_err(|_| Error::DeviceNotConnected)
    }

    fn send_frames(&self, frames: &[CanFrame]) {
        for f in frames {
            let _ = self.send_frame(f);
        }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn id_of(data: &[u8]) -> u32 {
    u32::from_be_bytes([data[0], data[1], data[2], data[3]])
}

fn pad(mut data: Vec<u8>, yes: bool) -> Vec<u8> {
    if yes {
        data.resize(8, 0);
    }
    data
}

impl Channel {
    fn passes(&self, data: &[u8]) -> bool {
        let f = || self.filters.iter().map(|(_, f)| f);
        !f().any(|x| x.kind == BLOCK_FILTER && x.matches(data))
            && f().any(|x| x.kind == PASS_FILTER && x.matches(data))
    }

    fn push(&mut self, m: Msg) {
        if self.rx.len() == RX_LIMIT {
            self.rx.pop_front();
        }
        self.rx.push_back(m);
    }

    /// A frame from the bus: queues what this channel receives, and returns the frames
    /// to send back (flow control) and the flow-control frames for waiting senders.
    fn take(&mut self, f: &CanFrame, now: u32) -> (Vec<CanFrame>, Option<CanFrame>) {
        let mut head = f.id.to_be_bytes().to_vec();
        let ext_id = if f.id > 0x7FF { CAN_29BIT_ID } else { 0 };
        if self.protocol == CAN {
            head.extend(&f.data);
            if self.passes(&head) {
                self.push(Msg {
                    protocol: CAN,
                    rx_status: ext_id,
                    timestamp: now,
                    data: head,
                    ..Msg::default()
                });
            }
            return (vec![], None);
        }
        // ISO 15765: only through a flow-control filter.
        let mut full = head.clone();
        full.extend(&f.data);
        let Some(filter) = self
            .filters
            .iter()
            .map(|(_, x)| x)
            .find(|x| x.kind == FLOW_CONTROL_FILTER && x.matches(&full))
            .cloned()
        else {
            return (vec![], None);
        };
        let ext = filter.extended_addressing();
        let at = usize::from(ext);
        if f.data.len() <= at {
            return (vec![], None);
        }
        if ext {
            head.push(f.data[0]);
        }
        let pci = f.data[at];
        let body = &f.data[at + 1..];
        let flags = filter.tx_flags | ext_id;
        match pci >> 4 {
            0 => {
                let n = usize::from(pci & 0x0F).min(body.len());
                let mut data = head;
                data.extend(&body[..n]);
                self.push(Msg {
                    protocol: ISO15765,
                    rx_status: flags & (CAN_29BIT_ID | ISO15765_ADDR_TYPE),
                    timestamp: now,
                    data,
                    ..Msg::default()
                });
                (vec![], None)
            }
            1 if !body.is_empty() => {
                let total = (usize::from(pci & 0x0F) << 8) | usize::from(body[0]);
                self.incoming.retain(|i| i.head != head);
                self.push(Msg {
                    protocol: ISO15765,
                    rx_status: ISO15765_FIRST_FRAME | (flags & (CAN_29BIT_ID | ISO15765_ADDR_TYPE)),
                    timestamp: now,
                    data: head.clone(),
                    ..Msg::default()
                });
                self.incoming.push(Incoming {
                    head,
                    total,
                    payload: body[1..].to_vec(),
                    next_sn: 1,
                    tx_flags: flags,
                });
                let mut fc = Vec::new();
                if ext {
                    fc.push(filter.flow.get(4).copied().unwrap_or(0));
                }
                fc.extend([0x30, self.block_size, self.st_min]);
                let to = CanFrame {
                    id: id_of(&filter.flow),
                    data: pad(fc, true),
                };
                (vec![to], None)
            }
            2 => {
                let Some(k) = self.incoming.iter().position(|i| i.head == head) else {
                    return (vec![], None);
                };
                let inc = &mut self.incoming[k];
                if pci & 0x0F != inc.next_sn {
                    self.incoming.remove(k);
                    return (vec![], None);
                }
                inc.next_sn = (inc.next_sn + 1) & 0x0F;
                let room = inc.total - inc.payload.len();
                inc.payload.extend(&body[..body.len().min(room)]);
                // A block size means another flow control after that many frames;
                // this side asks for none (0) unless configured, and counts nothing.
                if inc.payload.len() == inc.total {
                    let inc = self.incoming.remove(k);
                    let mut data = inc.head;
                    data.extend(inc.payload);
                    self.push(Msg {
                        protocol: ISO15765,
                        rx_status: inc.tx_flags & (CAN_29BIT_ID | ISO15765_ADDR_TYPE),
                        timestamp: now,
                        data,
                        ..Msg::default()
                    });
                }
                (vec![], None)
            }
            3 => (vec![], Some(f.clone())),
            _ => (vec![], None),
        }
    }
}

/// The connection to the emulated adapter.
pub struct Device {
    shared: Arc<Shared>,
    reader: Option<std::thread::JoinHandle<()>>,
    ticker: Option<std::thread::JoinHandle<()>>,
}

impl Device {
    /// Connects to `addr`, greets the adapter and starts reading the bus.
    pub fn open(addr: &str, log: Option<std::fs::File>) -> Result<Device, Error> {
        let stream = TcpStream::connect(addr).map_err(|_| Error::DeviceNotConnected)?;
        stream.set_nodelay(true).ok();
        let input = stream.try_clone().map_err(|_| Error::Failed)?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            out: Mutex::new(stream),
            t0: Instant::now(),
            stop: AtomicBool::new(false),
            log: log.map(Mutex::new),
        });
        shared.log(&format!("open {addr}"));
        let s = shared.clone();
        let reader = std::thread::spawn(move || read_loop(s, input));
        let s = shared.clone();
        let ticker = std::thread::spawn(move || tick_loop(s));
        let dev = Device {
            shared,
            reader: Some(reader),
            ticker: Some(ticker),
        };
        // The greeting: 66 bytes back, or nobody is there.
        dev.ask(0x83, &[], 66)?;
        Ok(dev)
    }

    /// Writes a message to the adapter and waits for its answer of `len` bytes.
    fn ask(&self, op: u8, data: &[u8], len: usize) -> Result<Vec<u8>, Error> {
        let sh = &self.shared;
        sh.lock().answers.clear();
        sh.out
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .write_all(&message(op, data))
            .map_err(|_| Error::DeviceNotConnected)?;
        let deadline = Instant::now() + ANSWER_WAIT;
        let mut st = sh.lock();
        loop {
            if let Some(k) = st.answers.iter().position(|a| a.len() == len) {
                return Ok(st.answers.remove(k).unwrap_or_default());
            }
            if st.gone {
                return Err(Error::DeviceNotConnected);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::DeviceNotConnected);
            }
            st = sh
                .wake
                .wait_timeout(st, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Battery voltage, millivolts.
    pub fn battery_mv(&self) -> Result<u32, Error> {
        let d = self.ask(0x20, &[], 6)?;
        Ok(u32::from(u16::from_le_bytes([d[0], d[1]])))
    }

    /// Opens a channel.
    pub fn connect(&self, protocol: u32, baud: u32) -> Result<u32, Error> {
        if protocol != CAN && protocol != ISO15765 {
            return Err(Error::InvalidProtocolId);
        }
        let mut st = self.shared.lock();
        if st.channels.len() >= 8 {
            return Err(Error::ExceededLimit);
        }
        st.next_channel += 1;
        let id = st.next_channel;
        st.channels.push(Channel {
            id,
            protocol,
            baud,
            filters: vec![],
            next_filter: 0,
            rx: VecDeque::new(),
            incoming: vec![],
            loopback: false,
            block_size: 0,
            st_min: 0,
        });
        self.shared
            .log(&format!("connect ch{id} protocol {protocol} baud {baud}"));
        Ok(id)
    }

    fn with<T>(
        &self,
        ch: u32,
        f: impl FnOnce(&mut Channel) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut st = self.shared.lock();
        let c = st
            .channels
            .iter_mut()
            .find(|c| c.id == ch)
            .ok_or(Error::InvalidChannelId)?;
        f(c)
    }

    /// Closes a channel.
    pub fn disconnect(&self, ch: u32) -> Result<(), Error> {
        let mut st = self.shared.lock();
        let n = st.channels.len();
        st.channels.retain(|c| c.id != ch);
        st.periodic.retain(|p| p.channel != ch);
        if st.channels.len() == n {
            return Err(Error::InvalidChannelId);
        }
        Ok(())
    }

    /// Adds a filter.
    pub fn start_filter(&self, ch: u32, filter: Filter) -> Result<u32, Error> {
        self.shared.log(&format!(
            "filter ch{ch} kind {} mask {} pattern {} flow {}",
            filter.kind,
            hex(&filter.mask),
            hex(&filter.pattern),
            hex(&filter.flow)
        ));
        self.with(ch, |c| {
            if c.filters.len() >= 32 {
                return Err(Error::ExceededLimit);
            }
            c.next_filter += 1;
            c.filters.push((c.next_filter, filter));
            Ok(c.next_filter)
        })
    }

    /// Removes a filter.
    pub fn stop_filter(&self, ch: u32, id: u32) -> Result<(), Error> {
        self.with(ch, |c| {
            let n = c.filters.len();
            c.filters.retain(|(k, _)| *k != id);
            if c.filters.len() == n {
                Err(Error::InvalidFilterId)
            } else {
                Ok(())
            }
        })
    }

    /// Reads up to `max` messages, waiting up to `timeout` for that many.
    pub fn read(&self, ch: u32, max: usize, timeout: Duration) -> (Vec<Msg>, Result<(), Error>) {
        let deadline = Instant::now() + timeout;
        let mut got = Vec::new();
        let mut st = self.shared.lock();
        loop {
            let Some(c) = st.channels.iter_mut().find(|c| c.id == ch) else {
                return (got, Err(Error::InvalidChannelId));
            };
            while got.len() < max {
                match c.rx.pop_front() {
                    Some(m) => got.push(m),
                    None => break,
                }
            }
            if got.len() == max {
                return (got, Ok(()));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                let r = if timeout.is_zero() {
                    if got.is_empty() {
                        Err(Error::BufferEmpty)
                    } else {
                        Ok(())
                    }
                } else {
                    Err(Error::Timeout)
                };
                return (got, r);
            }
            if st.gone {
                return (got, Err(Error::DeviceNotConnected));
            }
            st = self
                .shared
                .wake
                .wait_timeout(st, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Sends one message.
    pub fn write(&self, ch: u32, m: &Msg) -> Result<(), Error> {
        if m.data.len() < 4 {
            return Err(Error::InvalidMsg);
        }
        let (protocol, loopback, fc_from) = {
            let st = self.shared.lock();
            let c = st
                .channels
                .iter()
                .find(|c| c.id == ch)
                .ok_or(Error::InvalidChannelId)?;
            if m.protocol != c.protocol {
                return Err(Error::InvalidMsg);
            }
            // The identifier flow control comes back on: the flow-control filter whose
            // flow side is where this message goes.
            let ext = m.tx_flags & ISO15765_ADDR_TYPE != 0;
            let key = &m.data[..if ext { 5 } else { 4 }.min(m.data.len())];
            let fc_from = c
                .filters
                .iter()
                .map(|(_, f)| f)
                .find(|f| {
                    f.kind == FLOW_CONTROL_FILTER
                        && f.flow.len() >= key.len()
                        && f.flow[..key.len()] == *key
                })
                .map(|f| id_of(&f.pattern));
            (c.protocol, c.loopback, fc_from)
        };
        self.shared.log(&format!(
            "write ch{ch} flags {:X} {}",
            m.tx_flags,
            hex(&m.data)
        ));
        let id = id_of(&m.data);
        let padded = m.tx_flags & ISO15765_FRAME_PAD != 0;
        if protocol == CAN {
            if m.data.len() > 12 {
                return Err(Error::InvalidMsg);
            }
            self.shared.send_frame(&CanFrame {
                id,
                data: m.data[4..].to_vec(),
            })?;
        } else {
            let fc_from = fc_from.ok_or(Error::NoFlowControl)?;
            self.send_isotp(id, m, padded, fc_from)?;
        }
        if loopback {
            let mut echo = m.clone();
            echo.rx_status = TX_MSG_TYPE | (m.tx_flags & (CAN_29BIT_ID | ISO15765_ADDR_TYPE));
            echo.timestamp = self.shared.now();
            self.with(ch, |c| {
                c.push(echo);
                Ok(())
            })?;
        }
        Ok(())
    }

    fn send_isotp(&self, id: u32, m: &Msg, padded: bool, fc_from: u32) -> Result<(), Error> {
        let ext = m.tx_flags & ISO15765_ADDR_TYPE != 0;
        let (addr, payload) = if ext {
            (Some(*m.data.get(4).ok_or(Error::InvalidMsg)?), &m.data[5..])
        } else {
            (None, &m.data[4..])
        };
        let room = 7 - usize::from(ext);
        let frame = |body: Vec<u8>| {
            let mut d: Vec<u8> = addr.into_iter().collect();
            d.extend(body);
            CanFrame {
                id,
                data: pad(d, padded),
            }
        };
        if payload.len() <= room {
            let mut b = vec![payload.len() as u8];
            b.extend(payload);
            return self.shared.send_frame(&frame(b));
        }
        if payload.len() > 0xFFF {
            return Err(Error::InvalidMsg);
        }
        self.shared.lock().flow.clear();
        let first = room - 1;
        let mut b = vec![0x10 | (payload.len() >> 8) as u8, payload.len() as u8];
        b.extend(&payload[..first]);
        self.shared.send_frame(&frame(b))?;
        let mut at = first;
        let mut sn = 1u8;
        while at < payload.len() {
            let (bs, st_min) = self.wait_flow(fc_from, ext)?;
            let mut sent = 0u8;
            while at < payload.len() && (bs == 0 || sent < bs) {
                let n = (room).min(payload.len() - at);
                let mut b = vec![0x20 | sn];
                b.extend(&payload[at..at + n]);
                self.shared.send_frame(&frame(b))?;
                at += n;
                sn = (sn + 1) & 0x0F;
                sent += 1;
                if st_min > Duration::ZERO {
                    std::thread::sleep(st_min);
                }
            }
        }
        Ok(())
    }

    /// Waits for the ECU's flow control on `from`: (block size, separation time).
    fn wait_flow(&self, from: u32, ext: bool) -> Result<(u8, Duration), Error> {
        let deadline = Instant::now() + FC_WAIT;
        let mut st = self.shared.lock();
        loop {
            while let Some(f) = st.flow.pop_front() {
                if f.id != from {
                    continue;
                }
                let at = usize::from(ext);
                let Some(&pci) = f.data.get(at) else { continue };
                match pci & 0x0F {
                    0 => {
                        let bs = f.data.get(at + 1).copied().unwrap_or(0);
                        let st_min = match f.data.get(at + 2).copied().unwrap_or(0) {
                            ms @ 0..=0x7F => Duration::from_millis(u64::from(ms)),
                            us @ 0xF1..=0xF9 => Duration::from_micros(u64::from(us - 0xF0) * 100),
                            _ => Duration::from_millis(127),
                        };
                        return Ok((bs, st_min));
                    }
                    1 => continue, // wait: the next one decides
                    _ => return Err(Error::Failed),
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || st.gone {
                return Err(Error::Timeout);
            }
            st = self
                .shared
                .wake
                .wait_timeout(st, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Starts a periodic message.
    pub fn start_periodic(&self, ch: u32, m: Msg, every: Duration) -> Result<u32, Error> {
        self.with(ch, |_| Ok(()))?;
        let mut st = self.shared.lock();
        if st.periodic.len() >= 10 {
            return Err(Error::ExceededLimit);
        }
        st.next_periodic += 1;
        let id = st.next_periodic;
        st.periodic.push(Periodic {
            id,
            channel: ch,
            msg: m,
            every,
            due: Instant::now(),
        });
        Ok(id)
    }

    /// Stops a periodic message.
    pub fn stop_periodic(&self, ch: u32, id: u32) -> Result<(), Error> {
        let mut st = self.shared.lock();
        let n = st.periodic.len();
        st.periodic.retain(|p| !(p.channel == ch && p.id == id));
        if st.periodic.len() == n {
            return Err(Error::InvalidMsgId);
        }
        Ok(())
    }

    /// GET_CONFIG for one parameter.
    pub fn get_config(&self, ch: u32, param: u32) -> Result<u32, Error> {
        self.with(ch, |c| {
            Ok(match param {
                DATA_RATE => c.baud,
                LOOPBACK => u32::from(c.loopback),
                ISO15765_BS => u32::from(c.block_size),
                ISO15765_STMIN => u32::from(c.st_min),
                _ => 0,
            })
        })
    }

    /// SET_CONFIG for one parameter; the ones this side has no use for are accepted.
    pub fn set_config(&self, ch: u32, param: u32, value: u32) -> Result<(), Error> {
        self.with(ch, |c| {
            match param {
                DATA_RATE => c.baud = value,
                LOOPBACK => c.loopback = value != 0,
                ISO15765_BS => c.block_size = value as u8,
                ISO15765_STMIN => c.st_min = value as u8,
                _ => {}
            }
            Ok(())
        })
    }

    /// CLEAR_RX_BUFFER.
    pub fn clear_rx(&self, ch: u32) -> Result<(), Error> {
        self.with(ch, |c| {
            c.rx.clear();
            c.incoming.clear();
            Ok(())
        })
    }

    /// CLEAR_MSG_FILTERS.
    pub fn clear_filters(&self, ch: u32) -> Result<(), Error> {
        self.with(ch, |c| {
            c.filters.clear();
            Ok(())
        })
    }

    /// CLEAR_PERIODIC_MSGS.
    pub fn clear_periodic(&self, ch: u32) -> Result<(), Error> {
        self.with(ch, |_| Ok(()))?;
        self.shared.lock().periodic.retain(|p| p.channel != ch);
        Ok(())
    }

    /// Writes a line to the log, if there is one.
    pub fn log(&self, line: &str) {
        self.shared.log(line);
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Ok(s) = self.shared.out.lock() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        self.shared.wake.notify_all();
        if let Some(t) = self.reader.take() {
            let _ = t.join();
        }
        if let Some(t) = self.ticker.take() {
            let _ = t.join();
        }
    }
}

/// Parses the adapter's messages and hands the bus to the channels.
fn read_loop(sh: Arc<Shared>, mut input: TcpStream) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    while !sh.stop.load(Ordering::Relaxed) {
        let n = match input.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buf.extend_from_slice(&chunk[..n]);
        while let Some((op, d)) = take(&mut buf) {
            match op {
                0x00 => {
                    let mut st = sh.lock();
                    if st.answers.len() == 16 {
                        st.answers.pop_front();
                    }
                    st.answers.push_back(d);
                }
                0xFE if d.len() >= 36 => {
                    let count = usize::from(d[34] >> 4);
                    let now = sh.now();
                    let mut back = Vec::new();
                    let mut st = sh.lock();
                    for i in 0..count {
                        let o = 36 + 16 * i;
                        if o + 16 > d.len() {
                            break;
                        }
                        let len = (u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
                            as usize)
                            .min(8);
                        let id = u32::from_le_bytes([d[o + 4], d[o + 5], d[o + 6], d[o + 7]]);
                        let f = CanFrame {
                            id,
                            data: d[o + 8..o + 8 + len].to_vec(),
                        };
                        sh.log(&format!("rx {:08X} {}", f.id, hex(&f.data)));
                        let mut flow = None;
                        for c in st.channels.iter_mut() {
                            let (b, fl) = c.take(&f, now);
                            back.extend(b);
                            flow = flow.or(fl);
                        }
                        if let Some(fl) = flow {
                            st.flow.push_back(fl);
                        }
                    }
                    drop(st);
                    sh.send_frames(&back);
                }
                _ => {}
            }
            sh.wake.notify_all();
        }
    }
    sh.lock().gone = true;
    sh.wake.notify_all();
}

/// Sends the periodic messages when due.
fn tick_loop(sh: Arc<Shared>) {
    while !sh.stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        let mut due = Vec::new();
        {
            let mut st = sh.lock();
            for p in st.periodic.iter_mut() {
                if p.due <= now {
                    p.due = now + p.every;
                    due.push(p.msg.clone());
                }
            }
        }
        for m in due {
            if m.data.len() < 4 {
                continue;
            }
            let id = id_of(&m.data);
            let padded = m.tx_flags & ISO15765_FRAME_PAD != 0;
            let data = if m.protocol == ISO15765 {
                let ext = m.tx_flags & ISO15765_ADDR_TYPE != 0;
                let at = 4 + usize::from(ext);
                if m.data.len() < at || m.data.len() - at > 7 - usize::from(ext) {
                    continue;
                }
                let mut d = m.data[4..at].to_vec();
                d.push((m.data.len() - at) as u8);
                d.extend(&m.data[at..]);
                pad(d, padded)
            } else {
                m.data[4..].to_vec()
            };
            let _ = sh.send_frame(&CanFrame { id, data });
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
