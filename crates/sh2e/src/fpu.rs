//! The SH-2E floating-point unit (manual section 3 and table 2.18).
//!
//! What the manual fixes, and this follows exactly:
//! - single precision, **rounding toward zero only** (FPSCR.RM = 01, not writable);
//! - **denormals flushed to zero**, as operands and as results, in every operation that
//!   generates a value (FPSCR.DN = 1, not writable);
//! - only invalid-operation (V) and division-by-zero (Z) are detected; overflow gives the
//!   largest finite value and underflow gives zero, both with the right sign;
//! - **NaN convention opposite to the host's**: bit 22 set means *signaling* (3.3.2);
//! - an enabled exception leaves the destination unchanged and jumps through vector 13
//!   with the address of the next instruction saved (6.5.5);
//! - two documented deviations from IEEE 754 with infinities (3.6).
//!
//! What the manual leaves to the SH-2E Software Manual (not at hand) is implemented on the
//! closest documented behaviour and counted in [`crate::Unverified`].
//!
//! Host arithmetic is only used where it is exact or where the exact result can be
//! recovered (TwoSum, fused multiply-add remainder), so the host's round-to-nearest never
//! leaks into a result.

use crate::cpu::vector;
use crate::decode::{Op, m, n};
use crate::exec::Flow;
use crate::{Bus, Cpu};

/// The quiet NaN produced by invalid operations: bit 22 clear means quiet on the SH-2E.
pub const QNAN: u32 = 0x7FBF_FFFF;
const SIGN: u32 = 0x8000_0000;
const INF: u32 = 0x7F80_0000;
const MAX: u32 = 0x7F7F_FFFF;
const ONE: u32 = 0x3F80_0000;

const FLAG_Z: u32 = 1 << 5;
const FLAG_V: u32 = 1 << 6;
const ENABLE_Z: u32 = 1 << 10;
const ENABLE_V: u32 = 1 << 11;
const CAUSE_Z: u32 = 1 << 15;
const CAUSE_V: u32 = 1 << 16;
/// FPSCR bits LDS can write: CV, CZ, EV, EZ, FV, FZ (figure 3.2).
pub const FPSCR_WRITABLE: u32 = CAUSE_V | CAUSE_Z | ENABLE_V | ENABLE_Z | FLAG_V | FLAG_Z;
/// FPSCR bits fixed at 1: DN (bit 18) and RM = 01.
pub const FPSCR_FIXED: u32 = 0x0004_0001;

#[inline(always)]
fn is_nan(x: u32) -> bool {
    x & INF == INF && x & 0x007F_FFFF != 0
}

#[inline(always)]
fn is_snan(x: u32) -> bool {
    is_nan(x) && x & 0x0040_0000 != 0
}

#[inline(always)]
fn is_inf(x: u32) -> bool {
    x & !SIGN == INF
}

/// A denormal becomes a zero of the same sign.
#[inline(always)]
fn flush(x: u32) -> u32 {
    if x & INF == 0 { x & SIGN } else { x }
}

/// The value of a flushed, finite or infinite operand.
#[inline(always)]
fn val(x: u32) -> f64 {
    f32::from_bits(x) as f64
}

/// Rounds a finite `f64` toward zero to single precision: results below the smallest
/// normal flush to signed zero, results beyond the largest finite saturate to it.
fn rz32(v: f64) -> u32 {
    let sign = if v.is_sign_negative() { SIGN } else { 0 };
    let a = v.abs();
    if a >= f32::MAX as f64 {
        return sign | MAX;
    }
    if a < f32::MIN_POSITIVE as f64 {
        return sign;
    }
    let b = a.to_bits();
    let e = (((b >> 52) & 0x7FF) as i32 - 1023 + 127) as u32;
    let frac = ((b >> 29) & 0x007F_FFFF) as u32;
    sign | (e << 23) | frac
}

