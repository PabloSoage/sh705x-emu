//! Disassembly in the manual's syntax (REJ09B0177-0500, tables 2.12 to 2.19), with
//! numbers in hex and PC-relative operands and branch targets resolved to addresses.
//!
//! `pc` is the instruction's own address. For an instruction in a delay slot the CPU
//! takes "PC" as the branch target + 2 (see [`crate::Cpu`]); disassembly shows the
//! ordinary reading, address + 4.

use crate::decode::{DECODE, Op, disp4, imm8, m, n, sdisp12, simm8};

/// Where a PC-relative load reads its literal: (address, size in bytes). None for other
/// instructions.
pub fn literal(code: u16, pc: u32) -> Option<(u32, u32)> {
    let pc4 = pc.wrapping_add(4);
    match DECODE[code as usize] {
        Op::MovWPc => Some((pc4.wrapping_add(imm8(code) * 2), 2)),
        Op::MovLPc => Some(((pc4 & !3).wrapping_add(imm8(code) * 4), 4)),
        _ => None,
    }
}

/// The target of a branch with a displacement: BT, BF, BT/S, BF/S, BRA, BSR, and MOVA's
/// address. None for other instructions.
pub fn target(code: u16, pc: u32) -> Option<u32> {
    let pc4 = pc.wrapping_add(4);
    match DECODE[code as usize] {
        Op::Bt | Op::Bf | Op::Bts | Op::Bfs => Some(pc4.wrapping_add(simm8(code) << 1)),
        Op::Bra | Op::Bsr => Some(pc4.wrapping_add(sdisp12(code) << 1)),
        Op::Mova => Some((pc4 & !3).wrapping_add(imm8(code) * 4)),
        _ => None,
    }
}

