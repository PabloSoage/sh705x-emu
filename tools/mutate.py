#!/usr/bin/env python3
"""Mutation check for the sh2e core: every mutation must make `cargo test -p sh2e` fail.

Each mutation replaces one exact snippet in one file. A mutation that leaves the tests
green ("SURVIVED") marks behaviour that no test pins down. Files are always restored.

Usage: tools/mutate.py            (from the repository root)
"""
import pathlib
import subprocess
import sys

SRC = pathlib.Path("crates/sh2e/src")

# (file, original snippet, mutated snippet, what it breaks)
MUTATIONS = [
    ("exec.rs", "v as i8 as i32 as u32", "v as u32", "byte sign extension"),
    ("exec.rs", "let a = (pc4 & !3).wrapping_add(imm8(code) * 4);\n                self.r[n] = self.rd32(bus, a);",
     "let a = pc4.wrapping_add(imm8(code) * 4);\n                self.r[n] = self.rd32(bus, a);",
     "MOV.L @(disp,PC) alignment"),
    ("exec.rs", "if n != m {\n                    self.r[m] = self.r[m].wrapping_add(1);",
     "if true {\n                    self.r[m] = self.r[m].wrapping_add(1);", "post-increment with n = m"),
    ("exec.rs", "self.set_t(c1 | c2);", "self.set_t(c1);", "ADDC carry"),
    ("exec.rs", "self.set_t(0 < tmp || tmp < res);", "self.set_t(tmp < res);", "NEGC borrow"),
    ("exec.rs", "self.set_t(tmp0 < tmp1 || tmp1 < res);", "self.set_t(tmp0 < tmp1);", "SUBC borrow"),
    ("exec.rs", "q = if mm { !(q ^ carry) } else { q ^ carry };", "q ^= carry;", "DIV1 Q"),
    ("exec.rs", "self.set_t(q != mm);", "self.set_t(q == mm);", "DIV0S T"),
    ("exec.rs", "self.set_t(self.r[n] as i32 >= self.r[m] as i32)", "self.set_t(self.r[n] >= self.r[m])", "CMP/GE signed"),
    ("exec.rs", "self.r[n] = ((v as i32) >> 1) as u32;", "self.r[n] = v >> 1;", "SHAR arithmetic"),
    ("exec.rs", "self.r[n] = (v >> 1) | (self.t() << 31);", "self.r[n] = (v >> 1) | self.t();", "ROTCR"),
    ("exec.rs", "return Flow::Jump(pc4.wrapping_add(simm8(code) << 1));",
     "return Flow::Jump(pc4.wrapping_add(simm8(code)));", "BT/BF displacement scale"),
    ("exec.rs", "self.pr = pc4;\n                return Flow::Delayed(pc4.wrapping_add(sdisp12(code) << 1));",
     "self.pr = addr;\n                return Flow::Delayed(pc4.wrapping_add(sdisp12(code) << 1));", "BSR return address"),
    ("exec.rs", "self.raise(bus, vector::SLOT_ILLEGAL, target);", "self.raise(bus, vector::SLOT_ILLEGAL, slot);",
     "slot illegal saved PC"),
    ("exec.rs", "self.raise(bus, vector::GENERAL_ILLEGAL, addr);", "self.raise(bus, vector::GENERAL_ILLEGAL, next);",
     "general illegal saved PC"),
    ("exec.rs", "self.sr = sr & SR_MASK;\n                return Flow::Delayed(t);", "self.sr = sr;\n                return Flow::Delayed(t);",
     "RTE SR mask"),
    ("exec.rs", "Op::LdcSr => self.sr = self.r[n] & SR_MASK,", "Op::LdcSr => self.sr = self.r[n],", "LDC SR mask"),
    ("exec.rs", "self.wr8(bus, a, (v | 0x80) as u32);", "self.wr8(bus, a, v as u32);", "TAS.B sets bit 7"),
    ("exec.rs", "let mut block = op.blocks_interrupt();", "let mut block = false;", "no interrupt after LDC"),
    ("exec.rs", "Op::Rts => return Flow::Delayed(self.pr),", "Op::Rts => return Flow::Jump(self.pr),", "RTS delay slot"),
    ("exec.rs", "Op::Sett => self.set_t(true),", "Op::Sett => self.set_t(false),", "SETT"),
    ("exec.rs", "self.macl = ((self.r[n] as i16 as i32) * (self.r[m] as i16 as i32)) as u32;",
     "self.macl = ((self.r[n] as u16 as i32) * (self.r[m] as u16 as i32)) as u32;", "MULS.W signed"),
    ("cpu.rs", "bus.write32(sp, self.sr);", "bus.write32(sp, saved_pc);", "exception stacking order"),
    ("cpu.rs", "self.sr = (self.sr & !SR_I_MASK) | (level.min(15) << 4);",
     "self.sr = (self.sr & !SR_I_MASK) | (level.min(15) << 3);", "interrupt mask level"),
    ("cpu.rs", "!self.irq_block && (level >= 16 || level > self.interrupt_mask())",
     "!self.irq_block && (level >= 16 || level >= self.interrupt_mask())", "mask comparison"),
    ("fpu.rs", "flush(t - 1)", "t", "round toward zero, representable case"),
    ("fpu.rs", "let err = (a - (s - bb)) + (b - bb);", "let err = 0.0 * (a - (s - bb)) + 0.0 * (b - bb);", "TwoSum error term"),
    ("fpu.rs", "Out::ok(rz32(val(a) / val(b)))", "Out::ok(rz32(val(a) / val(b) * (1.0 - f64::EPSILON)))",
     "division rounding"),
    ("fpu.rs", "if x & INF == 0 { x & SIGN } else { x }", "x", "denormal flush"),
    ("fpu.rs", "pub const QNAN: u32 = 0x7FBF_FFFF;", "pub const QNAN: u32 = 0x7FFF_FFFF;", "SH quiet NaN pattern"),
    ("fpu.rs", "Op::LdsFpscr => self.fpscr = (self.r[n] & FPSCR_WRITABLE) | FPSCR_FIXED,",
     "Op::LdsFpscr => self.fpscr = self.r[n],", "FPSCR fixed bits"),
    ("fpu.rs", "let p = val(a) * val(b);\n        let (s, err) = two_sum(p, val(c));",
     "let p = val(rz32(val(a) * val(b)));\n        let (s, err) = two_sum(p, val(c));", "FMAC fused"),
    ("fpu.rs", "(if gt { a > b } else { a == b }, false)", "(if gt { a >= b } else { a == b }, false)", "FCMP/GT"),
    ("decode.rs", "                _ if mid != 0 => Op::Illegal,\n", "", "RTS/NOP need bits 11..8 clear"),
]


def main():
    survived = 0
    for name, old, new, what in MUTATIONS:
        path = SRC / name
        text = path.read_text(encoding="utf-8")
        if text.count(old) != 1:
            print(f"BAD MUTATION ({text.count(old)} matches): {what}")
            survived += 1
            continue
        path.write_text(text.replace(old, new), encoding="utf-8", newline="\n")
        try:
            r = subprocess.run(["cargo", "test", "-q", "-p", "sh2e", "--lib"],
                               capture_output=True, text=True)
        finally:
            path.write_text(text, encoding="utf-8", newline="\n")
        if r.returncode == 0:
            print(f"SURVIVED  {what}")
            survived += 1
        else:
            print(f"killed    {what}")
    print(f"\n{len(MUTATIONS) - survived} of {len(MUTATIONS)} killed")
    return 1 if survived else 0


if __name__ == "__main__":
    sys.exit(main())