/// Rounds the exact value `s + err` toward zero, where `s` is the host's nearest `f64`
/// and `err` the exact remainder (same sign as the true error).
fn rz_exact(s: f64, err: f64) -> u32 {
    let t = rz32(s);
    if err != 0.0 && f32::from_bits(t) as f64 == s && (err < 0.0) != (s < 0.0) {
        // s is representable, and the exact value lies just inside it: one step toward zero.
        flush(t - 1)
    } else {
        t
    }
}

/// Exact sum of two `f64`: `(s, err)` with `s + err == a + b` exactly (Knuth's TwoSum).
fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let s = a + b;
    let bb = s - a;
    let err = (a - (s - bb)) + (b - bb);
    (s, err)
}

/// Result of an arithmetic FPU operation before it is committed.
struct Out {
    value: u32,
    invalid: bool,
    div_zero: bool,
}

impl Out {
    fn ok(value: u32) -> Self {
        Out {
            value,
            invalid: false,
            div_zero: false,
        }
    }
    fn invalid(value: u32) -> Self {
        Out {
            value,
            invalid: true,
            div_zero: false,
        }
    }
}

impl Cpu {
    fn nan_out(&mut self, ops: &[u32]) -> Option<Out> {
        if ops.iter().any(|&x| is_nan(x)) {
            self.unverified.nan_operand += 1;
            let snan = ops.iter().any(|&x| is_snan(x));
            return Some(Out {
                value: QNAN,
                invalid: snan,
                div_zero: false,
            });
        }
        None
    }

    /// FRn + FRm.
    fn fadd(&mut self, frn: u32, frm: u32) -> Out {
        if let Some(o) = self.nan_out(&[frn, frm]) {
            return o;
        }
        // Manual 3.6 (1): FADD with FRm = -INF and FRn = MAX gives +INF on this FPU.
        if frm == SIGN | INF && frn == MAX {
            self.unverified.inf_max_quirk += 1;
            return Out::ok(INF);
        }
        let (a, b) = (flush(frn), flush(frm));
        match (is_inf(a), is_inf(b)) {
            (true, true) if (a ^ b) & SIGN != 0 => return Out::invalid(QNAN),
            (true, _) => return Out::ok(a),
            (_, true) => return Out::ok(b),
            _ => {}
        }
        let (s, err) = two_sum(val(a), val(b));
        Out::ok(rz_exact(s, err))
    }

    /// FRn - FRm.
    fn fsub(&mut self, frn: u32, frm: u32) -> Out {
        if is_nan(frm) {
            return self.fadd(frn, frm);
        }
        // Manual 3.6 (2): FSUB with FRm = +INF and FRn = MAX gives +INF on this FPU.
        if frm == INF && frn == MAX {
            self.unverified.inf_max_quirk += 1;
            return Out::ok(INF);
        }
        self.fadd(frn, frm ^ SIGN)
    }

    /// FRn x FRm. The product of two singles is exact in double precision.
    fn fmul(&mut self, frn: u32, frm: u32) -> Out {
        if let Some(o) = self.nan_out(&[frn, frm]) {
            return o;
        }
        let (a, b) = (flush(frn), flush(frm));
        let sign = (a ^ b) & SIGN;
        let zero = |x: u32| x & !SIGN == 0;
        if (is_inf(a) && zero(b)) || (zero(a) && is_inf(b)) {
            return Out::invalid(QNAN);
        }
        if is_inf(a) || is_inf(b) {
            return Out::ok(sign | INF);
        }
        Out::ok(rz32(val(a) * val(b)))
    }

