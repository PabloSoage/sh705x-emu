//! Instruction decoding: every 16-bit code maps to an [`Op`] through a table built at
//! compile time, so decoding at run time is one indexed load.
//!
//! Encodings are those of manual REJ09B0177-0500, tables 2.12 to 2.19. Codes the manual
//! does not list decode to [`Op::Illegal`].

/// One SH-2E instruction, without its operands (those stay in the code bits).
#[allow(missing_docs)] // variant names follow the manual's mnemonics
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    Illegal,
    // Data transfer (table 2.12)
    MovImm,
    MovWPc,
    MovLPc,
    Mov,
    MovBStore,
    MovWStore,
    MovLStore,
    MovBLoad,
    MovWLoad,
    MovLLoad,
    MovBStoreDec,
    MovWStoreDec,
    MovLStoreDec,
    MovBLoadInc,
    MovWLoadInc,
    MovLLoadInc,
    MovBStoreDisp,
    MovWStoreDisp,
    MovLStoreDisp,
    MovBLoadDisp,
    MovWLoadDisp,
    MovLLoadDisp,
    MovBStoreIdx,
    MovWStoreIdx,
    MovLStoreIdx,
    MovBLoadIdx,
    MovWLoadIdx,
    MovLLoadIdx,
    MovBStoreGbr,
    MovWStoreGbr,
    MovLStoreGbr,
    MovBLoadGbr,
    MovWLoadGbr,
    MovLLoadGbr,
    Mova,
    Movt,
    SwapB,
    SwapW,
    Xtrct,
    // Arithmetic (table 2.13)
    Add,
    AddImm,
    Addc,
    Addv,
    CmpEqImm,
    CmpEq,
    CmpHs,
    CmpGe,
    CmpHi,
    CmpGt,
    CmpPl,
    CmpPz,
    CmpStr,
    Div1,
    Div0s,
    Div0u,
    DmulsL,
    DmuluL,
    Dt,
    ExtsB,
    ExtsW,
    ExtuB,
    ExtuW,
    MacL,
    MacW,
    MulL,
    MulsW,
    MuluW,
    Neg,
    Negc,
    Sub,
    Subc,
    Subv,
    // Logic (table 2.14)
    And,
    AndImm,
    AndB,
    Not,
    Or,
    OrImm,
    OrB,
    TasB,
    Tst,
    TstImm,
    TstB,
    Xor,
    XorImm,
    XorB,
    // Shifts (table 2.15)
    Rotl,
    Rotr,
    Rotcl,
    Rotcr,
    Shal,
    Shar,
    Shll,
    Shlr,
    Shll2,
    Shlr2,
    Shll8,
    Shlr8,
    Shll16,
    Shlr16,
    // Branches (table 2.16)
    Bf,
    Bfs,
    Bt,
    Bts,
    Bra,
    Braf,
    Bsr,
    Bsrf,
    Jmp,
    Jsr,
    Rts,
    // System control (table 2.17)
    Clrt,
    Clrmac,
    LdcSr,
    LdcGbr,
    LdcVbr,
    LdcLSr,
    LdcLGbr,
    LdcLVbr,
    LdsMach,
    LdsMacl,
    LdsPr,
    LdsLMach,
    LdsLMacl,
    LdsLPr,
    Nop,
    Rte,
    Sett,
    Sleep,
    StcSr,
    StcGbr,
    StcVbr,
    StcLSr,
    StcLGbr,
    StcLVbr,
    StsMach,
    StsMacl,
    StsPr,
    StsLMach,
    StsLMacl,
    StsLPr,
    Trapa,
    // Floating point (table 2.18)
    Fabs,
    Fadd,
    FcmpEq,
    FcmpGt,
    Fdiv,
    Fldi0,
    Fldi1,
    Flds,
    Float,
    Fmac,
    Fmov,
    FmovLoadIdx,
    FmovLoadInc,
    FmovLoad,
    FmovStoreIdx,
    FmovStoreDec,
    FmovStore,
    Fmul,
    Fneg,
    Fsts,
    Fsub,
    Ftrc,
    // FPU-related CPU instructions (table 2.19)
    LdsFpscr,
    LdsFpul,
    LdsLFpscr,
    LdsLFpul,
    StsFpscr,
    StsFpul,
    StsLFpscr,
    StsLFpul,
}

