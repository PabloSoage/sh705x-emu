//! The oscilloscope: the ECU's pins as waveforms, in CPU cycles.
//!
//! - **Inputs** (crank on TI0A, cam on TI0B, MAF on TI0C): every edge the chip takes, at
//!   its own cycle ([`sh7059::Sh7059::edge_log`]): exact.
//! - **PWM outputs** of ATU-II channels 6 and 7: the timer's waveform (period, high time and
//!   phase, from its counter and registers), kept each time it changes. Between changes
//!   the edges are exact; a change is placed when the emulator next looks, within a slice
//!   of the run (half a millisecond at most in real time, an instruction stepping).
//! - **One-shot outputs** of channel 8 (the injectors on TO8I–L): each pulse as the timer
//!   ran it, exact.
//!
//! [`Osc::trace`] gives a channel's level and edges over a window, [`Osc::trigger`] the
//! last edge to hold a window still on, and [`measure`] its period, frequency and duty.

use sh7059::Machine;
use sh7059::atu::Wave;
use sh7059::pins::{Edge, Pin};
use std::collections::VecDeque;

/// A pin the oscilloscope shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    /// An input pin.
    Input(Pin),
    /// A PWM output of channel 6 or 7 (0 or 1), timer 0..3 = A..D.
    Pwm(u8, u8),
    /// A one-shot output of channel 8, 0 = TO8A .. 15 = TO8P.
    OneShot(u8),
}

/// The channels, with their names on the Astra's board (resultados fases 5, 6, 9, 11).
pub const CHANNELS: [(Channel, &str); 16] = [
    (Channel::Input(Pin::Ti0(0)), "crank (TI0A)"),
    (Channel::Input(Pin::Ti0(1)), "cam (TI0B)"),
    (Channel::Input(Pin::Ti0(2)), "MAF (TI0C)"),
    (Channel::Pwm(0, 0), "SCV (6A)"),
    (Channel::Pwm(0, 1), "throttle (6B)"),
    (Channel::Pwm(0, 2), "6C"),
    (Channel::Pwm(0, 3), "6D"),
    (Channel::Pwm(1, 0), "VGT (7A)"),
    (Channel::Pwm(1, 1), "swirl flaps (7B)"),
    (Channel::Pwm(1, 2), "EGR close (7C)"),
    (Channel::Pwm(1, 3), "EGR open (7D)"),
    (Channel::OneShot(8), "injector 1 (TO8I)"),
    (Channel::OneShot(9), "injector 2 (TO8J)"),
    (Channel::OneShot(10), "injector 3 (TO8K)"),
    (Channel::OneShot(11), "injector 4 (TO8L)"),
    (Channel::OneShot(0), "TO8A"),
];

/// Waveform changes kept per PWM output.
const WAVES_KEPT: usize = 4096;
/// One-shot pulses kept.
const PULSES_KEPT: usize = 65_536;

/// What the pins did.
#[derive(Clone, Debug, Default)]
pub struct Osc {
    /// Input edges, oldest first.
    pub edges: VecDeque<Edge>,
    /// Per PWM output (4 × channel + timer): from which cycle each waveform holds (`None`:
    /// the timer stopped).
    pub waves: [VecDeque<(u64, Option<Wave>)>; 8],
    /// One-shot pulses: output, start, end (cycles).
    pub pulses: VecDeque<(u8, u64, u64)>,
    /// End of the last pulse taken.
    last_pulse: u64,
    /// The cycle up to which it has looked.
    pub now: u64,
    /// The first cycle it has looked at.
    pub since: u64,
}

/// One channel over a window: its level at the start and every edge after (cycle, level
/// after it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trace {
    /// The level at the start of the window.
    pub initial: bool,
    /// The edges in the window.
    pub edges: Vec<(u64, bool)>,
}

/// What a trace measures: from its rising edges and its time high.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measure {
    /// Mean period, cycles.
    pub period: f64,
    /// Fraction of each period high.
    pub duty: f64,
    /// Rising edges counted.
    pub rises: usize,
}

impl Osc {
    /// Starts watching `m`'s pins.
    pub fn attach(m: &mut Machine) -> Self {
        m.chip.edge_log = Some(VecDeque::new());
        Self {
            now: m.cpu.cycles,
            since: m.cpu.cycles,
            last_pulse: m.cpu.cycles,
            ..Self::default()
        }
    }

