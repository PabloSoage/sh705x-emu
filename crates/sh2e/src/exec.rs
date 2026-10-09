//! Instruction execution.
//!
//! Semantics follow manual REJ09B0177-0500 tables 2.12 to 2.17 and section 6. Where the
//! manual is silent, the reading used is noted and counted in [`crate::Unverified`].
//!
//! "PC" in a PC-relative operand is the address of the instruction plus 4, or the branch
//! target plus 2 in a delay slot (SH-2E software manual REJ09B0316-0200, MOVA).

use crate::cpu::{SR_M, SR_MASK, SR_Q, SR_S, vector};
use crate::decode::{DECODE, Op, disp4, imm8, m, n, sdisp12, simm8};
use crate::{Bus, Cpu};

/// What happens to the PC after an instruction.
pub(crate) enum Flow {
    /// Continue with the next instruction.
    Next,
    /// Jump now (BT, BF).
    Jump(u32),
    /// Jump after executing the delay slot.
    Delayed(u32),
    /// The instruction already set the PC (exception taken).
    Done,
}

#[inline(always)]
fn sext8(v: u8) -> u32 {
    v as i8 as i32 as u32
}

#[inline(always)]
fn sext16(v: u16) -> u32 {
    v as i16 as i32 as u32
}

/// Minimum execution cycles from tables 2.12 to 2.19 (no wait states, no contention).
/// Taken conditional branches add their extra cycles where they are executed.
const fn base_cycles(op: Op) -> u64 {
    match op {
        Op::Bra | Op::Braf | Op::Bsr | Op::Bsrf | Op::Jmp | Op::Jsr | Op::Rts => 2,
        Op::DmulsL | Op::DmuluL | Op::MulL => 2,
        Op::MacL | Op::MacW => 3,
        Op::AndB | Op::OrB | Op::XorB | Op::TstB => 3,
        Op::LdcLSr | Op::LdcLGbr | Op::LdcLVbr | Op::Sleep => 3,
        Op::StcLSr | Op::StcLGbr | Op::StcLVbr => 2,
        Op::TasB | Op::Rte => 4,
        Op::Trapa => 8,
        Op::Fdiv => 13,
        _ => 1,
    }
}

impl Cpu {
    /// Executes one instruction, or a delayed branch together with its delay slot, and
    /// takes any exception it raises. Does nothing but count a cycle while asleep.
    pub fn step<B: Bus>(&mut self, bus: &mut B) {
        if self.sleeping {
            self.cycles += 1;
            return;
        }
        let pc = self.pc;
        self.irq_block = false;
        if pc & 1 != 0 {
            self.raise(bus, vector::CPU_ADDRESS_ERROR, pc);
            return;
        }
        let code = bus.fetch16(pc);
        let op = DECODE[code as usize];
        self.instructions += 1;
        self.cycles += base_cycles(op);
        let next = pc.wrapping_add(2);
        let mut block = op.blocks_interrupt();
        match self.exec(bus, op, code, pc, next) {
            Flow::Next => self.pc = next,
            Flow::Jump(t) => self.pc = t,
            Flow::Delayed(t) => block = self.delay_slot(bus, next, t),
            Flow::Done => {}
        }
        if self.addr_error {
            self.addr_error = false;
            let resume = self.pc;
            self.raise(bus, vector::CPU_ADDRESS_ERROR, resume);
        }
        self.irq_block = block;
    }

    /// Runs up to `count` instructions (delay slots included) or until asleep. Returns the
    /// number executed.
    pub fn run<B: Bus>(&mut self, bus: &mut B, count: u64) -> u64 {
        let start = self.instructions;
        while self.instructions - start < count && !self.sleeping {
            self.step(bus);
        }
        self.instructions - start
    }

    /// Executes the instruction in the delay slot at `slot`, then jumps to `target`.
    /// Returns whether the slot instruction blocks interrupts.
    fn delay_slot<B: Bus>(&mut self, bus: &mut B, slot: u32, target: u32) -> bool {
        let code = bus.fetch16(slot);
        let op = DECODE[code as usize];
        self.instructions += 1;
        self.cycles += base_cycles(op);
        if op.illegal_in_slot() {
            // Manual 6.5.3: the saved PC is the branch's jump address.
            self.raise(bus, vector::SLOT_ILLEGAL, target);
            return false;
        }
        // In a delay slot, the PC a PC-relative instruction sees is the branch target + 2
        // (software manual, MOV @(disp,PC) and MOVA), so the slot executes as if it sat
        // at target - 2: that gives "PC" = target + 2.
        match self.exec(bus, op, code, target.wrapping_sub(2), target) {
            Flow::Next => self.pc = target,
            Flow::Done => {}
            Flow::Jump(_) | Flow::Delayed(_) => unreachable!("branches are illegal in a slot"),
        }
        op.blocks_interrupt()
    }

