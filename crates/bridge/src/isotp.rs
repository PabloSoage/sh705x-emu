//! ISO 15765-2 (ISO-TP) from the diagnostic tool's side: a request out, the answers in,
//! with flow control for long ones.
//!
//! Requests of up to 7 bytes go as a single frame; longer ones as a first frame and
//! consecutive frames once the ECU's flow control comes back. Frames are padded to 8
//! bytes with H'00. Answers are reassembled per sender; a first frame is answered with a
//! flow control (continue, no block limit, no separation time) sent to the sender's ID
//! minus 8, the OBD pairing of 11-bit IDs.

use crate::link::CanLink;
use sh7059::hcan::Frame;
use std::collections::BTreeMap;

/// One ECU's answer: its CAN ID, the payload, and the frames it came in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// The ECU's CAN ID.
    pub id: u32,
    /// Reassembled payload.
    pub payload: Vec<u8>,
    /// The raw frames, in order.
    pub frames: Vec<Frame>,
}

/// How long to wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// For the first answer, and between frames of one answer, in microseconds.
    pub answer_us: u64,
    /// After an answer is complete, for more answers (other ECUs), in microseconds; 0
    /// returns at the first complete answer.
    pub more_us: u64,
}

impl Default for Timing {
    fn default() -> Self {
        // An ELM327's default AT ST is about 200 ms.
        Self {
            answer_us: 200_000,
            more_us: 20_000,
        }
    }
}

fn padded(bytes: &[u8]) -> [u8; 8] {
    let mut d = [0u8; 8];
    d[..bytes.len()].copy_from_slice(bytes);
    d
}

fn frame(id: u32, bytes: &[u8]) -> Frame {
    Frame {
        id,
        extended: false,
        remote: false,
        dlc: 8,
        data: padded(bytes),
    }
}

/// Sends `payload` to `tx_id` and collects the answers from IDs that `accept` takes.
/// Answers still incomplete when time runs out are returned as they are.
pub fn request(
    link: &mut dyn CanLink,
    tx_id: u32,
    payload: &[u8],
    accept: &dyn Fn(u32) -> bool,
    timing: Timing,
) -> Vec<Answer> {
    if payload.len() <= 7 {
        let mut b = vec![payload.len() as u8];
        b.extend_from_slice(payload);
        link.send(frame(tx_id, &b));
    } else if !send_long(link, tx_id, payload, accept, timing) {
        return Vec::new();
    }
    // Per sender: the answer so far, the length announced, the next sequence number.
    let mut partial: BTreeMap<u32, (Answer, usize, u8)> = BTreeMap::new();
    let mut done = Vec::new();
    // Frames from other IDs (the ECU's periodic broadcasts) do not move the deadline.
    let mut deadline = link.now_us() + timing.answer_us;
    loop {
        let now = link.now_us();
        if now >= deadline {
            break;
        }
        let Some(f) = link.recv(deadline - now) else {
            break;
        };
        if f.extended || f.remote || !accept(f.id) {
            continue;
        }
        let wait;
        let d = &f.data[..f.dlc.min(8) as usize];
        let Some(&pci) = d.first() else { continue };
        match pci >> 4 {
            0 => {
                let n = (pci & 0xF) as usize;
                if n == 0 || n + 1 > d.len() {
                    continue;
                }
                done.push(Answer {
                    id: f.id,
                    payload: d[1..1 + n].to_vec(),
                    frames: vec![f],
                });
                wait = timing.more_us;
            }
            1 if d.len() >= 2 => {
                let n = (((pci & 0xF) as usize) << 8) | d[1] as usize;
                let a = Answer {
                    id: f.id,
                    payload: d[2..].to_vec(),
                    frames: vec![f],
                };
                partial.insert(f.id, (a, n, 1));
                link.send(frame(f.id.wrapping_sub(8), &[0x30, 0, 0]));
                wait = timing.answer_us;
            }
            2 => {
                let Some((a, n, seq)) = partial.get_mut(&f.id) else {
                    continue;
                };
                if pci & 0xF != *seq & 0xF {
                    continue; // out of sequence: dropped, as a tester would
                }
                *seq = seq.wrapping_add(1);
                a.payload.extend_from_slice(&d[1..]);
                a.frames.push(f);
                if a.payload.len() >= *n {
                    let (mut a, n, _) = partial.remove(&f.id).expect("present");
                    a.payload.truncate(n);
                    done.push(a);
                    wait = timing.more_us;
                } else {
                    wait = timing.answer_us;
                }
            }
            _ => continue,
        }
        if wait == 0 {
            break;
        }
        deadline = link.now_us() + wait;
    }
    done.extend(partial.into_values().map(|(a, _, _)| a));
    done
}