impl Op {
    /// Instructions that rewrite the PC. In a delay slot they raise a slot illegal
    /// instruction exception (manual 6.5.3), as undefined codes do.
    pub const fn illegal_in_slot(self) -> bool {
        matches!(
            self,
            Op::Illegal
                | Op::Bf
                | Op::Bfs
                | Op::Bt
                | Op::Bts
                | Op::Bra
                | Op::Braf
                | Op::Bsr
                | Op::Bsrf
                | Op::Jmp
                | Op::Jsr
                | Op::Rts
                | Op::Rte
                | Op::Trapa
        )
    }

    /// Instructions after which an interrupt is not accepted (manual table 6.10, note 2,
    /// which lists LDC, LDC.L, STC, STC.L, LDS, LDS.L, STS and STS.L).
    pub const fn blocks_interrupt(self) -> bool {
        matches!(
            self,
            Op::LdcSr
                | Op::LdcGbr
                | Op::LdcVbr
                | Op::LdcLSr
                | Op::LdcLGbr
                | Op::LdcLVbr
                | Op::LdsMach
                | Op::LdsMacl
                | Op::LdsPr
                | Op::LdsLMach
                | Op::LdsLMacl
                | Op::LdsLPr
                | Op::StcSr
                | Op::StcGbr
                | Op::StcVbr
                | Op::StcLSr
                | Op::StcLGbr
                | Op::StcLVbr
                | Op::StsMach
                | Op::StsMacl
                | Op::StsPr
                | Op::StsLMach
                | Op::StsLMacl
                | Op::StsLPr
                | Op::LdsFpscr
                | Op::LdsFpul
                | Op::LdsLFpscr
                | Op::LdsLFpul
                | Op::StsFpscr
                | Op::StsFpul
                | Op::StsLFpscr
                | Op::StsLFpul
        )
    }
}