    /// Takes what the pins did since the last time.
    pub fn take(&mut self, m: &mut Machine) {
        let now = m.cpu.cycles;
        if let Some(log) = m.chip.edge_log.as_mut() {
            self.edges.extend(log.drain(..));
            while self.edges.len() > sh7059::EDGES_KEPT {
                self.edges.pop_front();
            }
        }
        for (i, w) in self.waves.iter_mut().enumerate() {
            let wave = m.chip.pwm_wave(i / 4, i % 4);
            if w.back().is_none_or(|&(_, last)| last != wave) {
                if w.len() == WAVES_KEPT {
                    w.pop_front();
                }
                w.push_back((now, wave));
            }
        }
        for &(k, start, end) in &m.chip.atu.pulses {
            if end > self.last_pulse {
                if self.pulses.len() == PULSES_KEPT {
                    self.pulses.pop_front();
                }
                self.pulses.push_back((k, start, end));
                self.last_pulse = end;
            }
        }
        self.now = now;
    }

    /// `ch` over cycles `t0` to `t1`.
    pub fn trace(&self, ch: Channel, t0: u64, t1: u64) -> Trace {
        match ch {
            Channel::Input(pin) => {
                let mut initial = false;
                let mut edges = Vec::new();
                for e in self.edges.iter().filter(|e| e.pin == pin) {
                    if e.at <= t0 {
                        initial = e.high;
                    } else if e.at <= t1 {
                        edges.push((e.at, e.high));
                    } else {
                        break;
                    }
                }
                Trace { initial, edges }
            }
            Channel::OneShot(k) => {
                let mut initial = false;
                let mut edges = Vec::new();
                for &(_, a, b) in self.pulses.iter().filter(|p| p.0 == k) {
                    if a <= t0 && b > t0 {
                        initial = true;
                    }
                    if a > t0 && a <= t1 {
                        edges.push((a, true));
                    }
                    if b > t0 && b <= t1 {
                        edges.push((b, false));
                    }
                }
                edges.sort_unstable();
                Trace { initial, edges }
            }
            Channel::Pwm(c, k) => {
                let w = &self.waves[4 * c as usize + k as usize];
                let at = |t: u64| {
                    w.iter()
                        .rev()
                        .find(|&&(from, _)| from <= t)
                        .and_then(|&(_, wave)| wave)
                };
                let initial = at(t0).is_some_and(|wave| wave.level(t0));
                let mut edges = Vec::new();
                // Each stretch of one waveform inside the window.
                let mut level = initial;
                let starts: Vec<u64> = std::iter::once(t0)
                    .chain(
                        w.iter()
                            .map(|&(from, _)| from)
                            .filter(|&f| f > t0 && f <= t1),
                    )
                    .collect();
                for (n, &from) in starts.iter().enumerate() {
                    let to = starts.get(n + 1).copied().unwrap_or(t1);
                    let wave = at(from);
                    let here = wave.is_some_and(|wave| wave.level(from));
                    if here != level && from > t0 {
                        edges.push((from, here));
                        level = here;
                    }
                    let Some(wave) = wave else { continue };
                    if wave.period == 0 || wave.high == 0 || wave.high >= wave.period {
                        continue;
                    }
                    // The periods' starts from the first one at or after `from`.
                    let off = (from + wave.period - wave.origin) % wave.period;
                    // The period under way at `from` may have started before cycle 0.
                    let mut start = from as i128 - off as i128;
                    while start < to as i128 {
                        for (t, l) in [(start, true), (start + wave.high as i128, false)] {
                            if t > from as i128 && t <= to as i128 && l != level {
                                edges.push((t as u64, l));
                                level = l;
                            }
                        }
                        start += wave.period as i128;
                    }
                }
                Trace { initial, edges }
            }
        }
    }

    /// The last edge of `ch` (rising or not) that leaves `before` cycles of what the pins did
    /// before it and `after` after it: where a window held on it starts and ends.
    pub fn trigger(&self, ch: Channel, rising: bool, before: u64, after: u64) -> Option<u64> {
        let end = self.now.checked_sub(after)?;
        let start = self.since + before;
        if end <= start {
            return None;
        }
        // Look back a window's worth at a time, up to a second of ECU time.
        let span = (before + after).max(1);
        let mut hi = end;
        while hi > start && end - hi < 80_000_000 {
            let lo = hi.saturating_sub(span).max(start);
            let t = self.trace(ch, lo, hi);
            if let Some(&(at, _)) = t.edges.iter().rev().find(|&&(_, l)| l == rising) {
                return Some(at);
            }
            hi = lo;
        }
        None
    }
}