    /// FRn / FRm.
    fn fdiv(&mut self, frn: u32, frm: u32) -> Out {
        if let Some(o) = self.nan_out(&[frn, frm]) {
            return o;
        }
        let (a, b) = (flush(frn), flush(frm));
        let sign = (a ^ b) & SIGN;
        let zero = |x: u32| x & !SIGN == 0;
        if (zero(a) && zero(b)) || (is_inf(a) && is_inf(b)) {
            return Out::invalid(QNAN);
        }
        if zero(b) {
            return Out {
                value: sign | INF,
                invalid: false,
                div_zero: true,
            };
        }
        if is_inf(a) {
            return Out::ok(sign | INF);
        }
        if is_inf(b) || zero(a) {
            return Out::ok(sign);
        }
        // Truncating the host's nearest double quotient is exact here. With 24-bit
        // significands, a quotient a/b that is not itself a single lies at least about
        // 2^-48 (relative) from every single: a - v*b, for a single v, is a non-zero
        // multiple of the smallest unit of a and v*b. Round-to-nearest in double moves it
        // by at most 2^-53, so it never lands on or crosses a single.
        Out::ok(rz32(val(a) / val(b)))
    }

    /// FR0 x FRm + FRn, computed fused (one rounding).
    fn fmac(&mut self, fr0: u32, frm: u32, frn: u32) -> Out {
        if let Some(o) = self.nan_out(&[fr0, frm, frn]) {
            return o;
        }
        let (a, b, c) = (flush(fr0), flush(frm), flush(frn));
        let zero = |x: u32| x & !SIGN == 0;
        if (is_inf(a) && zero(b)) || (zero(a) && is_inf(b)) {
            return Out::invalid(QNAN);
        }
        if is_inf(a) || is_inf(b) {
            let p = ((a ^ b) & SIGN) | INF;
            if is_inf(c) && (p ^ c) & SIGN != 0 {
                return Out::invalid(QNAN);
            }
            return Out::ok(p);
        }
        if is_inf(c) {
            return Out::ok(c);
        }
        let p = val(a) * val(b);
        let (s, err) = two_sum(p, val(c));
        let fused = rz_exact(s, err);
        let (s2, err2) = two_sum(val(rz32(p)), val(c));
        if rz_exact(s2, err2) != fused {
            self.unverified.fmac_fused_differs += 1;
        }
        Out::ok(fused)
    }

    /// FCMP/EQ and FCMP/GT: (result for T, invalid).
    fn fcmp(&mut self, frn: u32, frm: u32, gt: bool) -> (bool, bool) {
        if is_nan(frn) || is_nan(frm) {
            self.unverified.nan_operand += 1;
            return (false, is_snan(frn) || is_snan(frm));
        }
        let (a, b) = (val(flush(frn)), val(flush(frm)));
        (if gt { a > b } else { a == b }, false)
    }

    /// FTRC: truncation to a signed 32-bit integer. Out of range and NaN saturate and set V
    /// ([`crate::Unverified::ftrc_out_of_range`]).
    fn ftrc(&mut self, x: u32) -> (u32, bool) {
        if is_nan(x) {
            self.unverified.ftrc_out_of_range += 1;
            return (0x8000_0000, true);
        }
        let v = val(flush(x));
        if v >= 2_147_483_648.0 {
            self.unverified.ftrc_out_of_range += 1;
            return (0x7FFF_FFFF, true);
        }
        if v < -2_147_483_648.0 {
            self.unverified.ftrc_out_of_range += 1;
            return (0x8000_0000, true);
        }
        (v.trunc() as i32 as u32, false)
    }

    /// Updates the cause and flag fields for an instruction that can raise V or Z, and
    /// says whether the exception is enabled (manual 3.2.3 and 6.5.5).
    fn fpu_status(&mut self, invalid: bool, div_zero: bool) -> bool {
        self.fpscr &= !(CAUSE_V | CAUSE_Z);
        if invalid {
            self.fpscr |= CAUSE_V | FLAG_V;
        }
        if div_zero {
            self.fpscr |= CAUSE_Z | FLAG_Z;
        }
        (invalid && self.fpscr & ENABLE_V != 0) || (div_zero && self.fpscr & ENABLE_Z != 0)
    }

