//! Where a map lookup's inputs come from and where its result goes, read from the code
//! around the call: an input is a variable in RAM, the result of another call (another
//! map, or a routine such as the one that returns the engine speed), or computed there;
//! the result is stored in a variable, or used there.
//!
//! The code is read straight, without following its branches: from the call back to
//! where each input register was last written, through register moves, and from the
//! return forward to the first store of the result. It goes by the SH ABI: FR12-FR15
//! and R8-R14 survive a call, the others do not; a function returns FR0. A source can be
//! wrong where a branch joins in between; the viewer checks a variable against the
//! input it gave each time it can (see [`crate::maps::MapTrace`]).

/// Where a value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A float variable at this address.
    Ram(u32),
    /// The result of a lookup of this map (its descriptor).
    Map(u32),
    /// The result of a call to the routine at this address.
    Call(u32),
    /// Computed in the caller, or not followed.
    Unknown,
}

/// A call site, read from its code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Site {
    /// Where FR4 comes from.
    pub x: Source,
    /// Where FR5 comes from (for a 3D map).
    pub y: Source,
    /// Where FR0 is stored after the call, if it is.
    pub out: Option<u32>,
}

/// Instructions read back from a call at most: a function's prologue loads its inputs.
const BACK: u32 = 1024;
/// Instructions read forward from a return for the store of the result.
const FORWARD: u32 = 32;
/// Register moves followed at most.
const HOPS: usize = 8;

/// The code, a 16-bit word at a time.
struct Code<'a>(&'a dyn Fn(u32) -> u8);

impl Code<'_> {
    fn word(&self, a: u32) -> u16 {
        u16::from_be_bytes([(self.0)(a), (self.0)(a + 1)])
    }
    fn long(&self, a: u32) -> u32 {
        (0..4).fold(0, |v, k| v << 8 | (self.0)(a + k) as u32)
    }
    /// The literal a `MOV.L @(disp,PC),Rn` at `a` loads.
    fn literal(&self, a: u32, op: u16) -> u32 {
        self.long((a & !3) + 4 + (op & 0xFF) as u32 * 4)
    }
}

/// Whether `op` is a call (`JSR`, `BSR`, `BSRF`).
fn is_call(op: u16) -> bool {
    op & 0xF0FF == 0x400B || op & 0xF000 == 0xB000 || op & 0xF0FF == 0x0003
}

/// Whether `op` ends a function (`RTS`, `RTE`).
fn is_return(op: u16) -> bool {
    op == 0x000B || op == 0x002B
}

/// Whether `op` writes floating-point register `n`, and how: `Some(Some(m))` a move from
/// FRm, `Some(None)` anything else (loads are handled before).
fn writes_fr(op: u16, n: u16) -> Option<Option<u16>> {
    if op >> 12 != 0xF || (op >> 8) & 0xF != n {
        return None;
    }
    let (m, f) = ((op >> 4) & 0xF, op & 0xF);
    match f {
        0xC => Some(Some(m)),
        // FADD, FSUB, FMUL, FDIV, FMOV.S @(R0,Rm), @Rm, @Rm+, FMAC.
        0x0..=0x3 | 0x6 | 0x8 | 0x9 | 0xE => Some(None),
        // FSTS, FLOAT, FNEG, FABS, FSQRT, FLDI0, FLDI1.
        0xD if matches!(m, 0x0 | 0x2 | 0x4 | 0x5 | 0x6 | 0x8 | 0x9) => Some(None),
        _ => None,
    }
}

/// The value integer register `r` holds just before `at`, if it is a constant the code
/// sets: a PC-relative literal, moved, with immediates added.
fn int_before(code: &Code, at: u32, mut r: u16, stop: u32) -> Option<u32> {
    let mut add: u32 = 0;
    let mut hops = 0;
    let mut a = at;
    while a > stop {
        a -= 2;
        let op = code.word(a);
        if is_return(op) {
            return None;
        }
        if is_call(op) && r < 8 {
            return None;
        }
        let n = (op >> 8) & 0xF;
        if n != r {
            continue;
        }
        match op >> 12 {
            0xD => return Some(code.literal(a, op).wrapping_add(add)),
            0xE => return Some(((op & 0xFF) as i8 as i32 as u32).wrapping_add(add)),
            0x7 => add = add.wrapping_add((op & 0xFF) as i8 as i32 as u32),
            0x6 if op & 0xF == 0x3 => {
                hops += 1;
                if hops > HOPS {
                    return None;
                }
                r = (op >> 4) & 0xF;
            }
            // A load from memory, arithmetic, anything else that writes Rr.
            _ if writes_int(op, r) => return None,
            _ => {}
        }
    }
    None
}

/// Whether `op` (with Rn = `r` in bits 8-11) writes Rr: the forms that write Rn.
fn writes_int(op: u16, r: u16) -> bool {
    let n = (op >> 8) & 0xF;
    if n != r {
        return false;
    }
    match op >> 12 {
        // MOV.x @Rm,Rn and friends, EXTU/EXTS/NEG/NOT/SWAP, ADD/SUB/ADDC, ...
        0x6 | 0x3 => true,
        // STC/STS to Rn, MOVT.
        0x0 => matches!(
            op & 0xFF,
            0x02 | 0x12 | 0x22 | 0x0A | 0x1A | 0x2A | 0x29 | 0x5A | 0x6A
        ),
        // Shifts, DT, rotates.
        0x4 => matches!(
            op & 0xFF,
            0x00 | 0x01
                | 0x04
                | 0x05
                | 0x08
                | 0x09
                | 0x10
                | 0x18
                | 0x19
                | 0x20
                | 0x21
                | 0x24
                | 0x25
                | 0x28
                | 0x29
        ),
        0x5 | 0x9 => true,
        0x8 | 0xC => false,
        _ => false,
    }
}