/// The period, duty and rising edges of `t` over `t0`..`t1`, with two rising edges at
/// least.
pub fn measure(t: &Trace, t0: u64, t1: u64) -> Option<Measure> {
    let rises: Vec<u64> = t.edges.iter().filter(|e| e.1).map(|e| e.0).collect();
    if rises.len() < 2 {
        return None;
    }
    let (first, last) = (rises[0], *rises.last().expect("two"));
    let period = (last - first) as f64 / (rises.len() - 1) as f64;
    // Time high between the first and last rising edges.
    let mut high = 0u64;
    let mut level = t.initial;
    let mut from = t0;
    for &(at, l) in &t.edges {
        if level {
            high += at.clamp(first, last) - from.clamp(first, last);
        }
        level = l;
        from = at;
    }
    if level {
        high += t1.clamp(first, last) - from.clamp(first, last);
    }
    Some(Measure {
        period,
        duty: high as f64 / (last - first) as f64,
        rises: rises.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pwm_trace_is_its_waveform_and_measures_as_one() {
        let mut o = Osc::default();
        // 1000 cycles a period, 250 high, periods starting at 100; from cycle 0.
        o.waves[0].push_back((
            0,
            Some(Wave {
                period: 1000,
                high: 250,
                origin: 100,
            }),
        ));
        let t = o.trace(Channel::Pwm(0, 0), 0, 3000);
        assert!(!t.initial);
        assert_eq!(
            t.edges[..4],
            [(100, true), (350, false), (1100, true), (1350, false)]
        );
        let m = measure(&t, 0, 3000).unwrap();
        assert_eq!(m.period, 1000.0);
        assert!((m.duty - 0.25).abs() < 1e-9, "{m:?}");
    }

    #[test]
    fn inputs_and_pulses_trace_as_taken() {
        let mut o = Osc::default();
        for (at, high) in [(10, true), (20, false), (30, true)] {
            o.edges.push_back(Edge {
                at,
                pin: Pin::Ti0(1),
                high,
            });
        }
        o.pulses.push_back((8, 5, 15));
        let t = o.trace(Channel::Input(Pin::Ti0(1)), 15, 40);
        assert_eq!((t.initial, t.edges), (true, vec![(20, false), (30, true)]));
        let p = o.trace(Channel::OneShot(8), 0, 40);
        assert_eq!((p.initial, p.edges), (false, vec![(5, true), (15, false)]));
        o.now = 100;
        assert_eq!(
            o.trigger(Channel::Input(Pin::Ti0(1)), true, 5, 10),
            Some(30)
        );
    }

    /// The Astra at idle: the crank's teeth, and the SCV's PWM with the duty the firmware
    /// gives it.
    #[test]
    fn the_astra_pins_at_idle() {
        let Ok(p) = std::env::var("SH705X_FLASH") else {
            return;
        };
        let f = std::fs::read(p).unwrap();
        let mut m = crate::machine(&f, None, board::DEFAULT_LAYOUT).unwrap();
        crate::Engine::idle().apply(&mut m);
        let mut s = crate::Session::new(m, crate::Engine::idle());
        s.set_plants(crate::PlantMode::Held);
        for _ in 0..1000 {
            s.run(40_000);
        }
        s.scope_pins(true);
        for _ in 0..1000 {
            s.run(40_000);
        }
        let o = s.osc.clone().unwrap();
        let o = o.lock().unwrap();
        let (t1, t0) = (o.now, o.now - 8_000_000);
        // 100 ms at 750 rpm: 1.25 turns of a 60-4 wheel, about 70 teeth.
        let crank = o.trace(Channel::Input(Pin::Ti0(0)), t0, t1);
        let rises = crank.edges.iter().filter(|e| e.1).count();
        assert!((60..=75).contains(&rises), "{rises}");
        let scv = o.trace(Channel::Pwm(0, 0), t0, t1);
        let m = measure(&scv, t0, t1).expect("the SCV's PWM");
        let duty = s.m.chip.pwm_duty(0, 0).unwrap();
        eprintln!(
            "SCV: {:.1} Hz, duty {:.3} (registers {duty:.3})",
            80e6 / m.period,
            m.duty
        );
        assert!((m.duty - duty).abs() < 0.02, "{m:?} {duty}");
    }
}