/// Decodes one instruction code.
pub const fn decode(code: u16) -> Op {
    let lo4 = code & 0x000F;
    let lo8 = code & 0x00FF;
    let mid = (code >> 8) & 0x000F;
    match code >> 12 {
        0x0 => match lo4 {
            0x4 => Op::MovBStoreIdx,
            0x5 => Op::MovWStoreIdx,
            0x6 => Op::MovLStoreIdx,
            0x7 => Op::MulL,
            0xC => Op::MovBLoadIdx,
            0xD => Op::MovWLoadIdx,
            0xE => Op::MovLLoadIdx,
            0xF => Op::MacL,
            _ => match lo8 {
                0x02 => Op::StcSr,
                0x12 => Op::StcGbr,
                0x22 => Op::StcVbr,
                0x03 => Op::Bsrf,
                0x23 => Op::Braf,
                0x29 => Op::Movt,
                0x0A => Op::StsMach,
                0x1A => Op::StsMacl,
                0x2A => Op::StsPr,
                0x5A => Op::StsFpul,
                0x6A => Op::StsFpscr,
                _ if mid != 0 => Op::Illegal,
                0x08 => Op::Clrt,
                0x09 => Op::Nop,
                0x0B => Op::Rts,
                0x18 => Op::Sett,
                0x19 => Op::Div0u,
                0x1B => Op::Sleep,
                0x28 => Op::Clrmac,
                0x2B => Op::Rte,
                _ => Op::Illegal,
            },
        },
        0x1 => Op::MovLStoreDisp,
        0x2 => match lo4 {
            0x0 => Op::MovBStore,
            0x1 => Op::MovWStore,
            0x2 => Op::MovLStore,
            0x4 => Op::MovBStoreDec,
            0x5 => Op::MovWStoreDec,
            0x6 => Op::MovLStoreDec,
            0x7 => Op::Div0s,
            0x8 => Op::Tst,
            0x9 => Op::And,
            0xA => Op::Xor,
            0xB => Op::Or,
            0xC => Op::CmpStr,
            0xD => Op::Xtrct,
            0xE => Op::MuluW,
            0xF => Op::MulsW,
            _ => Op::Illegal,
        },
        0x3 => match lo4 {
            0x0 => Op::CmpEq,
            0x2 => Op::CmpHs,
            0x3 => Op::CmpGe,
            0x4 => Op::Div1,
            0x5 => Op::DmuluL,
            0x6 => Op::CmpHi,
            0x7 => Op::CmpGt,
            0x8 => Op::Sub,
            0xA => Op::Subc,
            0xB => Op::Subv,
            0xC => Op::Add,
            0xD => Op::DmulsL,
            0xE => Op::Addc,
            0xF => Op::Addv,
            _ => Op::Illegal,
        },
        0x4 => match lo4 {
            0xF => Op::MacW,
            _ => match lo8 {
                0x00 => Op::Shll,
                0x01 => Op::Shlr,
                0x02 => Op::StsLMach,
                0x03 => Op::StcLSr,
                0x04 => Op::Rotl,
                0x05 => Op::Rotr,
                0x06 => Op::LdsLMach,
                0x07 => Op::LdcLSr,
                0x08 => Op::Shll2,
                0x09 => Op::Shlr2,
                0x0A => Op::LdsMach,
                0x0B => Op::Jsr,
                0x0E => Op::LdcSr,
                0x10 => Op::Dt,
                0x11 => Op::CmpPz,
                0x12 => Op::StsLMacl,
                0x13 => Op::StcLGbr,
                0x15 => Op::CmpPl,
                0x16 => Op::LdsLMacl,
                0x17 => Op::LdcLGbr,
                0x18 => Op::Shll8,
                0x19 => Op::Shlr8,
                0x1A => Op::LdsMacl,
                0x1B => Op::TasB,
                0x1E => Op::LdcGbr,
                0x20 => Op::Shal,
                0x21 => Op::Shar,
                0x22 => Op::StsLPr,
                0x23 => Op::StcLVbr,
                0x24 => Op::Rotcl,
                0x25 => Op::Rotcr,
                0x26 => Op::LdsLPr,
                0x27 => Op::LdcLVbr,
                0x28 => Op::Shll16,
                0x29 => Op::Shlr16,
                0x2A => Op::LdsPr,
                0x2B => Op::Jmp,
                0x2E => Op::LdcVbr,
                0x52 => Op::StsLFpul,
                0x56 => Op::LdsLFpul,
                0x5A => Op::LdsFpul,
                0x62 => Op::StsLFpscr,
                0x66 => Op::LdsLFpscr,
                0x6A => Op::LdsFpscr,
                _ => Op::Illegal,
            },
        },
        0x5 => Op::MovLLoadDisp,
        0x6 => match lo4 {
            0x0 => Op::MovBLoad,
            0x1 => Op::MovWLoad,
            0x2 => Op::MovLLoad,
            0x3 => Op::Mov,
            0x4 => Op::MovBLoadInc,
            0x5 => Op::MovWLoadInc,
            0x6 => Op::MovLLoadInc,
            0x7 => Op::Not,
            0x8 => Op::SwapB,
            0x9 => Op::SwapW,
            0xA => Op::Negc,
            0xB => Op::Neg,
            0xC => Op::ExtuB,
            0xD => Op::ExtuW,
            0xE => Op::ExtsB,
            _ => Op::ExtsW,
        },
        0x7 => Op::AddImm,
        0x8 => match mid {
            0x0 => Op::MovBStoreDisp,
            0x1 => Op::MovWStoreDisp,
            0x4 => Op::MovBLoadDisp,
            0x5 => Op::MovWLoadDisp,
            0x8 => Op::CmpEqImm,
            0x9 => Op::Bt,
            0xB => Op::Bf,
            0xD => Op::Bts,
            0xF => Op::Bfs,
            _ => Op::Illegal,
        },
        0x9 => Op::MovWPc,
        0xA => Op::Bra,
        0xB => Op::Bsr,
        0xC => match mid {
            0x0 => Op::MovBStoreGbr,
            0x1 => Op::MovWStoreGbr,
            0x2 => Op::MovLStoreGbr,
            0x3 => Op::Trapa,
            0x4 => Op::MovBLoadGbr,
            0x5 => Op::MovWLoadGbr,
            0x6 => Op::MovLLoadGbr,
            0x7 => Op::Mova,
            0x8 => Op::TstImm,
            0x9 => Op::AndImm,
            0xA => Op::XorImm,
            0xB => Op::OrImm,
            0xC => Op::TstB,
            0xD => Op::AndB,
            0xE => Op::XorB,
            _ => Op::OrB,
        },
        0xD => Op::MovLPc,
        0xE => Op::MovImm,
        _ => match lo4 {
            0x0 => Op::Fadd,
            0x1 => Op::Fsub,
            0x2 => Op::Fmul,
            0x3 => Op::Fdiv,
            0x4 => Op::FcmpEq,
            0x5 => Op::FcmpGt,
            0x6 => Op::FmovLoadIdx,
            0x7 => Op::FmovStoreIdx,
            0x8 => Op::FmovLoad,
            0x9 => Op::FmovLoadInc,
            0xA => Op::FmovStore,
            0xB => Op::FmovStoreDec,
            0xC => Op::Fmov,
            0xE => Op::Fmac,
            0xD => match (code >> 4) & 0xF {
                0x0 => Op::Fsts,
                0x1 => Op::Flds,
                0x2 => Op::Float,
                0x3 => Op::Ftrc,
                0x4 => Op::Fneg,
                0x5 => Op::Fabs,
                0x8 => Op::Fldi0,
                0x9 => Op::Fldi1,
                _ => Op::Illegal,
            },
            _ => Op::Illegal,
        },
    }
}

