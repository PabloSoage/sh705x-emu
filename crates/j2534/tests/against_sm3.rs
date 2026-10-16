//! The DLL's device against the emulator's own SM3 server, with an ECU that speaks
//! ISO-TP: long answers in (this side's flow control), long requests out (the ECU's).

use bridge::link::CanLink;
use bridge::sm3::{Sm3, session};
use sh705x_j2534::device::*;
use sh7059::hcan::Frame;
use std::collections::VecDeque;
use std::net::TcpListener;
use std::time::Duration;

/// Answers on 0x7E8 to requests on 0x7E0. A 0x22 read gets a 21-byte answer, which
/// waits for the tester's flow control; a long request gets the ECU's flow control and
/// then a single-frame answer with its length.
#[derive(Default)]
struct Ecu {
    out: VecDeque<Frame>,
    rx: Vec<u8>,
    rx_total: usize,
    pending: Vec<u8>,
}

impl Ecu {
    fn say(&mut self, data: &[u8]) {
        let mut d = data.to_vec();
        d.resize(8, 0x00);
        self.out.push_back(Frame::standard(0x7E8, &d));
    }

    fn answer(&mut self, req: &[u8]) {
        if req[0] == 0x22 {
            let mut a = vec![0x62, req[1], req[2]];
            a.extend((0..18).map(|k| 0xA0 + k as u8));
            self.say(&[0x10, a.len() as u8, a[0], a[1], a[2], a[3], a[4], a[5]]);
            self.pending = a[6..].to_vec();
        } else {
            self.say(&[0x02, req[0] + 0x40, req.len() as u8]);
        }
    }
}

impl CanLink for Ecu {
    fn send(&mut self, f: Frame) {
        if f.id != 0x7E0 {
            return;
        }
        let d = &f.data[..f.dlc as usize];
        match d[0] >> 4 {
            0 => {
                let n = (d[0] & 0x0F) as usize;
                self.answer(&d[1..1 + n]);
            }
            1 => {
                self.rx_total = (((d[0] & 0x0F) as usize) << 8) | d[1] as usize;
                self.rx = d[2..].to_vec();
                self.say(&[0x30, 0x00, 0x00]);
            }
            2 => {
                let room = self.rx_total - self.rx.len();
                self.rx.extend(&d[1..d.len().min(1 + room)]);
                if self.rx.len() == self.rx_total {
                    let req = std::mem::take(&mut self.rx);
                    self.answer(&req);
                }
            }
            3 => {
                let rest = std::mem::take(&mut self.pending);
                for (k, c) in rest.chunks(7).enumerate() {
                    let mut f = vec![0x20 | ((k + 1) as u8 & 0x0F)];
                    f.extend(c);
                    self.say(&f);
                }
            }
            _ => {}
        }
    }
    fn recv(&mut self, _: u64) -> Option<Frame> {
        self.out.pop_front()
    }
    fn now_us(&self) -> u64 {
        0
    }
}

fn server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        let (s, _) = listener.accept().unwrap();
        let _ = session(s, &mut Sm3::default(), &mut Ecu::default());
    });
    addr
}

fn msg(data: &[u8]) -> Msg {
    Msg {
        protocol: ISO15765,
        tx_flags: ISO15765_FRAME_PAD,
        data: data.to_vec(),
        ..Msg::default()
    }
}

#[test]
fn iso15765_both_ways_through_the_sm3_frame() {
    let dev = Device::open(&server(), None).expect("the server greets");
    assert_eq!(dev.battery_mv().unwrap(), 12_600);
    let ch = dev.connect(ISO15765, 500_000).unwrap();
    assert_eq!(
        dev.write(ch, &msg(&[0, 0, 7, 0xE0, 0x3E, 0x00])),
        Err(Error::NoFlowControl)
    );
    dev.start_filter(
        ch,
        Filter {
            kind: FLOW_CONTROL_FILTER,
            mask: vec![0xFF; 4],
            pattern: vec![0, 0, 7, 0xE8],
            flow: vec![0, 0, 7, 0xE0],
            tx_flags: ISO15765_FRAME_PAD,
        },
    )
    .unwrap();

    // A long answer: the first-frame indication, then the whole message.
    dev.write(ch, &msg(&[0, 0, 7, 0xE0, 0x22, 0xF1, 0x90]))
        .unwrap();
    let (got, r) = dev.read(ch, 2, Duration::from_secs(2));
    assert_eq!(r, Ok(()), "{got:?}");
    assert_eq!(
        got[0].rx_status & ISO15765_FIRST_FRAME,
        ISO15765_FIRST_FRAME
    );
    assert_eq!(got[0].data, [0, 0, 7, 0xE8]);
    let mut want = vec![0, 0, 7, 0xE8, 0x62, 0xF1, 0x90];
    want.extend((0..18).map(|k| 0xA0 + k as u8));
    assert_eq!(got[1].data, want);

    // A long request: the ECU's flow control lets the consecutive frames go.
    let mut long = vec![0, 0, 7, 0xE0, 0x2E, 0xF1, 0x90];
    long.extend(1..=17u8);
    dev.write(ch, &msg(&long)).unwrap();
    let (got, r) = dev.read(ch, 1, Duration::from_secs(2));
    assert_eq!(r, Ok(()));
    assert_eq!(got[0].data, [0, 0, 7, 0xE8, 0x6E, 20]);

    // Nothing more: an empty read with no wait says so.
    let (got, r) = dev.read(ch, 1, Duration::ZERO);
    assert!(got.is_empty());
    assert_eq!(r, Err(Error::BufferEmpty));
}

#[test]
fn a_can_channel_sees_the_bus_through_its_pass_filter() {
    let dev = Device::open(&server(), None).unwrap();
    let iso = dev.connect(ISO15765, 500_000).unwrap();
    dev.start_filter(
        iso,
        Filter {
            kind: FLOW_CONTROL_FILTER,
            mask: vec![0xFF; 4],
            pattern: vec![0, 0, 7, 0xE8],
            flow: vec![0, 0, 7, 0xE0],
            tx_flags: ISO15765_FRAME_PAD,
        },
    )
    .unwrap();
    let can = dev.connect(CAN, 500_000).unwrap();
    dev.start_filter(
        can,
        Filter {
            kind: PASS_FILTER,
            mask: vec![0xFF, 0xFF, 0xFF, 0xF0],
            pattern: vec![0, 0, 7, 0xE0],
            flow: vec![],
            tx_flags: 0,
        },
    )
    .unwrap();
    dev.write(iso, &msg(&[0, 0, 7, 0xE0, 0x3E, 0x00])).unwrap();
    let (got, r) = dev.read(can, 1, Duration::from_secs(2));
    assert_eq!(r, Ok(()));
    assert_eq!(&got[0].data[..7], &[0, 0, 7, 0xE8, 0x02, 0x7E, 0x02]);
    dev.set_config(can, LOOPBACK, 1).unwrap();
    assert_eq!(dev.get_config(can, LOOPBACK), Ok(1));
}