    /// Commits an arithmetic result to FRn, or takes the FPU exception.
    fn fpu_commit<B: Bus>(&mut self, bus: &mut B, n: usize, out: Out, next: u32) -> Flow {
        if self.fpu_status(out.invalid, out.div_zero) {
            self.raise(bus, vector::FPU, next);
            return Flow::Done;
        }
        self.fr[n] = out.value;
        Flow::Next
    }

    pub(crate) fn exec_fpu<B: Bus>(&mut self, bus: &mut B, op: Op, code: u16, next: u32) -> Flow {
        let n = n(code);
        let m = m(code);
        match op {
            Op::Fadd => {
                let o = self.fadd(self.fr[n], self.fr[m]);
                return self.fpu_commit(bus, n, o, next);
            }
            Op::Fsub => {
                let o = self.fsub(self.fr[n], self.fr[m]);
                return self.fpu_commit(bus, n, o, next);
            }
            Op::Fmul => {
                let o = self.fmul(self.fr[n], self.fr[m]);
                return self.fpu_commit(bus, n, o, next);
            }
            Op::Fdiv => {
                let o = self.fdiv(self.fr[n], self.fr[m]);
                return self.fpu_commit(bus, n, o, next);
            }
            Op::Fmac => {
                let o = self.fmac(self.fr[0], self.fr[m], self.fr[n]);
                return self.fpu_commit(bus, n, o, next);
            }
            Op::FcmpEq | Op::FcmpGt => {
                let (t, invalid) = self.fcmp(self.fr[n], self.fr[m], op == Op::FcmpGt);
                if self.fpu_status(invalid, false) {
                    self.raise(bus, vector::FPU, next);
                    return Flow::Done;
                }
                self.set_t(t);
            }
            Op::Fneg | Op::Fabs => {
                let x = self.fr[n];
                if is_nan(x) {
                    self.unverified.fneg_fabs_nan += 1;
                }
                self.fpu_status(false, false);
                self.fr[n] = if op == Op::Fneg { x ^ SIGN } else { x & !SIGN };
            }
            // FLDS and FTRC: 1111mmmm00x11101, the register is in bits 11..8.
            Op::Ftrc => {
                let (v, invalid) = self.ftrc(self.fr[n]);
                if self.fpu_status(invalid, false) {
                    self.raise(bus, vector::FPU, next);
                    return Flow::Done;
                }
                self.fpul = v;
            }
            Op::Float => {
                self.fr[n] = rz32(self.fpul as i32 as f64);
            }
            Op::Flds => self.fpul = self.fr[n],
            Op::Fsts => self.fr[n] = self.fpul,
            Op::Fldi0 => self.fr[n] = 0,
            Op::Fldi1 => self.fr[n] = ONE,
            Op::Fmov => self.fr[n] = self.fr[m],
            Op::FmovLoad => self.fr[n] = self.rd32(bus, self.r[m]),
            Op::FmovLoadInc => {
                let a = self.r[m];
                self.fr[n] = self.rd32(bus, a);
                self.r[m] = a.wrapping_add(4);
            }
            Op::FmovLoadIdx => {
                let a = self.r[0].wrapping_add(self.r[m]);
                self.fr[n] = self.rd32(bus, a);
            }
            Op::FmovStore => self.wr32(bus, self.r[n], self.fr[m]),
            Op::FmovStoreDec => {
                let a = self.r[n].wrapping_sub(4);
                self.r[n] = a;
                self.wr32(bus, a, self.fr[m]);
            }
            Op::FmovStoreIdx => {
                let a = self.r[0].wrapping_add(self.r[n]);
                self.wr32(bus, a, self.fr[m]);
            }
            // FPU-related CPU instructions (table 2.19): register in bits 11..8.
            Op::LdsFpscr => self.fpscr = (self.r[n] & FPSCR_WRITABLE) | FPSCR_FIXED,
            Op::LdsFpul => self.fpul = self.r[n],
            Op::LdsLFpscr => {
                let v = self.pop(bus, n);
                self.fpscr = (v & FPSCR_WRITABLE) | FPSCR_FIXED;
            }
            Op::LdsLFpul => self.fpul = self.pop(bus, n),
            Op::StsFpscr => self.r[n] = self.fpscr,
            Op::StsFpul => self.r[n] = self.fpul,
            Op::StsLFpscr => self.push(bus, n, self.fpscr),
            Op::StsLFpul => self.push(bus, n, self.fpul),
            _ => unreachable!("exec_fpu called with {op:?}"),
        }
        Flow::Next
    }
}