/// A request longer than a single frame: first frame, wait for flow control, then the
/// consecutive frames. False if no flow control came.
fn send_long(
    link: &mut dyn CanLink,
    tx_id: u32,
    payload: &[u8],
    accept: &dyn Fn(u32) -> bool,
    timing: Timing,
) -> bool {
    let n = payload.len().min(0xFFF);
    let mut first = vec![0x10 | (n >> 8) as u8, n as u8];
    first.extend_from_slice(&payload[..6]);
    link.send(frame(tx_id, &first));
    let mut rest = &payload[6..n];
    let mut seq = 1u8;
    while !rest.is_empty() {
        // Wait for a flow control: continue (0), wait (1), or overflow (2).
        let deadline = link.now_us() + timing.answer_us;
        let fc = loop {
            let now = link.now_us();
            if now >= deadline {
                return false;
            }
            match link.recv(deadline - now) {
                None => return false,
                Some(f) if accept(f.id) && f.data[0] >> 4 == 3 => break f,
                Some(_) => {}
            }
        };
        match fc.data[0] & 0xF {
            0 => {}
            1 => continue,
            _ => return false,
        }
        let block = fc.data[1] as usize;
        let mut sent = 0;
        while !rest.is_empty() && (block == 0 || sent < block) {
            let k = rest.len().min(7);
            let mut cf = vec![0x20 | (seq & 0xF)];
            cf.extend_from_slice(&rest[..k]);
            link.send(frame(tx_id, &cf));
            rest = &rest[k..];
            seq = seq.wrapping_add(1);
            sent += 1;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A fake ECU at 0x7E8: answers 01 00 with a single frame and 09 02 with three
    /// frames once it gets a flow control; echoes long requests' length.
    #[derive(Default)]
    struct Fake {
        out: VecDeque<Frame>,
        sent: Vec<Frame>,
        pending_vin: bool,
    }

    impl CanLink for Fake {
        fn send(&mut self, f: Frame) {
            self.sent.push(f);
            let d = f.data;
            match (f.id, d[0], d[1], d[2]) {
                (0x7DF | 0x7E0, 0x02, 0x01, 0x00) => self
                    .out
                    .push_back(frame(0x7E8, &[0x06, 0x41, 0x00, 0x98, 0x3B, 0x20, 0x13])),
                (0x7DF | 0x7E0, 0x02, 0x09, 0x02) => {
                    self.out.push_back(frame(
                        0x7E8,
                        &[0x10, 0x14, 0x49, 0x02, 0x01, b'W', b'0', b'L'],
                    ));
                    self.pending_vin = true;
                }
                (0x7E0, 0x30, _, _) if self.pending_vin => {
                    self.pending_vin = false;
                    self.out.push_back(frame(
                        0x7E8,
                        &[0x21, b'A', b'B', b'C', b'D', b'E', b'F', b'G'],
                    ));
                    self.out.push_back(frame(
                        0x7E8,
                        &[0x22, b'H', b'J', b'K', b'L', b'M', b'N', b'P'],
                    ));
                }
                (0x7E0, 0x10, _, _) => self.out.push_back(frame(0x7E8, &[0x30, 0x02, 0x00])),
                _ => {}
            }
        }

        fn now_us(&self) -> u64 {
            0
        }

        fn recv(&mut self, _: u64) -> Option<Frame> {
            self.out.pop_front()
        }
    }

    fn obd(id: u32) -> bool {
        (0x7E8..=0x7EF).contains(&id)
    }

    #[test]
    fn single_frame_answer() {
        let mut f = Fake::default();
        let a = request(&mut f, 0x7DF, &[0x01, 0x00], &obd, Timing::default());
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].payload, [0x41, 0x00, 0x98, 0x3B, 0x20, 0x13]);
        assert_eq!(
            f.sent[0].data,
            [0x02, 0x01, 0x00, 0, 0, 0, 0, 0],
            "padded with 00"
        );
    }

    #[test]
    fn multi_frame_answer_with_flow_control() {
        let mut f = Fake::default();
        let a = request(&mut f, 0x7DF, &[0x09, 0x02], &obd, Timing::default());
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].payload.len(), 0x14);
        assert_eq!(&a[0].payload[..6], &[0x49, 0x02, 0x01, b'W', b'0', b'L']);
        assert_eq!(&a[0].payload[17..], b"MNP");
        assert_eq!(a[0].frames.len(), 3);
        let fc = f.sent[1];
        assert_eq!(
            (fc.id, fc.data[0]),
            (0x7E0, 0x30),
            "flow control to 0x7E8 - 8"
        );
    }

    #[test]
    fn long_request_in_blocks() {
        let mut f = Fake::default();
        let payload: Vec<u8> = (0..20).collect();
        let a = request(&mut f, 0x7E0, &payload, &obd, Timing::default());
        // 6 bytes in the first frame, then two consecutive frames of 7, within the block
        // of 2 the flow control allows.
        let cfs: Vec<u8> = f.sent.iter().map(|s| s.data[0]).collect();
        assert_eq!(cfs, [0x10, 0x21, 0x22]);
        assert_eq!(f.sent[0].data[1], 20);
        assert_eq!(&f.sent[2].data[1..], &[13, 14, 15, 16, 17, 18, 19]);
        assert!(a.is_empty(), "the fake does not answer this one");
    }

    /// A bus with an ECU that only broadcasts, a frame every millisecond.
    struct Chatty {
        t: u64,
    }

    impl CanLink for Chatty {
        fn send(&mut self, _: Frame) {}
        fn now_us(&self) -> u64 {
            self.t
        }
        fn recv(&mut self, _: u64) -> Option<Frame> {
            self.t += 1000;
            Some(frame(0x0C9, &[0; 8]))
        }
    }

    #[test]
    fn other_traffic_does_not_hold_the_wait_open() {
        let mut c = Chatty { t: 0 };
        let a = request(&mut c, 0x7DF, &[0x01, 0x00], &obd, Timing::default());
        assert!(a.is_empty());
        assert_eq!(c.t, 200_000, "gave up at the 200 ms deadline");
    }

    #[test]
    fn nothing_back_is_an_empty_answer() {
        let mut f = Fake::default();
        assert!(request(&mut f, 0x7DF, &[0x01, 0x99], &obd, Timing::default()).is_empty());
    }
}