/// One instruction as text, e.g. `MOV.L @(H'0001A4C0),R1` or `BT H'00002B10`.
pub fn disassemble(code: u16, pc: u32) -> String {
    let op = DECODE[code as usize];
    let (rn, rm) = (n(code), m(code));
    let i8s = simm8(code) as i32;
    let i8u = imm8(code);
    let d4 = disp4(code);
    let lit = literal(code, pc).map(|(a, _)| a).unwrap_or(0);
    let tgt = target(code, pc).unwrap_or(0);
    let imm = |v: i32| {
        if v < 0 {
            format!("#-H'{:02X}", -v)
        } else {
            format!("#H'{v:02X}")
        }
    };
    let s = |m: &str| m.to_string();
    match op {
        Op::Illegal => format!(".WORD H'{code:04X}"),
        Op::MovImm => format!("MOV {},R{rn}", imm(i8s)),
        Op::MovWPc => format!("MOV.W @(H'{lit:08X}),R{rn}"),
        Op::MovLPc => format!("MOV.L @(H'{lit:08X}),R{rn}"),
        Op::Mov => format!("MOV R{rm},R{rn}"),
        Op::MovBStore => format!("MOV.B R{rm},@R{rn}"),
        Op::MovWStore => format!("MOV.W R{rm},@R{rn}"),
        Op::MovLStore => format!("MOV.L R{rm},@R{rn}"),
        Op::MovBLoad => format!("MOV.B @R{rm},R{rn}"),
        Op::MovWLoad => format!("MOV.W @R{rm},R{rn}"),
        Op::MovLLoad => format!("MOV.L @R{rm},R{rn}"),
        Op::MovBStoreDec => format!("MOV.B R{rm},@-R{rn}"),
        Op::MovWStoreDec => format!("MOV.W R{rm},@-R{rn}"),
        Op::MovLStoreDec => format!("MOV.L R{rm},@-R{rn}"),
        Op::MovBLoadInc => format!("MOV.B @R{rm}+,R{rn}"),
        Op::MovWLoadInc => format!("MOV.W @R{rm}+,R{rn}"),
        Op::MovLLoadInc => format!("MOV.L @R{rm}+,R{rn}"),
        Op::MovBStoreDisp => format!("MOV.B R0,@(H'{d4:X},R{rm})"),
        Op::MovWStoreDisp => format!("MOV.W R0,@(H'{:X},R{rm})", d4 * 2),
        Op::MovLStoreDisp => format!("MOV.L R{rm},@(H'{:X},R{rn})", d4 * 4),
        Op::MovBLoadDisp => format!("MOV.B @(H'{d4:X},R{rm}),R0"),
        Op::MovWLoadDisp => format!("MOV.W @(H'{:X},R{rm}),R0", d4 * 2),
        Op::MovLLoadDisp => format!("MOV.L @(H'{:X},R{rm}),R{rn}", d4 * 4),
        Op::MovBStoreIdx => format!("MOV.B R{rm},@(R0,R{rn})"),
        Op::MovWStoreIdx => format!("MOV.W R{rm},@(R0,R{rn})"),
        Op::MovLStoreIdx => format!("MOV.L R{rm},@(R0,R{rn})"),
        Op::MovBLoadIdx => format!("MOV.B @(R0,R{rm}),R{rn}"),
        Op::MovWLoadIdx => format!("MOV.W @(R0,R{rm}),R{rn}"),
        Op::MovLLoadIdx => format!("MOV.L @(R0,R{rm}),R{rn}"),
        Op::MovBStoreGbr => format!("MOV.B R0,@(H'{i8u:X},GBR)"),
        Op::MovWStoreGbr => format!("MOV.W R0,@(H'{:X},GBR)", i8u * 2),
        Op::MovLStoreGbr => format!("MOV.L R0,@(H'{:X},GBR)", i8u * 4),
        Op::MovBLoadGbr => format!("MOV.B @(H'{i8u:X},GBR),R0"),
        Op::MovWLoadGbr => format!("MOV.W @(H'{:X},GBR),R0", i8u * 2),
        Op::MovLLoadGbr => format!("MOV.L @(H'{:X},GBR),R0", i8u * 4),
        Op::Mova => format!("MOVA @(H'{tgt:08X}),R0"),
        Op::Movt => format!("MOVT R{rn}"),
        Op::SwapB => format!("SWAP.B R{rm},R{rn}"),
        Op::SwapW => format!("SWAP.W R{rm},R{rn}"),
        Op::Xtrct => format!("XTRCT R{rm},R{rn}"),
        Op::Add => format!("ADD R{rm},R{rn}"),
        Op::AddImm => format!("ADD {},R{rn}", imm(i8s)),
        Op::Addc => format!("ADDC R{rm},R{rn}"),
        Op::Addv => format!("ADDV R{rm},R{rn}"),
        Op::CmpEqImm => format!("CMP/EQ {},R0", imm(i8s)),
        Op::CmpEq => format!("CMP/EQ R{rm},R{rn}"),
        Op::CmpHs => format!("CMP/HS R{rm},R{rn}"),
        Op::CmpGe => format!("CMP/GE R{rm},R{rn}"),
        Op::CmpHi => format!("CMP/HI R{rm},R{rn}"),
        Op::CmpGt => format!("CMP/GT R{rm},R{rn}"),
        Op::CmpPl => format!("CMP/PL R{rn}"),
        Op::CmpPz => format!("CMP/PZ R{rn}"),
        Op::CmpStr => format!("CMP/STR R{rm},R{rn}"),
        Op::Div1 => format!("DIV1 R{rm},R{rn}"),
        Op::Div0s => format!("DIV0S R{rm},R{rn}"),
        Op::Div0u => s("DIV0U"),
        Op::DmulsL => format!("DMULS.L R{rm},R{rn}"),
        Op::DmuluL => format!("DMULU.L R{rm},R{rn}"),
        Op::Dt => format!("DT R{rn}"),
        Op::ExtsB => format!("EXTS.B R{rm},R{rn}"),
        Op::ExtsW => format!("EXTS.W R{rm},R{rn}"),
        Op::ExtuB => format!("EXTU.B R{rm},R{rn}"),
        Op::ExtuW => format!("EXTU.W R{rm},R{rn}"),
        Op::MacL => format!("MAC.L @R{rm}+,@R{rn}+"),
        Op::MacW => format!("MAC.W @R{rm}+,@R{rn}+"),
        Op::MulL => format!("MUL.L R{rm},R{rn}"),
        Op::MulsW => format!("MULS.W R{rm},R{rn}"),
        Op::MuluW => format!("MULU.W R{rm},R{rn}"),
        Op::Neg => format!("NEG R{rm},R{rn}"),
        Op::Negc => format!("NEGC R{rm},R{rn}"),
        Op::Sub => format!("SUB R{rm},R{rn}"),
        Op::Subc => format!("SUBC R{rm},R{rn}"),
        Op::Subv => format!("SUBV R{rm},R{rn}"),
        Op::And => format!("AND R{rm},R{rn}"),
        Op::AndImm => format!("AND #H'{i8u:02X},R0"),
        Op::AndB => format!("AND.B #H'{i8u:02X},@(R0,GBR)"),
        Op::Not => format!("NOT R{rm},R{rn}"),
        Op::Or => format!("OR R{rm},R{rn}"),
        Op::OrImm => format!("OR #H'{i8u:02X},R0"),
        Op::OrB => format!("OR.B #H'{i8u:02X},@(R0,GBR)"),
        Op::TasB => format!("TAS.B @R{rn}"),
        Op::Tst => format!("TST R{rm},R{rn}"),
        Op::TstImm => format!("TST #H'{i8u:02X},R0"),
        Op::TstB => format!("TST.B #H'{i8u:02X},@(R0,GBR)"),
        Op::Xor => format!("XOR R{rm},R{rn}"),
        Op::XorImm => format!("XOR #H'{i8u:02X},R0"),
        Op::XorB => format!("XOR.B #H'{i8u:02X},@(R0,GBR)"),
        Op::Rotl => format!("ROTL R{rn}"),
        Op::Rotr => format!("ROTR R{rn}"),
        Op::Rotcl => format!("ROTCL R{rn}"),
        Op::Rotcr => format!("ROTCR R{rn}"),
        Op::Shal => format!("SHAL R{rn}"),
        Op::Shar => format!("SHAR R{rn}"),
        Op::Shll => format!("SHLL R{rn}"),
        Op::Shlr => format!("SHLR R{rn}"),
        Op::Shll2 => format!("SHLL2 R{rn}"),
        Op::Shlr2 => format!("SHLR2 R{rn}"),
        Op::Shll8 => format!("SHLL8 R{rn}"),
        Op::Shlr8 => format!("SHLR8 R{rn}"),
        Op::Shll16 => format!("SHLL16 R{rn}"),
        Op::Shlr16 => format!("SHLR16 R{rn}"),
        Op::Bf => format!("BF H'{tgt:08X}"),
        Op::Bfs => format!("BF/S H'{tgt:08X}"),
        Op::Bt => format!("BT H'{tgt:08X}"),
        Op::Bts => format!("BT/S H'{tgt:08X}"),
        Op::Bra => format!("BRA H'{tgt:08X}"),
        Op::Braf => format!("BRAF R{rn}"),
        Op::Bsr => format!("BSR H'{tgt:08X}"),
        Op::Bsrf => format!("BSRF R{rn}"),
        Op::Jmp => format!("JMP @R{rn}"),
        Op::Jsr => format!("JSR @R{rn}"),
        Op::Rts => s("RTS"),
        Op::Clrt => s("CLRT"),
        Op::Clrmac => s("CLRMAC"),
        Op::LdcSr => format!("LDC R{rn},SR"),
        Op::LdcGbr => format!("LDC R{rn},GBR"),
        Op::LdcVbr => format!("LDC R{rn},VBR"),
        Op::LdcLSr => format!("LDC.L @R{rn}+,SR"),
        Op::LdcLGbr => format!("LDC.L @R{rn}+,GBR"),
        Op::LdcLVbr => format!("LDC.L @R{rn}+,VBR"),
        Op::LdsMach => format!("LDS R{rn},MACH"),
        Op::LdsMacl => format!("LDS R{rn},MACL"),
        Op::LdsPr => format!("LDS R{rn},PR"),
        Op::LdsLMach => format!("LDS.L @R{rn}+,MACH"),
        Op::LdsLMacl => format!("LDS.L @R{rn}+,MACL"),
        Op::LdsLPr => format!("LDS.L @R{rn}+,PR"),
        Op::Nop => s("NOP"),
        Op::Rte => s("RTE"),
        Op::Sett => s("SETT"),
        Op::Sleep => s("SLEEP"),
        Op::StcSr => format!("STC SR,R{rn}"),
        Op::StcGbr => format!("STC GBR,R{rn}"),
        Op::StcVbr => format!("STC VBR,R{rn}"),
        Op::StcLSr => format!("STC.L SR,@-R{rn}"),
        Op::StcLGbr => format!("STC.L GBR,@-R{rn}"),
        Op::StcLVbr => format!("STC.L VBR,@-R{rn}"),
        Op::StsMach => format!("STS MACH,R{rn}"),
        Op::StsMacl => format!("STS MACL,R{rn}"),
        Op::StsPr => format!("STS PR,R{rn}"),
        Op::StsLMach => format!("STS.L MACH,@-R{rn}"),
        Op::StsLMacl => format!("STS.L MACL,@-R{rn}"),
        Op::StsLPr => format!("STS.L PR,@-R{rn}"),
        Op::Trapa => format!("TRAPA #H'{i8u:02X}"),
        Op::Fabs => format!("FABS FR{rn}"),
        Op::Fadd => format!("FADD FR{rm},FR{rn}"),
        Op::FcmpEq => format!("FCMP/EQ FR{rm},FR{rn}"),
        Op::FcmpGt => format!("FCMP/GT FR{rm},FR{rn}"),
        Op::Fdiv => format!("FDIV FR{rm},FR{rn}"),
        Op::Fldi0 => format!("FLDI0 FR{rn}"),
        Op::Fldi1 => format!("FLDI1 FR{rn}"),
        Op::Flds => format!("FLDS FR{rn},FPUL"),
        Op::Float => format!("FLOAT FPUL,FR{rn}"),
        Op::Fmac => format!("FMAC FR0,FR{rm},FR{rn}"),
        Op::Fmov => format!("FMOV FR{rm},FR{rn}"),
        Op::FmovLoadIdx => format!("FMOV.S @(R0,R{rm}),FR{rn}"),
        Op::FmovLoadInc => format!("FMOV.S @R{rm}+,FR{rn}"),
        Op::FmovLoad => format!("FMOV.S @R{rm},FR{rn}"),
        Op::FmovStoreIdx => format!("FMOV.S FR{rm},@(R0,R{rn})"),
        Op::FmovStoreDec => format!("FMOV.S FR{rm},@-R{rn}"),
        Op::FmovStore => format!("FMOV.S FR{rm},@R{rn}"),
        Op::Fmul => format!("FMUL FR{rm},FR{rn}"),
        Op::Fneg => format!("FNEG FR{rn}"),
        Op::Fsts => format!("FSTS FPUL,FR{rn}"),
        Op::Fsub => format!("FSUB FR{rm},FR{rn}"),
        Op::Ftrc => format!("FTRC FR{rn},FPUL"),
        Op::LdsFpscr => format!("LDS R{rn},FPSCR"),
        Op::LdsFpul => format!("LDS R{rn},FPUL"),
        Op::LdsLFpscr => format!("LDS.L @R{rn}+,FPSCR"),
        Op::LdsLFpul => format!("LDS.L @R{rn}+,FPUL"),
        Op::StsFpscr => format!("STS FPSCR,R{rn}"),
        Op::StsFpul => format!("STS FPUL,R{rn}"),
        Op::StsLFpscr => format!("STS.L FPSCR,@-R{rn}"),
        Op::StsLFpul => format!("STS.L FPUL,@-R{rn}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operands_from_the_manual_encodings() {
        // Encodings from tables 2.12 to 2.19.
        let cases: &[(u16, u32, &str)] = &[
            (0xE3FE, 0, "MOV #-H'02,R3"),
            (0xD105, 0x1002, "MOV.L @(H'00001018),R1"), // (0x1006 & !3) + 20
            (0x9203, 0x1000, "MOV.W @(H'0000100A),R2"),
            (0x85A3, 0, "MOV.W @(H'6,R10),R0"),
            (0x1A23, 0, "MOV.L R2,@(H'C,R10)"),
            (0x5A23, 0, "MOV.L @(H'C,R2),R10"),
            (0xC7FF, 0x2000, "MOVA @(H'00002400),R0"),
            (0x89FE, 0x2000, "BT H'00002000"),
            (0x8F02, 0x2000, "BF/S H'00002008"),
            (0xAFFE, 0x2000, "BRA H'00002000"),
            (0xB800, 0x2000, "BSR H'00001004"),
            (0x6A3C, 0, "EXTU.B R3,R10"),
            (0x4A0B, 0, "JSR @R10"),
            (0xF12E, 0, "FMAC FR0,FR2,FR1"),
            (0xF13D, 0, "FTRC FR1,FPUL"),
            (0xF10D, 0, "FSTS FPUL,FR1"),
            (0x4F22, 0, "STS.L PR,@-R15"),
            (0xC910, 0, "AND #H'10,R0"),
            (0xCD80, 0, "AND.B #H'80,@(R0,GBR)"),
            (0x000B, 0, "RTS"),
            (0xFFFF, 0, ".WORD H'FFFF"),
        ];
        for &(code, pc, want) in cases {
            assert_eq!(disassemble(code, pc), want, "code {code:04X}");
        }
        assert_eq!(literal(0xD105, 0x1002), Some((0x1018, 4)));
        assert_eq!(literal(0x6A3C, 0), None);
        assert_eq!(target(0x8F02, 0x2000), Some(0x2008));
    }

    #[test]
    fn every_code_disassembles() {
        for code in 0..=0xFFFFu16 {
            let t = disassemble(code, 0x1000);
            assert!(!t.is_empty());
            assert_eq!(
                t.starts_with(".WORD"),
                DECODE[code as usize] == Op::Illegal,
                "{code:04X}"
            );
        }
    }
}