#[cfg(test)]
impl Cpu {
    pub(crate) fn fadd_bits(&mut self, a: u32, b: u32) -> u32 {
        self.fadd(a, b).value
    }
    pub(crate) fn fsub_bits(&mut self, a: u32, b: u32) -> u32 {
        self.fsub(a, b).value
    }
    pub(crate) fn fmul_bits(&mut self, a: u32, b: u32) -> u32 {
        self.fmul(a, b).value
    }
    pub(crate) fn fdiv_bits(&mut self, a: u32, b: u32) -> u32 {
        self.fdiv(a, b).value
    }
    pub(crate) fn fmac_bits(&mut self, fr0: u32, frm: u32, frn: u32) -> u32 {
        self.fmac(fr0, frm, frn).value
    }
    pub(crate) fn float_bits(&mut self, x: u32) -> u32 {
        rz32(x as i32 as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(x: f32) -> u32 {
        x.to_bits()
    }

    #[test]
    fn rz32_truncates_toward_zero() {
        // 1 + 2^-24 is halfway; RN gives 1.0 (even), RZ too. 1 + 3*2^-25 rounds up in RN.
        let v = 1.0 + 3.0 * 2f64.powi(-25);
        assert_eq!(rz32(v), bits(1.0));
        assert_eq!(
            v as f32,
            f32::from_bits(bits(1.0) + 1),
            "the host rounds up here"
        );
        assert_eq!(rz32(-v), bits(-1.0));
    }

    #[test]
    fn rz32_flushes_and_saturates() {
        assert_eq!(rz32(1e-40), 0);
        assert_eq!(rz32(-1e-40), SIGN);
        assert_eq!(rz32(1e39), MAX);
        assert_eq!(rz32(-1e39), SIGN | MAX);
    }

    #[test]
    fn add_rounds_toward_zero_where_the_host_rounds_up() {
        let mut c = Cpu::new();
        // 1 + (2^-24 + 2^-30): the exact sum is above the midpoint; RN rounds up, RZ not.
        let x = 2f32.powi(-24) + 2f32.powi(-30);
        assert_eq!(c.fadd(bits(1.0), bits(x)).value, bits(1.0));
        assert_eq!((1.0f32 + x).to_bits(), bits(1.0) + 1);
    }

    #[test]
    fn add_steps_down_when_the_host_sum_is_exactly_representable() {
        let mut c = Cpu::new();
        // 2^24 + (1 - 2^-30)... in single precision 2^24 has ulp 2, so take a, b with
        // a + b = 2^25 - tiny: RN in f64 is exact here, so test a case where s is a
        // representable single and the error is negative.
        let a = bits(16_777_216.0); // 2^24
        let b = bits(-(2f32.powi(-30)));
        // Exact: 2^24 - 2^-30, just below 2^24: RZ gives the single below 2^24.
        assert_eq!(c.fadd(a, b).value, bits(16_777_216.0) - 1);
    }

    #[test]
    fn mul_and_div_round_toward_zero() {
        let mut c = Cpu::new();
        // 1/3 and 2/3 round up under round-to-nearest; toward zero they do not.
        assert_eq!(c.fdiv(bits(1.0), bits(3.0)).value, 0x3EAA_AAAA);
        assert_eq!(bits(1.0 / 3.0), 0x3EAA_AAAB);
        assert_eq!(c.fdiv(bits(2.0), bits(3.0)).value, 0x3F2A_AAAA);
        assert_eq!(c.fdiv(bits(-2.0), bits(3.0)).value, 0xBF2A_AAAA);
        assert_eq!(c.fdiv(bits(1.0), bits(4.0)).value, bits(0.25));
        let x = bits(1.0) + 1; // 1 + 2^-23
        let sq = c.fmul(x, x).value; // 1 + 2^-22 + 2^-46 -> 1 + 2^-22
        assert_eq!(sq, bits(1.0) + 2);
    }

    #[test]
    fn denormals_are_flushed() {
        let mut c = Cpu::new();
        let tiny = 0x0000_0001; // smallest positive denormal
        assert_eq!(c.fadd(bits(1.0), tiny).value, bits(1.0));
        assert_eq!(c.fmul(bits(1.0), tiny).value, 0);
        let min = bits(f32::MIN_POSITIVE);
        assert_eq!(
            c.fmul(min, bits(0.5)).value,
            0,
            "a denormal result is flushed"
        );
    }

    #[test]
    fn special_cases() {
        let mut c = Cpu::new();
        assert!(c.fadd(INF, SIGN | INF).invalid);
        assert_eq!(c.fadd(INF, SIGN | INF).value, QNAN);
        assert!(c.fmul(INF, 0).invalid);
        let d = c.fdiv(bits(1.0), SIGN);
        assert!(d.div_zero);
        assert_eq!(d.value, SIGN | INF);
        assert!(c.fdiv(0, 0).invalid);
        assert_eq!(c.fadd(bits(1.0), bits(-1.0)).value, 0, "x - x is +0");
        // A quiet NaN in gives the default quiet NaN, without an invalid operation.
        let q = c.fadd(QNAN, bits(1.0));
        assert_eq!((q.value, q.invalid), (QNAN, false));
        // A signaling NaN (bit 22 set on the SH-2E) is an invalid operation.
        assert!(c.fadd(0x7FC0_0000, bits(1.0)).invalid);
    }

    #[test]
    fn documented_inf_max_deviations() {
        let mut c = Cpu::new();
        assert_eq!(c.fadd(MAX, SIGN | INF).value, INF);
        assert_eq!(c.fsub(MAX, INF).value, INF);
        assert_eq!(c.fsub(bits(1.0), INF).value, SIGN | INF);
    }

    #[test]
    fn fmac_is_fused() {
        let mut c = Cpu::new();
        // (1 + 2^-23)^2 - (1 + 2^-22) = 2^-46 exactly when fused; 0 when rounded twice.
        let x = bits(1.0) + 1;
        let o = c.fmac(x, x, bits(-(1.0 + 2f32.powi(-22))));
        assert_eq!(o.value, bits(2f32.powi(-46)));
        assert_eq!(c.unverified.fmac_fused_differs, 1);
    }

    #[test]
    fn ftrc_and_float() {
        let mut c = Cpu::new();
        assert_eq!(c.ftrc(bits(-2.9)), (-2i32 as u32, false));
        assert_eq!(c.ftrc(bits(3e9)), (0x7FFF_FFFF, true));
        assert_eq!(c.ftrc(bits(-2_147_483_648.0)), (0x8000_0000, false));
        // 2^24 + 1 is not representable: RZ gives 2^24.
        assert_eq!(rz32(16_777_217_f64), bits(16_777_216.0));
        assert_eq!(rz32(-16_777_217_f64), bits(-16_777_216.0));
    }

    #[test]
    fn compare() {
        let mut c = Cpu::new();
        assert_eq!(c.fcmp(0, SIGN, false), (true, false), "+0 == -0");
        assert_eq!(c.fcmp(bits(2.0), bits(1.0), true), (true, false));
        assert_eq!(c.fcmp(QNAN, QNAN, false), (false, false));
        assert_eq!(c.fcmp(0x7FC0_0000, 0, true), (false, true));
        assert_eq!(
            c.fcmp(0x0000_0001, 0, false),
            (true, false),
            "denormal equals zero"
        );
    }
}
