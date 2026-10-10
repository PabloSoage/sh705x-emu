//! An ELM327 v1.5 as diagnostic apps expect it, on the CAN bus of the emulated ECU.
//!
//! What it answers like an ELM327 does:
//! - `AT Z`, `WS`, `D`, `I`, `@1`, `RV`, `E0/1`, `L0/1`, `S0/1`, `H0/1`, `SP`/`TP` (any
//!   protocol is taken as 6, ISO 15765-4 CAN 11-bit 500 kbit/s), `DP`, `DPN`, `SH`,
//!   `CRA`, `CAF0/1`, `ST`, `AT0/1/2`, `M0/1`, `AL`, `NL`, `V0/1`, `CFC0/1`, `R0/1`, `PC`,
//!   `BI`, `FCSH`, `FCSD`, `FCSM`;
//! - a request in hex, with an optional response count digit; an empty line repeats the
//!   last request;
//! - answers formatted as an ELM327 prints them: with or without headers and spaces, and
//!   long answers as a byte count and numbered lines (`014`, `0: ...`, `1: ...`).
//!
//! Anything else answers `?`.

use crate::isotp::{self, Answer, Timing};
use crate::link::CanLink;
use sh7059::hcan::Frame;

/// The adapter's identification.
pub const ID: &str = "ELM327 v1.5";

/// Settings, as AT commands change them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Elm327 {
    /// Echo the command (AT E).
    pub echo: bool,
    /// End lines with CR LF instead of CR (AT L).
    pub linefeeds: bool,
    /// Spaces between bytes (AT S).
    pub spaces: bool,
    /// Show CAN IDs and every frame (AT H).
    pub headers: bool,
    /// CAN automatic formatting: ISO-TP framing done by the adapter (AT CAF).
    pub auto_format: bool,
    /// Transmit ID (AT SH); H'7DF, the OBD functional address, by default.
    pub tx_id: u32,
    /// Only this ID is listened to (AT CRA), instead of the OBD answers to `tx_id`.
    pub rx_id: Option<u32>,
    /// Answer timeout, ms (AT ST × 4).
    pub timeout_ms: u64,
    /// Battery voltage shown by AT RV, in millivolts.
    pub battery_mv: u32,
    last: String,
}

impl Default for Elm327 {
    fn default() -> Self {
        Self {
            echo: true,
            linefeeds: false,
            spaces: true,
            headers: false,
            auto_format: true,
            tx_id: 0x7DF,
            rx_id: None,
            timeout_ms: 200,
            battery_mv: 12_600,
            last: String::new(),
        }
    }
}

impl Elm327 {
    /// An adapter just powered up.
    pub fn new() -> Self {
        Self::default()
    }