/// Where floating-point register `n` comes from just before `at`.
fn fr_source(code: &Code, at: u32, mut n: u16, stop: u32, masters: [u32; 2]) -> Source {
    let mut hops = 0;
    let mut a = at;
    while a > stop {
        a -= 2;
        let op = code.word(a);
        if is_return(op) {
            return Source::Unknown;
        }
        if is_call(op) {
            // The delay slot after the call ran before it returned: it is part of the
            // way in, not of what comes back.
            if n == 0 {
                return call_result(code, a, op, stop, masters);
            }
            if n < 12 {
                return Source::Unknown;
            }
            continue;
        }
        // FMOV.S @Rm,FRn: a variable, if Rm is one the code sets.
        if op >> 12 == 0xF && (op >> 8) & 0xF == n && op & 0xF == 0x8 {
            let m = (op >> 4) & 0xF;
            return int_before(code, a, m, stop).map_or(Source::Unknown, Source::Ram);
        }
        match writes_fr(op, n) {
            Some(Some(m)) => {
                hops += 1;
                if hops > HOPS {
                    return Source::Unknown;
                }
                n = m;
            }
            Some(None) => return Source::Unknown,
            None => {}
        }
    }
    Source::Unknown
}

/// What FR0 is after the call at `a` (`op`): a map if it calls a master with a
/// descriptor in R4, else a call to its target.
fn call_result(code: &Code, a: u32, op: u16, stop: u32, masters: [u32; 2]) -> Source {
    let target = if op & 0xF000 == 0xB000 {
        let d = ((op & 0xFFF) << 4) as i16 >> 4;
        Some(a.wrapping_add(4).wrapping_add((2 * d as i32) as u32))
    } else if op & 0xF0FF == 0x400B {
        int_before(code, a, (op >> 8) & 0xF, stop)
    } else {
        None
    };
    // R4 may be set in the delay slot; else before the call (not by the call itself).
    let slot = code.word(a + 2);
    let r4 = if slot >> 12 == 0xD && (slot >> 8) & 0xF == 4 {
        Some(code.literal(a + 2, slot))
    } else {
        int_before(code, a, 4, stop)
    };
    match (target, r4) {
        (Some(t), Some(d)) if masters.contains(&t) => Source::Map(d),
        (Some(t), _) => Source::Call(t),
        _ => Source::Unknown,
    }
}

/// Reads the call site that returns to `ret` (the call is at `ret - 4`, its delay slot at
/// `ret - 2`), for a firmware whose interpolation routines are `masters`.
pub fn read(read: &dyn Fn(u32) -> u8, ret: u32, masters: [u32; 2]) -> Site {
    let code = Code(read);
    let stop = ret.saturating_sub(2 * BACK);
    // The inputs as they are when the call jumps: its delay slot first, then the code
    // before the call itself.
    let input = |n: u16| {
        let slot = ret - 2;
        let op = code.word(slot);
        let n = if op >> 12 == 0xF && (op >> 8) & 0xF == n && op & 0xF == 0x8 {
            let m = (op >> 4) & 0xF;
            return int_before(&code, slot, m, stop).map_or(Source::Unknown, Source::Ram);
        } else {
            match writes_fr(op, n) {
                Some(Some(m)) => m,
                Some(None) => return Source::Unknown,
                None => n,
            }
        };
        fr_source(&code, ret - 4, n, stop, masters)
    };
    let (x, y) = (input(4), input(5));
    // The result: the first store of FR0, or of a register it was moved to.
    let mut held = [false; 16];
    held[0] = true;
    let mut out = None;
    for k in 0..FORWARD {
        let a = ret + 2 * k;
        let op = code.word(a);
        if is_call(op) || is_return(op) || op & 0xF000 == 0xA000 || op & 0xFF00 == 0x8900 {
            break;
        }
        if op >> 12 == 0xF {
            let (n, m, f) = ((op >> 8) & 0xF, (op >> 4) & 0xF, op & 0xF);
            // FMOV.S FRm,@Rn
            if f == 0xA && held[m as usize] {
                out = int_before(&code, a, n, stop);
                break;
            }
            if f == 0xC && held[m as usize] {
                held[n as usize] = true;
                continue;
            }
            if writes_fr(op, n).is_some() || f == 0x8 {
                held[n as usize] = false;
            }
            if !held.iter().any(|&h| h) {
                break;
            }
        }
    }
    Site { x, y, out }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The boost target's call site in the Astra's flash (resultados fase 7), read by hand:
    /// FR4 from FR15, set from the result of an earlier call; FR5 from FR14, loaded from a
    /// variable in the prologue; the result stored at FFFEFE90.
    #[test]
    fn the_boost_target_site_reads_as_by_hand() {
        let Ok(p) = std::env::var("SH705X_FLASH") else {
            return;
        };
        let f = std::fs::read(p).unwrap();
        let read = |a: u32| f.get(a as usize).copied().unwrap_or(0);
        let s = super::read(&read, 0x3D856, [0x7128, 0x70B4]);
        assert_eq!(s.out, Some(0xFFFE_FE90), "{s:x?}");
        assert!(matches!(s.y, Source::Ram(_)), "{s:x?}");
        assert!(!matches!(s.x, Source::Unknown), "{s:x?}");
    }
}