    /// Executes `op` at `addr`. `next` is where execution continues afterwards: `addr + 2`,
    /// or the branch target for a delay slot. Exceptions raised here save `next`.
    #[inline(always)]
    pub(crate) fn exec<B: Bus>(
        &mut self,
        bus: &mut B,
        op: Op,
        code: u16,
        addr: u32,
        next: u32,
    ) -> Flow {
        let n = n(code);
        let m = m(code);
        let pc4 = addr.wrapping_add(4);
        match op {
            Op::Illegal => {
                // Manual 6.5.4: the saved PC is the address of the undefined code.
                self.raise(bus, vector::GENERAL_ILLEGAL, addr);
                return Flow::Done;
            }

            // Data transfer
            Op::MovImm => self.r[n] = simm8(code),
            Op::MovWPc => {
                let a = pc4.wrapping_add(imm8(code) * 2);
                self.r[n] = sext16(self.rd16(bus, a));
            }
            Op::MovLPc => {
                let a = (pc4 & !3).wrapping_add(imm8(code) * 4);
                self.r[n] = self.rd32(bus, a);
            }
            Op::Mov => self.r[n] = self.r[m],
            Op::MovBStore => self.wr8(bus, self.r[n], self.r[m]),
            Op::MovWStore => self.wr16(bus, self.r[n], self.r[m]),
            Op::MovLStore => self.wr32(bus, self.r[n], self.r[m]),
            Op::MovBLoad => self.r[n] = sext8(self.rd8(bus, self.r[m])),
            Op::MovWLoad => self.r[n] = sext16(self.rd16(bus, self.r[m])),
            Op::MovLLoad => self.r[n] = self.rd32(bus, self.r[m]),
            Op::MovBStoreDec | Op::MovWStoreDec | Op::MovLStoreDec => {
                let size = match op {
                    Op::MovBStoreDec => 1,
                    Op::MovWStoreDec => 2,
                    _ => 4,
                };
                // Software manual 7.2.30: the write uses Rm before Rn is decremented, so
                // with m = n the original value is stored.
                let v = self.r[m];
                let a = self.r[n].wrapping_sub(size);
                self.r[n] = a;
                match size {
                    1 => self.wr8(bus, a, v),
                    2 => self.wr16(bus, a, v),
                    _ => self.wr32(bus, a, v),
                }
            }
            Op::MovBLoadInc => {
                self.r[n] = sext8(self.rd8(bus, self.r[m]));
                if n != m {
                    self.r[m] = self.r[m].wrapping_add(1);
                }
            }
            Op::MovWLoadInc => {
                self.r[n] = sext16(self.rd16(bus, self.r[m]));
                if n != m {
                    self.r[m] = self.r[m].wrapping_add(2);
                }
            }
            Op::MovLLoadInc => {
                self.r[n] = self.rd32(bus, self.r[m]);
                if n != m {
                    self.r[m] = self.r[m].wrapping_add(4);
                }
            }
            // 10000000nnnndddd and 10000001nnnndddd: the register is in bits 7..4.
            Op::MovBStoreDisp => {
                let a = self.r[m].wrapping_add(disp4(code));
                self.wr8(bus, a, self.r[0]);
            }
            Op::MovWStoreDisp => {
                let a = self.r[m].wrapping_add(disp4(code) * 2);
                self.wr16(bus, a, self.r[0]);
            }
            Op::MovLStoreDisp => {
                let a = self.r[n].wrapping_add(disp4(code) * 4);
                self.wr32(bus, a, self.r[m]);
            }
            Op::MovBLoadDisp => {
                let a = self.r[m].wrapping_add(disp4(code));
                self.r[0] = sext8(self.rd8(bus, a));
            }
            Op::MovWLoadDisp => {
                let a = self.r[m].wrapping_add(disp4(code) * 2);
                self.r[0] = sext16(self.rd16(bus, a));
            }
            Op::MovLLoadDisp => {
                let a = self.r[m].wrapping_add(disp4(code) * 4);
                self.r[n] = self.rd32(bus, a);
            }
            Op::MovBStoreIdx => {
                let a = self.r[0].wrapping_add(self.r[n]);
                self.wr8(bus, a, self.r[m]);
            }
            Op::MovWStoreIdx => {
                let a = self.r[0].wrapping_add(self.r[n]);
                self.wr16(bus, a, self.r[m]);
            }
            Op::MovLStoreIdx => {
                let a = self.r[0].wrapping_add(self.r[n]);
                self.wr32(bus, a, self.r[m]);
            }
            Op::MovBLoadIdx => {
                let a = self.r[0].wrapping_add(self.r[m]);
                self.r[n] = sext8(self.rd8(bus, a));
            }
            Op::MovWLoadIdx => {
                let a = self.r[0].wrapping_add(self.r[m]);
                self.r[n] = sext16(self.rd16(bus, a));
            }
            Op::MovLLoadIdx => {
                let a = self.r[0].wrapping_add(self.r[m]);
                self.r[n] = self.rd32(bus, a);
            }
            Op::MovBStoreGbr => {
                let a = self.gbr.wrapping_add(imm8(code));
                self.wr8(bus, a, self.r[0]);
            }
            Op::MovWStoreGbr => {
                let a = self.gbr.wrapping_add(imm8(code) * 2);
                self.wr16(bus, a, self.r[0]);
            }
            Op::MovLStoreGbr => {
                let a = self.gbr.wrapping_add(imm8(code) * 4);
                self.wr32(bus, a, self.r[0]);
            }
            Op::MovBLoadGbr => {
                let a = self.gbr.wrapping_add(imm8(code));
                self.r[0] = sext8(self.rd8(bus, a));
            }
            Op::MovWLoadGbr => {
                let a = self.gbr.wrapping_add(imm8(code) * 2);
                self.r[0] = sext16(self.rd16(bus, a));
            }
            Op::MovLLoadGbr => {
                let a = self.gbr.wrapping_add(imm8(code) * 4);
                self.r[0] = self.rd32(bus, a);
            }
            Op::Mova => self.r[0] = (pc4 & !3).wrapping_add(imm8(code) * 4),
            Op::Movt => self.r[n] = self.t(),
            Op::SwapB => {
                let v = self.r[m];
                self.r[n] = (v & 0xFFFF_0000) | ((v & 0xFF) << 8) | ((v >> 8) & 0xFF);
            }
            Op::SwapW => self.r[n] = self.r[m].rotate_left(16),
            Op::Xtrct => self.r[n] = (self.r[n] >> 16) | (self.r[m] << 16),

            // Arithmetic
            Op::Add => self.r[n] = self.r[n].wrapping_add(self.r[m]),
            Op::AddImm => self.r[n] = self.r[n].wrapping_add(simm8(code)),
            Op::Addc => {
                let (s1, c1) = self.r[n].overflowing_add(self.r[m]);
                let (s2, c2) = s1.overflowing_add(self.t());
                self.r[n] = s2;
                self.set_t(c1 | c2);
            }
            Op::Addv => {
                let (s, o) = (self.r[n] as i32).overflowing_add(self.r[m] as i32);
                self.r[n] = s as u32;
                self.set_t(o);
            }
            Op::CmpEqImm => self.set_t(self.r[0] == simm8(code)),
            Op::CmpEq => self.set_t(self.r[n] == self.r[m]),
            Op::CmpHs => self.set_t(self.r[n] >= self.r[m]),
            Op::CmpGe => self.set_t(self.r[n] as i32 >= self.r[m] as i32),
            Op::CmpHi => self.set_t(self.r[n] > self.r[m]),
            Op::CmpGt => self.set_t(self.r[n] as i32 > self.r[m] as i32),
            Op::CmpPl => self.set_t(self.r[n] as i32 > 0),
            Op::CmpPz => self.set_t(self.r[n] as i32 >= 0),
            Op::CmpStr => {
                let x = self.r[n] ^ self.r[m];
                let any = x & 0xFF00_0000 == 0
                    || x & 0x00FF_0000 == 0
                    || x & 0x0000_FF00 == 0
                    || x & 0x0000_00FF == 0;
                self.set_t(any);
            }
            Op::Div0s => {
                let q = self.r[n] >> 31;
                let mm = self.r[m] >> 31;
                self.sr = (self.sr & !(SR_Q | SR_M)) | (q << 8) | (mm << 9);
                self.set_t(q != mm);
            }
            Op::Div0u => {
                self.sr &= !(SR_Q | SR_M);
                self.set_t(false);
            }
            Op::Div1 => self.div1(n, m),
            Op::DmulsL => {
                let p = (self.r[n] as i32 as i64).wrapping_mul(self.r[m] as i32 as i64);
                self.mach = (p >> 32) as u32;
                self.macl = p as u32;
            }
            Op::DmuluL => {
                let p = (self.r[n] as u64) * (self.r[m] as u64);
                self.mach = (p >> 32) as u32;
                self.macl = p as u32;
            }
            Op::Dt => {
                self.r[n] = self.r[n].wrapping_sub(1);
                self.set_t(self.r[n] == 0);
            }
            Op::ExtsB => self.r[n] = sext8(self.r[m] as u8),
            Op::ExtsW => self.r[n] = sext16(self.r[m] as u16),
            Op::ExtuB => self.r[n] = self.r[m] & 0xFF,
            Op::ExtuW => self.r[n] = self.r[m] & 0xFFFF,
            Op::MacL => self.mac_l(bus, n, m),
            Op::MacW => self.mac_w(bus, n, m),
            Op::MulL => self.macl = self.r[n].wrapping_mul(self.r[m]),
            Op::MulsW => {
                self.macl = ((self.r[n] as i16 as i32) * (self.r[m] as i16 as i32)) as u32;
            }
            Op::MuluW => self.macl = (self.r[n] & 0xFFFF) * (self.r[m] & 0xFFFF),
            Op::Neg => self.r[n] = 0u32.wrapping_sub(self.r[m]),
            Op::Negc => {
                let tmp = 0u32.wrapping_sub(self.r[m]);
                let res = tmp.wrapping_sub(self.t());
                self.r[n] = res;
                self.set_t(0 < tmp || tmp < res);
            }
            Op::Sub => self.r[n] = self.r[n].wrapping_sub(self.r[m]),
            Op::Subc => {
                let tmp0 = self.r[n];
                let tmp1 = tmp0.wrapping_sub(self.r[m]);
                let res = tmp1.wrapping_sub(self.t());
                self.r[n] = res;
                self.set_t(tmp0 < tmp1 || tmp1 < res);
            }
            Op::Subv => {
                let (s, o) = (self.r[n] as i32).overflowing_sub(self.r[m] as i32);
                self.r[n] = s as u32;
                self.set_t(o);
            }

            // Logic
            Op::And => self.r[n] &= self.r[m],
            Op::AndImm => self.r[0] &= imm8(code),
            Op::Or => self.r[n] |= self.r[m],
            Op::OrImm => self.r[0] |= imm8(code),
            Op::Xor => self.r[n] ^= self.r[m],
            Op::XorImm => self.r[0] ^= imm8(code),
            Op::Not => self.r[n] = !self.r[m],
            Op::Tst => self.set_t(self.r[n] & self.r[m] == 0),
            Op::TstImm => self.set_t(self.r[0] & imm8(code) == 0),
            Op::AndB | Op::OrB | Op::XorB | Op::TstB => {
                let a = self.gbr.wrapping_add(self.r[0]);
                let v = self.rd8(bus, a) as u32;
                let i = imm8(code);
                match op {
                    Op::AndB => self.wr8(bus, a, v & i),
                    Op::OrB => self.wr8(bus, a, v | i),
                    Op::XorB => self.wr8(bus, a, v ^ i),
                    _ => self.set_t(v & i == 0),
                }
            }
            Op::TasB => {
                let a = self.r[n];
                let v = self.rd8(bus, a);
                self.set_t(v == 0);
                self.wr8(bus, a, (v | 0x80) as u32);
            }

            // Shifts
            Op::Rotl => {
                let v = self.r[n];
                self.set_t(v >> 31 != 0);
                self.r[n] = v.rotate_left(1);
            }
            Op::Rotr => {
                let v = self.r[n];
                self.set_t(v & 1 != 0);
                self.r[n] = v.rotate_right(1);
            }
            Op::Rotcl => {
                let v = self.r[n];
                self.r[n] = (v << 1) | self.t();
                self.set_t(v >> 31 != 0);
            }
            Op::Rotcr => {
                let v = self.r[n];
                self.r[n] = (v >> 1) | (self.t() << 31);
                self.set_t(v & 1 != 0);
            }
            Op::Shal | Op::Shll => {
                let v = self.r[n];
                self.set_t(v >> 31 != 0);
                self.r[n] = v << 1;
            }
            Op::Shar => {
                let v = self.r[n];
                self.set_t(v & 1 != 0);
                self.r[n] = ((v as i32) >> 1) as u32;
            }
            Op::Shlr => {
                let v = self.r[n];
                self.set_t(v & 1 != 0);
                self.r[n] = v >> 1;
            }
            Op::Shll2 => self.r[n] <<= 2,
            Op::Shlr2 => self.r[n] >>= 2,
            Op::Shll8 => self.r[n] <<= 8,
            Op::Shlr8 => self.r[n] >>= 8,
            Op::Shll16 => self.r[n] <<= 16,
            Op::Shlr16 => self.r[n] >>= 16,

            // Branches
            Op::Bt | Op::Bf => {
                if (self.t() != 0) == (op == Op::Bt) {
                    self.cycles += 2;
                    return Flow::Jump(pc4.wrapping_add(simm8(code) << 1));
                }
            }
            Op::Bts | Op::Bfs => {
                if (self.t() != 0) == (op == Op::Bts) {
                    self.cycles += 1;
                    return Flow::Delayed(pc4.wrapping_add(simm8(code) << 1));
                }
            }
            Op::Bra => return Flow::Delayed(pc4.wrapping_add(sdisp12(code) << 1)),
            Op::Bsr => {
                self.pr = pc4;
                return Flow::Delayed(pc4.wrapping_add(sdisp12(code) << 1));
            }
            // 0000mmmm00100011, 0000mmmm00000011, 0100mmmm00101011, 0100mmmm00001011:
            // the register is in bits 11..8.
            Op::Braf => return Flow::Delayed(pc4.wrapping_add(self.r[n])),
            Op::Bsrf => {
                let t = pc4.wrapping_add(self.r[n]);
                self.pr = pc4;
                return Flow::Delayed(t);
            }
            Op::Jmp => return Flow::Delayed(self.r[n]),
            Op::Jsr => {
                let t = self.r[n];
                self.pr = pc4;
                return Flow::Delayed(t);
            }
            Op::Rts => return Flow::Delayed(self.pr),
            Op::Rte => {
                let sp = self.r[15];
                let t = self.rd32(bus, sp);
                let sr = self.rd32(bus, sp.wrapping_add(4));
                self.r[15] = sp.wrapping_add(8);
                self.sr = sr & SR_MASK;
                return Flow::Delayed(t);
            }

            // System control. Register fields of LDC/LDS (0100mmmm...) are in bits 11..8.
            Op::Clrt => self.set_t(false),
            Op::Sett => self.set_t(true),
            Op::Clrmac => {
                self.mach = 0;
                self.macl = 0;
            }
            Op::Nop => {}
            Op::Sleep => self.sleeping = true,
            Op::LdcSr => self.sr = self.r[n] & SR_MASK,
            Op::LdcGbr => self.gbr = self.r[n],
            Op::LdcVbr => self.vbr = self.r[n],
            Op::LdsMach => self.mach = self.r[n],
            Op::LdsMacl => self.macl = self.r[n],
            Op::LdsPr => self.pr = self.r[n],
            Op::LdcLSr => self.sr = self.pop(bus, n) & SR_MASK,
            Op::LdcLGbr => self.gbr = self.pop(bus, n),
            Op::LdcLVbr => self.vbr = self.pop(bus, n),
            Op::LdsLMach => self.mach = self.pop(bus, n),
            Op::LdsLMacl => self.macl = self.pop(bus, n),
            Op::LdsLPr => self.pr = self.pop(bus, n),
            Op::StcSr => self.r[n] = self.sr,
            Op::StcGbr => self.r[n] = self.gbr,
            Op::StcVbr => self.r[n] = self.vbr,
            Op::StsMach => self.r[n] = self.mach,
            Op::StsMacl => self.r[n] = self.macl,
            Op::StsPr => self.r[n] = self.pr,
            Op::StcLSr => self.push(bus, n, self.sr),
            Op::StcLGbr => self.push(bus, n, self.gbr),
            Op::StcLVbr => self.push(bus, n, self.vbr),
            Op::StsLMach => self.push(bus, n, self.mach),
            Op::StsLMacl => self.push(bus, n, self.macl),
            Op::StsLPr => self.push(bus, n, self.pr),
            Op::Trapa => {
                // Manual 6.5.2: the saved PC is the instruction after TRAPA.
                self.raise(bus, imm8(code), next);
                return Flow::Done;
            }

            // FPU
            _ => return self.exec_fpu(bus, op, code, next),
        }
        Flow::Next
    }

