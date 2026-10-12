//! The oscilloscope, as the app reads it: one frame of the channels asked for, held on
//! the last edge of the trigger channel, or running free without one.

use crate::{Live, lock};
use session::osc::{CHANNELS, measure};

/// Numbers before the channels in [`Live::osc_frame`].
pub const OSC_HEADER: usize = 4;
/// CPU cycles a microsecond.
const PER_US: f64 = 80.0;

impl Live {
    /// The channels the oscilloscope knows, one name per line, in [`CHANNELS`] order.
    pub fn osc_channels() -> String {
        CHANNELS.iter().map(|c| c.1).collect::<Vec<_>>().join("\n")
    }

    /// A frame `span_us` wide of the channels whose bits are set in `mask` ([`CHANNELS`]
    /// order), held on the last edge (`rising` or falling) of channel `trigger` with
    /// `pre` of it before, or the latest `span_us` without one:
    ///
    /// - [`OSC_HEADER`] numbers: the window's span (µs), the trigger's time from the
    ///   window's start (µs, NaN running free), the channels that follow, 1 if held on a
    ///   trigger;
    /// - per channel: its index, its level at the start (1/0), its edges, then each edge's
    ///   time from the start (µs) and level, then its period (µs) and duty (NaN if it has
    ///   not two rising edges).
    ///
    /// Empty while the pins are not watched.
    pub fn osc_frame(
        &self,
        span_us: f64,
        trigger: usize,
        rising: bool,
        pre: f64,
        mask: u32,
    ) -> Vec<f64> {
        let Some(slot) = lock(&self.osc).clone() else {
            return Vec::new();
        };
        let o = lock(&slot);
        let span = (span_us * PER_US) as u64;
        let before = (span as f64 * pre.clamp(0.0, 1.0)) as u64;
        let trig = CHANNELS
            .get(trigger)
            .and_then(|c| o.trigger(c.0, rising, before, span - before));
        let t0 = match trig {
            Some(t) => t - before,
            None => o.now.saturating_sub(span),
        };
        let t1 = t0 + span;
        let shown: Vec<usize> = (0..CHANNELS.len()).filter(|k| mask >> k & 1 != 0).collect();
        let mut out = Vec::with_capacity(OSC_HEADER + 64);
        out.extend([
            span_us,
            trig.map_or(f64::NAN, |t| (t - t0) as f64 / PER_US),
            shown.len() as f64,
            trig.is_some() as u8 as f64,
        ]);
        for k in shown {
            let t = o.trace(CHANNELS[k].0, t0, t1);
            out.extend([k as f64, t.initial as u8 as f64, t.edges.len() as f64]);
            for &(at, l) in &t.edges {
                out.extend([(at - t0) as f64 / PER_US, l as u8 as f64]);
            }
            match measure(&t, t0, t1) {
                Some(m) => out.extend([m.period / PER_US, m.duty]),
                None => out.extend([f64::NAN, f64::NAN]),
            }
        }
        out
    }
}