    fn eol(&self) -> &'static str {
        if self.linefeeds { "\r\n" } else { "\r" }
    }

    /// Handles one line from the app (without its CR) and returns everything the adapter
    /// sends back, up to and including the `>` prompt.
    pub fn line(&mut self, input: &str, link: &mut dyn CanLink) -> String {
        let mut out = String::new();
        if self.echo {
            out.push_str(input);
            out.push_str(self.eol());
        }
        let mut cmd: String = input
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_uppercase();
        if cmd.is_empty() {
            cmd = self.last.clone();
        }
        let body = if let Some(at) = cmd.strip_prefix("AT") {
            self.at(at)
        } else {
            let r = self.request(&cmd, link);
            self.last = cmd;
            r
        };
        for l in &body {
            out.push_str(l);
            out.push_str(self.eol());
        }
        out.push_str(self.eol());
        out.push('>');
        out
    }

    fn at(&mut self, a: &str) -> Vec<String> {
        let ok = || vec!["OK".to_string()];
        let flag = |s: &str, p: &str| match s.strip_prefix(p) {
            Some("0") => Some(false),
            Some("1") => Some(true),
            _ => None,
        };
        if let Some(v) = flag(a, "E") {
            self.echo = v;
            return ok();
        }
        if let Some(v) = flag(a, "L") {
            self.linefeeds = v;
            return ok();
        }
        if let Some(v) = flag(a, "S") {
            self.spaces = v;
            return ok();
        }
        if let Some(v) = flag(a, "H") {
            self.headers = v;
            return ok();
        }
        if let Some(v) = flag(a, "CAF") {
            self.auto_format = v;
            return ok();
        }
        match a {
            "Z" | "WS" => {
                let battery = self.battery_mv;
                *self = Self::new();
                self.battery_mv = battery;
                return vec![String::new(), ID.to_string()];
            }
            "D" => {
                let battery = self.battery_mv;
                *self = Self::new();
                self.battery_mv = battery;
                return ok();
            }
            "I" => return vec![ID.to_string()],
            "@1" => return vec!["SH705x emulator".to_string()],
            "RV" => {
                return vec![format!(
                    "{}.{}V",
                    self.battery_mv / 1000,
                    self.battery_mv % 1000 / 100
                )];
            }
            "DP" => return vec!["ISO 15765-4 (CAN 11/500)".to_string()],
            "DPN" => return vec!["6".to_string()],
            "CRA" => {
                self.rx_id = None;
                return ok();
            }
            "AL" | "NL" | "PC" | "BI" | "M0" | "M1" | "V0" | "V1" | "R0" | "R1" | "CFC0"
            | "CFC1" | "AT0" | "AT1" | "AT2" | "FCSM0" | "FCSM1" => return ok(),
            _ => {}
        }
        let hex_arg = |p: &str| {
            a.strip_prefix(p)
                .filter(|h| !h.is_empty())
                .and_then(|h| u32::from_str_radix(h, 16).ok())
        };
        if a.starts_with("SP") || a.starts_with("TP") {
            return ok();
        }
        if let Some(v) = hex_arg("SH") {
            self.tx_id = v & 0x7FF;
            return ok();
        }
        if let Some(v) = hex_arg("CRA") {
            self.rx_id = Some(v & 0x7FF);
            return ok();
        }
        if let Some(v) = hex_arg("ST") {
            // AT ST counts in 4 ms; 0 restores the default.
            self.timeout_ms = if v == 0 { 200 } else { 4 * v as u64 };
            return ok();
        }
        if a.starts_with("FCSH") || a.starts_with("FCSD") {
            return ok();
        }
        vec!["?".to_string()]
    }

    /// Which IDs answer: the one set by AT CRA, or for a functional request the eight
    /// OBD answer IDs, or for a physical one its pair (+ 8).
    fn accepts(&self, id: u32) -> bool {
        match self.rx_id {
            Some(r) => id == r,
            None if self.tx_id == 0x7DF => (0x7E8..=0x7EF).contains(&id),
            None => id == self.tx_id + 8,
        }
    }

    fn request(&mut self, cmd: &str, link: &mut dyn CanLink) -> Vec<String> {
        if !cmd.chars().all(|c| c.is_ascii_hexdigit()) || cmd.len() < 2 {
            return vec!["?".to_string()];
        }
        // An odd digit at the end is how many answers to wait for.
        let (hex, want) = if cmd.len() % 2 == 1 {
            let (h, n) = cmd.split_at(cmd.len() - 1);
            (h, n.parse::<usize>().ok().filter(|&n| n > 0))
        } else {
            (cmd, None)
        };
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("checked hex"))
            .collect();
        if bytes.len() > 8 || (self.auto_format && bytes.len() > 7) {
            return vec!["?".to_string()];
        }
        // A functional request may draw answers from several ECUs; a physical one, or one
        // with a single receive ID, from one.
        let several = self.rx_id.is_none() && self.tx_id == 0x7DF;
        let timing = Timing {
            answer_us: self.timeout_ms * 1000,
            more_us: if several {
                (self.timeout_ms * 1000).min(50_000)
            } else {
                0
            },
        };
        let accept = |id: u32| self.accepts(id);
        if !self.auto_format {
            // Raw frames out and in.
            let mut d = [0u8; 8];
            d[..bytes.len()].copy_from_slice(&bytes);
            link.send(Frame {
                id: self.tx_id,
                extended: false,
                remote: false,
                dlc: 8,
                data: d,
            });
            let mut lines = Vec::new();
            while let Some(f) = link.recv(timing.answer_us) {
                if !f.extended && accept(f.id) {
                    lines.push(self.raw(&f));
                    if want.is_some_and(|n| lines.len() >= n) {
                        break;
                    }
                }
            }
            return if lines.is_empty() {
                vec!["NO DATA".to_string()]
            } else {
                lines
            };
        }
        let mut answers = isotp::request(link, self.tx_id, &bytes, &accept, timing);
        if let Some(n) = want {
            answers.truncate(n);
        }
        if answers.is_empty() {
            return vec!["NO DATA".to_string()];
        }
        answers.iter().flat_map(|a| self.format(a)).collect()
    }

    fn bytes(&self, b: &[u8]) -> String {
        // An ELM327 puts a space after every byte, the last one included.
        let sep = if self.spaces { " " } else { "" };
        b.iter().map(|x| format!("{x:02X}{sep}")).collect()
    }

    fn raw(&self, f: &Frame) -> String {
        let sep = if self.spaces { " " } else { "" };
        let body = self.bytes(&f.data[..f.dlc.min(8) as usize]);
        if self.headers {
            format!("{:03X}{sep}{body}", f.id)
        } else {
            body
        }
    }

    /// One answer as an ELM327 prints it.
    fn format(&self, a: &Answer) -> Vec<String> {
        if self.headers {
            return a.frames.iter().map(|f| self.raw(f)).collect();
        }
        if a.frames.len() == 1 {
            return vec![self.bytes(&a.payload)];
        }
        // Long answer: the length, then the first frame's 6 bytes and 7 per frame after.
        let mut lines = vec![format!("{:03X}", a.payload.len())];
        let sep = if self.spaces { " " } else { "" };
        let mut chunks = vec![&a.payload[..a.payload.len().min(6)]];
        chunks.extend(a.payload.get(6..).unwrap_or(&[]).chunks(7));
        for (k, c) in chunks.iter().enumerate() {
            lines.push(format!("{:X}:{sep}{}", k & 0xF, self.bytes(c)));
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// An ECU at 0x7E8 that answers 01 00 in one frame and 09 02 in three.
    #[derive(Default)]
    struct Ecu {
        out: VecDeque<Frame>,
        sent: Vec<Frame>,
    }

    fn f(id: u32, b: &[u8]) -> Frame {
        let mut d = [0u8; 8];
        d[..b.len()].copy_from_slice(b);
        Frame {
            id,
            extended: false,
            remote: false,
            dlc: 8,
            data: d,
        }
    }

    impl CanLink for Ecu {
        fn send(&mut self, fr: Frame) {
            self.sent.push(fr);
            match (fr.data[0], fr.data[1], fr.data[2]) {
                (0x02, 0x01, 0x00) => self
                    .out
                    .push_back(f(0x7E8, &[0x06, 0x41, 0x00, 0x98, 0x3B, 0x20, 0x13])),
                (0x02, 0x09, 0x02) => self
                    .out
                    .push_back(f(0x7E8, &[0x10, 0x14, 0x49, 0x02, 0x01, b'W', b'0', b'L'])),
                (0x30, _, _) => {
                    self.out.push_back(f(0x7E8, &[0x21, 1, 2, 3, 4, 5, 6, 7]));
                    self.out
                        .push_back(f(0x7E8, &[0x22, 8, 9, 10, 11, 12, 13, 14]));
                }
                (0x02, 0x1A, 0x90) if fr.id == 0x7E0 => {
                    self.out.push_back(f(0x7E8, &[0x03, 0x7F, 0x1A, 0x31]))
                }
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

    fn talk(elm: &mut Elm327, ecu: &mut Ecu, line: &str) -> String {
        elm.line(line, ecu)
    }

    #[test]
    fn a_session_as_an_app_starts_it() {
        let (mut e, mut ecu) = (Elm327::new(), Ecu::default());
        assert_eq!(talk(&mut e, &mut ecu, "ATZ"), "ATZ\r\rELM327 v1.5\r\r>");
        assert_eq!(talk(&mut e, &mut ecu, "ATE0"), "ATE0\rOK\r\r>");
        assert_eq!(
            talk(&mut e, &mut ecu, "atl0"),
            "OK\r\r>",
            "case does not matter"
        );
        assert_eq!(talk(&mut e, &mut ecu, "AT SP 0"), "OK\r\r>");
        assert_eq!(talk(&mut e, &mut ecu, "ATDPN"), "6\r\r>");
        assert_eq!(talk(&mut e, &mut ecu, "ATRV"), "12.6V\r\r>");
        assert_eq!(talk(&mut e, &mut ecu, "0100"), "41 00 98 3B 20 13 \r\r>");
        assert_eq!(
            talk(&mut e, &mut ecu, ""),
            "41 00 98 3B 20 13 \r\r>",
            "repeat"
        );
        assert_eq!(talk(&mut e, &mut ecu, "0105"), "NO DATA\r\r>");
        assert_eq!(talk(&mut e, &mut ecu, "ATXYZ"), "?\r\r>");
    }

    #[test]
    fn long_answers_headers_and_spaces() {
        let (mut e, mut ecu) = (Elm327::new(), Ecu::default());
        e.echo = false;
        assert_eq!(
            talk(&mut e, &mut ecu, "0902"),
            "014\r0: 49 02 01 57 30 4C \r1: 01 02 03 04 05 06 07 \r2: 08 09 0A 0B 0C 0D 0E \r\r>"
        );
        talk(&mut e, &mut ecu, "ATH1");
        talk(&mut e, &mut ecu, "ATS0");
        assert_eq!(
            talk(&mut e, &mut ecu, "0100"),
            "7E806410098 3B201300\r\r>".replace(' ', "")
        );
        talk(&mut e, &mut ecu, "ATL1");
        assert!(talk(&mut e, &mut ecu, "0100").ends_with("\r\n\r\n>"));
    }

    #[test]
    fn physical_addressing_and_receive_filter() {
        let (mut e, mut ecu) = (Elm327::new(), Ecu::default());
        e.echo = false;
        talk(&mut e, &mut ecu, "ATSH7E0");
        assert_eq!(talk(&mut e, &mut ecu, "1A90"), "7F 1A 31 \r\r>");
        assert_eq!(ecu.sent.last().unwrap().id, 0x7E0);
        talk(&mut e, &mut ecu, "ATCRA7E9");
        assert_eq!(
            talk(&mut e, &mut ecu, "1A90"),
            "NO DATA\r\r>",
            "0x7E8 filtered out"
        );
        talk(&mut e, &mut ecu, "ATCRA");
        talk(&mut e, &mut ecu, "ATCAF0");
        assert_eq!(
            talk(&mut e, &mut ecu, "021A90"),
            "03 7F 1A 31 00 00 00 00 \r\r>"
        );
    }
}