    /// `(Rn) -> value, Rn + 4 -> Rn`, as LDC.L and LDS.L do.
    #[inline(always)]
    pub(crate) fn pop<B: Bus>(&mut self, bus: &mut B, n: usize) -> u32 {
        let a = self.r[n];
        let v = self.rd32(bus, a);
        self.r[n] = a.wrapping_add(4);
        v
    }

    /// `Rn - 4 -> Rn, value -> (Rn)`, as STC.L and STS.L do.
    #[inline(always)]
    pub(crate) fn push<B: Bus>(&mut self, bus: &mut B, n: usize, v: u32) {
        let a = self.r[n].wrapping_sub(4);
        self.r[n] = a;
        self.wr32(bus, a, v);
    }

    /// DIV1: one step of non-restoring division (the SH algorithm, with Q, M and T).
    fn div1(&mut self, n: usize, m: usize) {
        let old_q = self.sr & SR_Q != 0;
        let mm = self.sr & SR_M != 0;
        let mut q = self.r[n] & 0x8000_0000 != 0;
        let divisor = self.r[m];
        let shifted = (self.r[n] << 1) | self.t();
        let (res, carry) = if old_q == mm {
            let r = shifted.wrapping_sub(divisor);
            (r, r > shifted)
        } else {
            let r = shifted.wrapping_add(divisor);
            (r, r < shifted)
        };
        self.r[n] = res;
        // The new Q is the shifted-out bit combined with the borrow (subtract) or carry
        // (add) of this step, inverted when M = 1.
        q = if mm { !(q ^ carry) } else { q ^ carry };
        self.sr = (self.sr & !SR_Q) | ((q as u32) << 8);
        self.set_t(q == mm);
    }