const fn build() -> [Op; 0x1_0000] {
    let mut t = [Op::Illegal; 0x1_0000];
    let mut i = 0;
    while i < 0x1_0000 {
        t[i] = decode(i as u16);
        i += 1;
    }
    t
}

/// The decode table: `DECODE[code]` is the instruction for that code.
pub static DECODE: [Op; 0x1_0000] = build();

/// Register field `nnnn`, bits 11..8.
#[inline(always)]
pub const fn n(code: u16) -> usize {
    ((code >> 8) & 0xF) as usize
}

/// Register field `mmmm`, bits 7..4.
#[inline(always)]
pub const fn m(code: u16) -> usize {
    ((code >> 4) & 0xF) as usize
}

/// The low 8 bits, sign-extended.
#[inline(always)]
pub const fn simm8(code: u16) -> u32 {
    code as u8 as i8 as i32 as u32
}

/// The low 8 bits, zero-extended.
#[inline(always)]
pub const fn imm8(code: u16) -> u32 {
    (code & 0xFF) as u32
}

/// The low 4 bits, zero-extended.
#[inline(always)]
pub const fn disp4(code: u16) -> u32 {
    (code & 0xF) as u32
}

/// The low 12 bits, sign-extended.
#[inline(always)]
pub const fn sdisp12(code: u16) -> u32 {
    (((code & 0x0FFF) << 4) as i16 >> 4) as i32 as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_extraction() {
        assert_eq!(n(0x6A3C), 0xA);
        assert_eq!(m(0x6A3C), 0x3);
        assert_eq!(simm8(0xE0FF), 0xFFFF_FFFF);
        assert_eq!(simm8(0xE07F), 0x7F);
        assert_eq!(imm8(0xC8FF), 0xFF);
        assert_eq!(sdisp12(0xAFFE), 0xFFFF_FFFE);
        assert_eq!(sdisp12(0xA7FF), 0x7FF);
        assert_eq!(sdisp12(0xA800), 0xFFFF_F800);
    }

    #[test]
    fn some_known_codes() {
        // From the phase 0 boot code and the manual's tables.
        assert_eq!(DECODE[0x0009], Op::Nop);
        assert_eq!(DECODE[0x000B], Op::Rts);
        assert_eq!(DECODE[0x002B], Op::Rte);
        assert_eq!(DECODE[0x010B], Op::Illegal, "RTS needs bits 11..8 clear");
        assert_eq!(DECODE[0x6241], Op::MovWLoad);
        assert_eq!(DECODE[0x622D], Op::ExtuW);
        assert_eq!(DECODE[0x2258], Op::Tst);
        assert_eq!(DECODE[0x89FB], Op::Bt);
        assert_eq!(DECODE[0xD547], Op::MovLPc);
        assert_eq!(DECODE[0xAFFE], Op::Bra);
        assert_eq!(DECODE[0x430B], Op::Jsr);
        assert_eq!(DECODE[0xF00D], Op::Fsts);
        assert_eq!(DECODE[0xF06D], Op::Illegal);
        assert_eq!(DECODE[0xFFFF], Op::Illegal);
        assert_eq!(DECODE[0x3001], Op::Illegal);
        assert_eq!(DECODE[0x8200], Op::Illegal);
    }

    #[test]
    fn the_table_holds_every_instruction_of_the_manual() {
        // Every variant of Op is the decoding of at least one code.
        let mut seen = std::collections::HashSet::new();
        for op in DECODE.iter() {
            seen.insert(*op as u8);
        }
        seen.remove(&(Op::Illegal as u8));
        assert_eq!(seen.len(), Op::StsLFpul as usize);
    }

    #[test]
    fn slot_and_interrupt_classes() {
        assert!(Op::Bra.illegal_in_slot());
        assert!(Op::Trapa.illegal_in_slot());
        assert!(Op::Illegal.illegal_in_slot());
        assert!(!Op::Mov.illegal_in_slot());
        assert!(Op::LdcSr.blocks_interrupt());
        assert!(Op::StsLFpul.blocks_interrupt());
        assert!(!Op::Add.blocks_interrupt());
    }
}