    /// MAC.L @Rm+,@Rn+: (Rn) x (Rm) + MAC -> MAC, signed 32 x 32 + 64.
    ///
    /// With S = 1 the sum saturates to 48 bits signed ([`crate::Unverified::mac_saturation`]).
    fn mac_l<B: Bus>(&mut self, bus: &mut B, n: usize, m: usize) {
        let a = self.r[n];
        let vn = self.rd32(bus, a) as i32 as i64;
        self.r[n] = a.wrapping_add(4);
        let b = self.r[m];
        let vm = self.rd32(bus, b) as i32 as i64;
        self.r[m] = b.wrapping_add(4);
        let mac = ((self.mach as u64) << 32 | self.macl as u64) as i64;
        let mut sum = mac.wrapping_add(vn * vm);
        if self.sr & SR_S != 0 {
            self.unverified.mac_saturation += 1;
            sum = sum.clamp(-(1i64 << 47), (1i64 << 47) - 1);
        }
        self.mach = (sum >> 32) as u32;
        self.macl = sum as u32;
    }

    /// MAC.W @Rm+,@Rn+: (Rn) x (Rm) + MAC -> MAC, signed 16 x 16 + 64.
    ///
    /// With S = 1 only MACL accumulates, saturating to 32 bits signed
    /// ([`crate::Unverified::mac_saturation`]).
    fn mac_w<B: Bus>(&mut self, bus: &mut B, n: usize, m: usize) {
        let a = self.r[n];
        let vn = self.rd16(bus, a) as i16 as i64;
        self.r[n] = a.wrapping_add(2);
        let b = self.r[m];
        let vm = self.rd16(bus, b) as i16 as i64;
        self.r[m] = b.wrapping_add(2);
        if self.sr & SR_S != 0 {
            self.unverified.mac_saturation += 1;
            let sum = (self.macl as i32 as i64 + vn * vm).clamp(i32::MIN as i64, i32::MAX as i64);
            self.macl = sum as u32;
        } else {
            let mac = ((self.mach as u64) << 32 | self.macl as u64) as i64;
            let sum = mac.wrapping_add(vn * vm);
            self.mach = (sum >> 32) as u32;
            self.macl = sum as u32;
        }
    }
}
