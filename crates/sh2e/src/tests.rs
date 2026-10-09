//! Instruction tests. Each instruction is encoded from the bit pattern printed in manual
//! tables 2.12 to 2.19 (`enc`), not from the decoder, so a mistake in one does not hide a
//! mistake in the other.

use crate::testing::{CODE, Flat, STACK, handler, machine};
use crate::{Bus, Cpu, SR_M, SR_Q, SR_S, SR_T, vector};

/// Encodes a manual pattern such as `"0011nnnnmmmm1100"`. Fields: n, m, i (immediate),
/// d (displacement); each is filled from its value, most significant bit first.
fn enc(pattern: &str, fields: &[(char, u32)]) -> u16 {
    let p: Vec<char> = pattern.chars().filter(|c| !c.is_whitespace()).collect();
    assert_eq!(p.len(), 16, "pattern {pattern}");
    let mut code = 0u16;
    for (bit, c) in p.iter().enumerate() {
        let v = match c {
            '0' => 0,
            '1' => 1,
            f => {
                let (_, val) = fields
                    .iter()
                    .find(|(k, _)| k == f)
                    .unwrap_or_else(|| panic!("field {f} missing for {pattern}"));
                let width = p.iter().filter(|x| *x == f).count();
                let pos_in_field = p[..bit].iter().filter(|x| *x == f).count();
                (val >> (width - 1 - pos_in_field)) & 1
            }
        };
        code = (code << 1) | v as u16;
    }
    code
}

fn nm(pattern: &str, n: u32, m: u32) -> u16 {
    enc(pattern, &[('n', n), ('m', m)])
}

const NOP: u16 = 0x0009;

/// Runs `code` from CODE until `steps` steps have executed.
fn run(code: &[u16], steps: usize, setup: impl FnOnce(&mut Cpu, &mut Flat)) -> (Cpu, Flat) {
    let (mut cpu, mut mem) = machine(code);
    setup(&mut cpu, &mut mem);
    for _ in 0..steps {
        cpu.step(&mut mem);
    }
    (cpu, mem)
}

fn one(code: u16, setup: impl FnOnce(&mut Cpu, &mut Flat)) -> (Cpu, Flat) {
    run(&[code, NOP], 1, setup)
}

// ---------------------------------------------------------------- data transfer

#[test]
fn mov_immediate_is_sign_extended() {
    let (c, _) = one(enc("1110nnnniiiiiiii", &[('n', 3), ('i', 0x80)]), |_, _| {});
    assert_eq!(c.r[3], 0xFFFF_FF80);
    let (c, _) = one(enc("1110nnnniiiiiiii", &[('n', 3), ('i', 0x7F)]), |_, _| {});
    assert_eq!(c.r[3], 0x7F);
}

#[test]
fn mov_pc_relative() {
    // MOV.W @(disp,PC): PC + 4 + disp*2, sign-extended.
    let code = [
        enc("1001nnnndddddddd", &[('n', 1), ('d', 1)]),
        NOP,
        NOP,
        0x8001,
    ];
    let (c, _) = run(&code, 1, |_, _| {});
    assert_eq!(c.r[1], 0xFFFF_8001);
    // MOV.L @(disp,PC) from an address that is 2 mod 4: (PC & ~3) + 4 + disp*4.
    let code = [
        NOP,
        enc("1101nnnndddddddd", &[('n', 2), ('d', 0)]),
        NOP,
        NOP,
    ];
    let (c, _) = run(&code, 2, |_, m| m.write32(CODE + 4, 0x1234_5678));
    assert_eq!(c.r[2], 0x1234_5678);
    // MOVA: same address computation, into R0.
    let code = [NOP, enc("11000111dddddddd", &[('d', 3)])];
    let (c, _) = run(&code, 2, |_, _| {});
    assert_eq!(c.r[0], CODE + 4 + 12);
}

#[test]
fn mov_register_and_indirect() {
    let (c, _) = one(nm("0110nnnnmmmm0011", 4, 5), |c, _| c.r[5] = 0xABCD);
    assert_eq!(c.r[4], 0xABCD);
    let (_, mut m) = one(nm("0010nnnnmmmm0000", 1, 2), |c, _| {
        c.r[1] = 0x2000;
        c.r[2] = 0x1234_56F0;
    });
    assert_eq!(m.read8(0x2000), 0xF0);
    let (_, mut m) = one(nm("0010nnnnmmmm0001", 1, 2), |c, _| {
        c.r[1] = 0x2000;
        c.r[2] = 0x1234_56F0;
    });
    assert_eq!(m.read16(0x2000), 0x56F0);
    let (_, mut m) = one(nm("0010nnnnmmmm0010", 1, 2), |c, _| {
        c.r[1] = 0x2000;
        c.r[2] = 0x1234_56F0;
    });
    assert_eq!(m.read32(0x2000), 0x1234_56F0);
    for (p, v) in [
        ("0110nnnnmmmm0000", 0xFFFF_FF80u32),
        ("0110nnnnmmmm0001", 0xFFFF_8081),
        ("0110nnnnmmmm0010", 0x8081_8283),
    ] {
        let (c, _) = one(nm(p, 1, 2), |c, m| {
            c.r[2] = 0x2000;
            m.write32(0x2000, 0x8081_8283);
        });
        assert_eq!(c.r[1], v, "{p}");
    }
}

#[test]
fn mov_predecrement_and_postincrement() {
    for (p, size) in [
        ("0010nnnnmmmm0100", 1u32),
        ("0010nnnnmmmm0101", 2),
        ("0010nnnnmmmm0110", 4),
    ] {
        let (c, mut m) = one(nm(p, 1, 2), |c, _| {
            c.r[1] = 0x2008;
            c.r[2] = 0xA1B2_C3D4;
        });
        assert_eq!(c.r[1], 0x2008 - size, "{p}");
        let got = match size {
            1 => m.read8(c.r[1]) as u32,
            2 => m.read16(c.r[1]) as u32,
            _ => m.read32(c.r[1]),
        };
        assert_eq!(got, 0xA1B2_C3D4 & (u32::MAX >> (32 - 8 * size)), "{p}");
    }
    for (p, size, v) in [
        ("0110nnnnmmmm0100", 1u32, 0xFFFF_FFA1u32),
        ("0110nnnnmmmm0101", 2, 0xFFFF_A1B2),
        ("0110nnnnmmmm0110", 4, 0xA1B2_C3D4),
    ] {
        let (c, _) = one(nm(p, 1, 2), |c, m| {
            c.r[2] = 0x2000;
            m.write32(0x2000, 0xA1B2_C3D4);
        });
        assert_eq!((c.r[1], c.r[2]), (v, 0x2000 + size), "{p}");
        // With n = m the loaded value wins: no increment.
        let (c, _) = one(nm(p, 2, 2), |c, m| {
            c.r[2] = 0x2000;
            m.write32(0x2000, 0xA1B2_C3D4);
        });
        assert_eq!(c.r[2], v, "{p} n=m");
    }
}

#[test]
fn mov_displacement_forms() {
    // R0 -> @(disp,Rn) byte and word: register in bits 7..4.
    let (_, mut m) = one(enc("10000000nnnndddd", &[('n', 3), ('d', 5)]), |c, _| {
        c.r[3] = 0x2000;
        c.r[0] = 0x77;
    });
    assert_eq!(m.read8(0x2005), 0x77);
    let (_, mut m) = one(enc("10000001nnnndddd", &[('n', 3), ('d', 5)]), |c, _| {
        c.r[3] = 0x2000;
        c.r[0] = 0x7788;
    });
    assert_eq!(m.read16(0x200A), 0x7788);
    let (_, mut m) = one(
        enc("0001nnnnmmmmdddd", &[('n', 3), ('m', 4), ('d', 15)]),
        |c, _| {
            c.r[3] = 0x2000;
            c.r[4] = 0xCAFE_BABE;
        },
    );
    assert_eq!(m.read32(0x203C), 0xCAFE_BABE);
    let setup = |c: &mut Cpu, m: &mut Flat| {
        c.r[3] = 0x2000;
        m.write32(0x2004, 0x8899_AABB);
    };
    let (c, _) = one(enc("10000100mmmmdddd", &[('m', 3), ('d', 4)]), setup);
    assert_eq!(c.r[0], 0xFFFF_FF88);
    let (c, _) = one(enc("10000101mmmmdddd", &[('m', 3), ('d', 2)]), setup);
    assert_eq!(c.r[0], 0xFFFF_8899);
    let (c, _) = one(
        enc("0101nnnnmmmmdddd", &[('n', 7), ('m', 3), ('d', 1)]),
        setup,
    );
    assert_eq!(c.r[7], 0x8899_AABB);
}

#[test]
fn mov_indexed_forms() {
    let setup = |c: &mut Cpu, _: &mut Flat| {
        c.r[0] = 0x10;
        c.r[1] = 0x2000;
        c.r[2] = 0x8182_8384;
    };
    for (p, size) in [
        ("0000nnnnmmmm0100", 1u32),
        ("0000nnnnmmmm0101", 2),
        ("0000nnnnmmmm0110", 4),
    ] {
        let (_, mut m) = one(nm(p, 1, 2), setup);
        assert_eq!(
            m.read32(0x2010) >> (32 - 8 * size),
            0x8182_8384 & (u32::MAX >> (32 - 8 * size))
        );
    }
    for (p, v) in [
        ("0000nnnnmmmm1100", 0xFFFF_FF81u32),
        ("0000nnnnmmmm1101", 0xFFFF_8182),
        ("0000nnnnmmmm1110", 0x8182_8384),
    ] {
        let (c, _) = one(nm(p, 3, 1), |c, m| {
            c.r[0] = 0x10;
            c.r[1] = 0x2000;
            m.write32(0x2010, 0x8182_8384);
        });
        assert_eq!(c.r[3], v, "{p}");
    }
}

#[test]
fn mov_gbr_forms() {
    let setup = |c: &mut Cpu, m: &mut Flat| {
        c.gbr = 0x2000;
        c.r[0] = 0x8182_8384;
        m.write32(0x2008, 0xF1F2_F3F4);
    };
    let (_, mut m) = one(enc("11000000dddddddd", &[('d', 3)]), setup);
    assert_eq!(m.read8(0x2003), 0x84);
    let (_, mut m) = one(enc("11000001dddddddd", &[('d', 3)]), setup);
    assert_eq!(m.read16(0x2006), 0x8384);
    let (_, mut m) = one(enc("11000010dddddddd", &[('d', 3)]), setup);
    assert_eq!(m.read32(0x200C), 0x8182_8384);
    let (c, _) = one(enc("11000100dddddddd", &[('d', 8)]), setup);
    assert_eq!(c.r[0], 0xFFFF_FFF1);
    let (c, _) = one(enc("11000101dddddddd", &[('d', 5)]), setup);
    assert_eq!(c.r[0], 0xFFFF_F3F4);
    let (c, _) = one(enc("11000110dddddddd", &[('d', 2)]), setup);
    assert_eq!(c.r[0], 0xF1F2_F3F4);
}

#[test]
fn movt_swap_xtrct() {
    let (c, _) = one(enc("0000nnnn00101001", &[('n', 2)]), |c, _| c.sr |= SR_T);
    assert_eq!(c.r[2], 1);
    let (c, _) = one(nm("0110nnnnmmmm1000", 1, 2), |c, _| c.r[2] = 0x1122_3344);
    assert_eq!(c.r[1], 0x1122_4433);
    let (c, _) = one(nm("0110nnnnmmmm1001", 1, 2), |c, _| c.r[2] = 0x1122_3344);
    assert_eq!(c.r[1], 0x3344_1122);
    let (c, _) = one(nm("0010nnnnmmmm1101", 1, 2), |c, _| {
        c.r[1] = 0x1122_3344;
        c.r[2] = 0x5566_7788;
    });
    assert_eq!(c.r[1], 0x7788_1122);
}

// ---------------------------------------------------------------- arithmetic

/// (pattern, Rn, Rm, T in) -> (Rn, T out)
fn alu(p: &str, rn: u32, rm: u32, t: bool) -> (u32, bool) {
    let (c, _) = one(nm(p, 1, 2), |c, _| {
        c.r[1] = rn;
        c.r[2] = rm;
        c.sr = (c.sr & !SR_T) | t as u32;
    });
    (c.r[1], c.t() != 0)
}

#[test]
fn add_family() {
    assert_eq!(alu("0011nnnnmmmm1100", 0xFFFF_FFFF, 2, false).0, 1);
    let (c, _) = one(enc("0111nnnniiiiiiii", &[('n', 1), ('i', 0xFE)]), |c, _| {
        c.r[1] = 5
    });
    assert_eq!(c.r[1], 3);
    assert_eq!(alu("0011nnnnmmmm1110", 0xFFFF_FFFF, 0, true), (0, true));
    assert_eq!(alu("0011nnnnmmmm1110", 1, 2, true), (4, false));
    assert_eq!(
        alu("0011nnnnmmmm1111", 0x7FFF_FFFF, 1, false),
        (0x8000_0000, true)
    );
    assert_eq!(alu("0011nnnnmmmm1111", 0xFFFF_FFFF, 1, true), (0, false));
}

#[test]
fn sub_family() {
    assert_eq!(alu("0011nnnnmmmm1000", 0, 1, false).0, 0xFFFF_FFFF);
    assert_eq!(alu("0011nnnnmmmm1010", 0, 0, true), (0xFFFF_FFFF, true));
    assert_eq!(alu("0011nnnnmmmm1010", 5, 3, true), (1, false));
    assert_eq!(
        alu("0011nnnnmmmm1011", 0x8000_0000, 1, false),
        (0x7FFF_FFFF, true)
    );
    assert_eq!(alu("0011nnnnmmmm1011", 0, 1, true), (0xFFFF_FFFF, false));
    assert_eq!(alu("0110nnnnmmmm1011", 0, 5, false).0, 0xFFFF_FFFB);
    assert_eq!(alu("0110nnnnmmmm1010", 0, 0, false), (0, false));
    assert_eq!(alu("0110nnnnmmmm1010", 0, 0, true), (0xFFFF_FFFF, true));
    assert_eq!(alu("0110nnnnmmmm1010", 0, 1, false), (0xFFFF_FFFF, true));
}

#[test]
fn sixty_four_bit_arithmetic_with_carry() {
    // ADDC/SUBC/NEGC chains, as the manual's 64-bit idioms use them, against Rust.
    let pairs = [
        (0x0000_0001_FFFF_FFFFu64, 0x0000_0000_0000_0001u64),
        (0xFFFF_FFFF_FFFF_FFFF, 0xFFFF_FFFF_FFFF_FFFF),
        (0x8000_0000_0000_0000, 0x7FFF_FFFF_FFFF_FFFF),
        (0x1234_5678_9ABC_DEF0, 0x0FED_CBA9_8765_4321),
    ];
    let clrt = enc("0000000000001000", &[]);
    for (a, b) in pairs {
        // R1:R0 + R3:R2 (high:low)
        let code = [
            clrt,
            nm("0011nnnnmmmm1110", 0, 2),
            nm("0011nnnnmmmm1110", 1, 3),
        ];
        let (c, _) = run(&code, 3, |c, _| {
            c.r[1] = (a >> 32) as u32;
            c.r[0] = a as u32;
            c.r[3] = (b >> 32) as u32;
            c.r[2] = b as u32;
        });
        assert_eq!(((c.r[1] as u64) << 32) | c.r[0] as u64, a.wrapping_add(b));
        let code = [
            clrt,
            nm("0011nnnnmmmm1010", 0, 2),
            nm("0011nnnnmmmm1010", 1, 3),
        ];
        let (c, _) = run(&code, 3, |c, _| {
            c.r[1] = (a >> 32) as u32;
            c.r[0] = a as u32;
            c.r[3] = (b >> 32) as u32;
            c.r[2] = b as u32;
        });
        assert_eq!(((c.r[1] as u64) << 32) | c.r[0] as u64, a.wrapping_sub(b));
        let code = [
            clrt,
            nm("0110nnnnmmmm1010", 0, 0),
            nm("0110nnnnmmmm1010", 1, 1),
        ];
        let (c, _) = run(&code, 3, |c, _| {
            c.r[1] = (a >> 32) as u32;
            c.r[0] = a as u32;
        });
        assert_eq!(((c.r[1] as u64) << 32) | c.r[0] as u64, a.wrapping_neg());
    }
}

#[test]
fn compares() {
    let t = |p: &str, a: u32, b: u32| alu(p, a, b, false).1;
    assert!(t("0011nnnnmmmm0000", 7, 7));
    assert!(!t("0011nnnnmmmm0000", 7, 8));
    assert!(t("0011nnnnmmmm0010", 0xFFFF_FFFF, 1), "HS is unsigned");
    assert!(t("0011nnnnmmmm0010", 1, 1));
    assert!(!t("0011nnnnmmmm0011", 0xFFFF_FFFF, 1), "GE is signed");
    assert!(t("0011nnnnmmmm0011", 1, 1));
    assert!(t("0011nnnnmmmm0110", 0xFFFF_FFFF, 1), "HI is unsigned");
    assert!(!t("0011nnnnmmmm0110", 1, 1));
    assert!(!t("0011nnnnmmmm0111", 0xFFFF_FFFF, 1), "GT is signed");
    assert!(t("0011nnnnmmmm0111", 2, 1));
    assert!(
        t("0010nnnnmmmm1100", 0x1122_3344, 0xAA22_BBCC),
        "STR: byte 2 equal"
    );
    assert!(!t("0010nnnnmmmm1100", 0x1122_3344, 0x2233_4411));
    let t1 = |p: &str, a: u32| alu(p, a, 0, false).1;
    assert!(!t1("0100nnnn00010101", 0), "PL: > 0");
    assert!(t1("0100nnnn00010101", 1));
    assert!(t1("0100nnnn00010001", 0), "PZ: >= 0");
    assert!(!t1("0100nnnn00010001", 0x8000_0000));
    let (c, _) = one(enc("10001000iiiiiiii", &[('i', 0xFF)]), |c, _| {
        c.r[0] = 0xFFFF_FFFF
    });
    assert_eq!(c.t(), 1, "CMP/EQ #imm sign-extends");
}

#[test]
fn division_by_div1_matches_the_host() {
    // Unsigned 64 / 32: R1:R2 / R0 -> R2, the 32-step DIV1 idiom.
    let div0u = enc("0000000000011001", &[]);
    let rotcl2 = enc("0100nnnn00100100", &[('n', 2)]);
    let div1 = nm("0011nnnnmmmm0100", 1, 0);
    let mut code = vec![div0u];
    for _ in 0..32 {
        code.push(rotcl2);
        code.push(div1);
    }
    code.push(rotcl2);
    let mut seed = 0x1234_5678_9ABC_DEF1u64;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for i in 0..300 {
        let divisor = match i {
            0 => 1,
            1 => 0xFFFF_FFFF,
            _ => (rnd() as u32).max(1),
        };
        let dividend = rnd() % ((divisor as u64) << 32);
        let (c, _) = run(&code, code.len(), |c, _| {
            c.r[0] = divisor;
            c.r[1] = (dividend >> 32) as u32;
            c.r[2] = dividend as u32;
        });
        assert_eq!(
            c.r[2] as u64,
            dividend / divisor as u64,
            "{dividend:#x} / {divisor:#x}"
        );
    }
}

/// DIV1 as the SH programming manuals write it: nested cases on the old Q and on M.
/// Structured differently from the emulator's version, which folds the cases.
fn div1_reference(rn: u32, rm: u32, q: bool, m: bool, t: bool) -> (u32, bool, bool) {
    let old_q = q;
    let mut q = rn & 0x8000_0000 != 0;
    let tmp2 = rm;
    let mut rn = (rn << 1) | t as u32;
    let tmp0 = rn;
    let tmp1;
    match (old_q, m) {
        (false, false) => {
            rn = rn.wrapping_sub(tmp2);
            tmp1 = rn > tmp0;
            q = if q { !tmp1 } else { tmp1 };
        }
        (false, true) => {
            rn = rn.wrapping_add(tmp2);
            tmp1 = rn < tmp0;
            q = if q { tmp1 } else { !tmp1 };
        }
        (true, false) => {
            rn = rn.wrapping_add(tmp2);
            tmp1 = rn < tmp0;
            q = if q { !tmp1 } else { tmp1 };
        }
        (true, true) => {
            rn = rn.wrapping_sub(tmp2);
            tmp1 = rn > tmp0;
            q = if q { tmp1 } else { !tmp1 };
        }
    }
    (rn, q, q == m)
}

#[test]
fn div1_matches_the_reference_in_every_q_m_t_state() {
    let div1 = nm("0011nnnnmmmm0100", 1, 2);
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    for i in 0..200_000u32 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let (rn, rm) = (seed as u32, (seed >> 32) as u32);
        let (q, m, t) = (i & 1 != 0, i & 2 != 0, i & 4 != 0);
        let (c, _) = one(div1, |c, _| {
            c.r[1] = rn;
            c.r[2] = rm;
            c.sr = (q as u32) << 8 | (m as u32) << 9 | t as u32;
        });
        let want = div1_reference(rn, rm, q, m, t);
        let got = (c.r[1], c.sr & SR_Q != 0, c.t() != 0);
        assert_eq!(got, want, "Rn {rn:#x} Rm {rm:#x} Q {q} M {m} T {t}");
        assert_eq!(c.sr & SR_M != 0, m, "M unchanged");
    }
}

#[test]
fn div0s_sets_q_m_t() {
    let (c, _) = one(nm("0010nnnnmmmm0111", 1, 2), |c, _| {
        c.r[1] = 0x8000_0000;
        c.r[2] = 1;
    });
    assert_eq!(
        (c.sr & SR_Q != 0, c.sr & SR_M != 0, c.t()),
        (true, false, 1)
    );
    let (c, _) = one(enc("0000000000011001", &[]), |c, _| {
        c.sr |= SR_Q | SR_M | SR_T
    });
    assert_eq!(c.sr & (SR_Q | SR_M | SR_T), 0);
}

#[test]
fn multiplies() {
    let run2 = |p: &str, a: u32, b: u32| {
        let (c, _) = one(nm(p, 1, 2), |c, _| {
            c.r[1] = a;
            c.r[2] = b;
            c.mach = 0x5555_5555;
        });
        (c.mach, c.macl)
    };
    assert_eq!(
        run2("0011nnnnmmmm1101", 0xFFFF_FFFF, 2),
        (0xFFFF_FFFF, 0xFFFF_FFFE)
    );
    assert_eq!(run2("0011nnnnmmmm0101", 0xFFFF_FFFF, 2), (1, 0xFFFF_FFFE));
    assert_eq!(
        run2("0000nnnnmmmm0111", 0x1_0001, 0x1_0001),
        (0x5555_5555, 0x0002_0001)
    );
    assert_eq!(
        run2("0010nnnnmmmm1111", 0x0001_FFFF, 2).1,
        0xFFFF_FFFE,
        "MULS.W"
    );
    assert_eq!(
        run2("0010nnnnmmmm1110", 0x0001_FFFF, 2).1,
        0x0001_FFFE,
        "MULU.W"
    );
}

#[test]
fn multiply_and_accumulate() {
    // MAC.L @Rm+,@Rn+ : (-2) * 3 + 10 = 4, 64-bit.
    let (c, _) = one(nm("0000nnnnmmmm1111", 1, 2), |c, m| {
        c.r[1] = 0x2000;
        c.r[2] = 0x2010;
        m.write32(0x2000, (-2i32) as u32);
        m.write32(0x2010, 3);
        c.macl = 10;
    });
    assert_eq!((c.mach, c.macl, c.r[1], c.r[2]), (0, 4, 0x2004, 0x2014));
    // MAC.W: 16 x 16 signed.
    let (c, _) = one(nm("0100nnnnmmmm1111", 1, 2), |c, m| {
        c.r[1] = 0x2000;
        c.r[2] = 0x2010;
        m.write16(0x2000, (-2i16) as u16);
        m.write16(0x2010, 3);
    });
    assert_eq!(
        (c.mach, c.macl, c.r[1], c.r[2]),
        (0xFFFF_FFFF, 0xFFFF_FFFA, 0x2002, 0x2012)
    );
    assert_eq!(c.unverified.mac_saturation, 0);
    // With S = 1, MAC.W saturates MACL and is counted as unverified.
    let (c, _) = one(nm("0100nnnnmmmm1111", 1, 2), |c, m| {
        c.r[1] = 0x2000;
        c.r[2] = 0x2010;
        m.write16(0x2000, 0x7FFF);
        m.write16(0x2010, 0x7FFF);
        c.macl = 0x7FFF_0000;
        c.sr |= SR_S;
    });
    assert_eq!(c.macl, 0x7FFF_FFFF);
    assert_eq!(c.unverified.mac_saturation, 1);
}

#[test]
fn dt_and_extensions() {
    assert_eq!(alu("0100nnnn00010000", 1, 0, false), (0, true));
    assert_eq!(alu("0100nnnn00010000", 0, 0, true), (0xFFFF_FFFF, false));
    assert_eq!(alu("0110nnnnmmmm1110", 0, 0x80, false).0, 0xFFFF_FF80);
    assert_eq!(alu("0110nnnnmmmm1111", 0, 0x8000, false).0, 0xFFFF_8000);
    assert_eq!(alu("0110nnnnmmmm1100", 0, 0xFFFF_FF80, false).0, 0x80);
    assert_eq!(alu("0110nnnnmmmm1101", 0, 0xFFFF_8000, false).0, 0x8000);
}

// ---------------------------------------------------------------- logic and shifts

#[test]
fn logic() {
    assert_eq!(alu("0010nnnnmmmm1001", 0xF0F0, 0xFF00, false).0, 0xF000);
    assert_eq!(alu("0010nnnnmmmm1011", 0xF0F0, 0xFF00, false).0, 0xFFF0);
    assert_eq!(alu("0010nnnnmmmm1010", 0xF0F0, 0xFF00, false).0, 0x0FF0);
    assert_eq!(alu("0110nnnnmmmm0111", 0, 0xF0F0, false).0, 0xFFFF_0F0F);
    assert!(alu("0010nnnnmmmm1000", 0xF0, 0x0F, false).1);
    assert!(!alu("0010nnnnmmmm1000", 0xF0, 0x1F, false).1);
    // Immediates are zero-extended.
    for (p, want) in [
        ("11001001iiiiiiii", 0x80u32),
        ("11001011iiiiiiii", 0xFFFF_FFFF),
        ("11001010iiiiiiii", 0xFFFF_FF7F),
    ] {
        let (c, _) = one(enc(p, &[('i', 0x80)]), |c, _| c.r[0] = 0xFFFF_FFFF);
        assert_eq!(c.r[0], want, "{p}");
    }
    let (c, _) = one(enc("11001000iiiiiiii", &[('i', 0x80)]), |c, _| {
        c.r[0] = 0xFFFF_FF7F
    });
    assert_eq!(c.t(), 1);
}

#[test]
fn logic_on_memory_through_gbr() {
    let setup = |c: &mut Cpu, m: &mut Flat| {
        c.gbr = 0x2000;
        c.r[0] = 4;
        m.write8(0x2004, 0b1100_1010);
    };
    for (p, want) in [
        ("11001101iiiiiiii", 0b1000_1000u8),
        ("11001111iiiiiiii", 0b1110_1110),
        ("11001110iiiiiiii", 0b0110_0110),
    ] {
        let (_, mut m) = one(enc(p, &[('i', 0b1010_1100)]), setup);
        assert_eq!(m.read8(0x2004), want, "{p}");
    }
    let (c, _) = one(enc("11001100iiiiiiii", &[('i', 0b0011_0101)]), setup);
    assert_eq!(c.t(), 1);
    let (c, mut m) = one(enc("0100nnnn00011011", &[('n', 1)]), |c, _| c.r[1] = 0x2000);
    assert_eq!((c.t(), m.read8(0x2000)), (1, 0x80));
}

#[test]
fn shifts_and_rotates() {
    let s = |p: &str, v: u32, t: bool| alu(p, v, 0, t);
    assert_eq!(
        s("0100nnnn00000100", 0x8000_0001, false),
        (0x0000_0003, true)
    );
    assert_eq!(
        s("0100nnnn00000101", 0x8000_0001, false),
        (0xC000_0000, true)
    );
    assert_eq!(
        s("0100nnnn00100100", 0x8000_0000, true),
        (0x0000_0001, true)
    );
    assert_eq!(
        s("0100nnnn00100101", 0x0000_0001, true),
        (0x8000_0000, true)
    );
    assert_eq!(
        s("0100nnnn00100000", 0xC000_0000, false),
        (0x8000_0000, true)
    );
    assert_eq!(
        s("0100nnnn00100001", 0x8000_0001, false),
        (0xC000_0000, true)
    );
    assert_eq!(
        s("0100nnnn00000000", 0x8000_0001, false),
        (0x0000_0002, true)
    );
    assert_eq!(
        s("0100nnnn00000001", 0x8000_0001, false),
        (0x4000_0000, true)
    );
    assert_eq!(
        s("0100nnnn00001000", 0xC000_0001, true),
        (0x0000_0004, true)
    );
    assert_eq!(
        s("0100nnnn00001001", 0x8000_0003, false),
        (0x2000_0000, false)
    );
    assert_eq!(s("0100nnnn00011000", 0x1234_5678, false).0, 0x3456_7800);
    assert_eq!(s("0100nnnn00011001", 0x1234_5678, false).0, 0x0012_3456);
    assert_eq!(s("0100nnnn00101000", 0x1234_5678, false).0, 0x5678_0000);
    assert_eq!(s("0100nnnn00101001", 0x1234_5678, false).0, 0x0000_1234);
}

// ---------------------------------------------------------------- branches

#[test]
fn conditional_branches() {
    // BT taken jumps to PC + 4 + disp*2, no delay slot.
    let bt = enc("10001001dddddddd", &[('d', 2)]);
    let (c, _) = run(&[bt], 1, |c, _| c.sr |= SR_T);
    assert_eq!(c.pc, CODE + 4 + 4);
    assert_eq!(c.cycles, 3);
    let (c, _) = run(&[bt], 1, |_, _| {});
    assert_eq!((c.pc, c.cycles), (CODE + 2, 1));
    let bf = enc("10001011dddddddd", &[('d', 0xFE)]);
    let (c, _) = run(&[bf], 1, |_, _| {});
    assert_eq!(c.pc, CODE, "BF -2*2 back to itself");
    // BT/S runs the slot, then jumps.
    let bts = enc("10001101dddddddd", &[('d', 4)]);
    let addi = enc("0111nnnniiiiiiii", &[('n', 1), ('i', 1)]);
    let (c, _) = run(&[bts, addi], 1, |c, _| c.sr |= SR_T);
    assert_eq!((c.pc, c.r[1], c.instructions), (CODE + 4 + 8, 1, 2));
    let bfs = enc("10001111dddddddd", &[('d', 4)]);
    let (c, _) = run(&[bfs, addi], 1, |c, _| c.sr |= SR_T);
    assert_eq!(
        (c.pc, c.r[1]),
        (CODE + 2, 0),
        "not taken: the next instruction is not a slot"
    );
}

#[test]
fn unconditional_branches() {
    let addi = enc("0111nnnniiiiiiii", &[('n', 1), ('i', 1)]);
    let bra = enc("1010dddddddddddd", &[('d', 0xFFE)]);
    let (c, _) = run(&[bra, addi], 1, |_, _| {});
    assert_eq!(
        (c.pc, c.r[1]),
        (CODE, 1),
        "BRA -4 lands on itself after the slot"
    );
    let bsr = enc("1011dddddddddddd", &[('d', 0x10)]);
    let (c, _) = run(&[bsr, addi], 1, |_, _| {});
    assert_eq!((c.pc, c.pr, c.r[1]), (CODE + 4 + 0x20, CODE + 4, 1));
    let braf = enc("0000mmmm00100011", &[('m', 3)]);
    let (c, _) = run(&[braf, addi], 1, |c, _| c.r[3] = 0x100);
    assert_eq!(c.pc, CODE + 4 + 0x100);
    let bsrf = enc("0000mmmm00000011", &[('m', 3)]);
    let (c, _) = run(&[bsrf, addi], 1, |c, _| c.r[3] = 0x100);
    assert_eq!((c.pc, c.pr), (CODE + 4 + 0x100, CODE + 4));
    let jmp = enc("0100mmmm00101011", &[('m', 3)]);
    let (c, _) = run(&[jmp, addi], 1, |c, _| c.r[3] = 0x2468);
    assert_eq!((c.pc, c.r[1]), (0x2468, 1));
    let jsr = enc("0100mmmm00001011", &[('m', 3)]);
    let (c, _) = run(&[jsr, addi], 1, |c, _| c.r[3] = 0x2468);
    assert_eq!((c.pc, c.pr), (0x2468, CODE + 4));
}

#[test]
fn rts_target_is_taken_before_the_slot_changes_pr() {
    let rts = enc("0000000000001011", &[]);
    let ldspr = enc("0100mmmm00101010", &[('m', 4)]);
    let (c, _) = run(&[rts, ldspr], 1, |c, _| {
        c.pr = 0x2222;
        c.r[4] = 0x4444;
    });
    assert_eq!((c.pc, c.pr), (0x2222, 0x4444));
}

#[test]
fn rte_restores_pc_and_sr() {
    let rte = enc("0000000000101011", &[]);
    let (c, _) = run(&[rte, NOP], 1, |c, m| {
        c.r[15] = STACK - 8;
        m.write32(STACK - 8, 0x2468);
        m.write32(STACK - 4, 0xFFFF_FFFF);
    });
    assert_eq!((c.pc, c.sr, c.r[15]), (0x2468, 0x3F3, STACK));
}

// ---------------------------------------------------------------- exceptions

#[test]
fn general_illegal_saves_its_own_address() {
    let (c, mut m) = run(&[0xFFFF], 1, |c, _| c.sr = 0x0000_0071);
    assert_eq!(c.pc, handler(vector::GENERAL_ILLEGAL));
    assert_eq!(c.r[15], STACK - 8);
    assert_eq!(m.read32(STACK - 8), CODE, "saved PC");
    assert_eq!(m.read32(STACK - 4), 0x71, "saved SR");
    assert_eq!(c.sr, 0x71, "I bits untouched by instruction exceptions");
}

#[test]
fn slot_illegal_saves_the_branch_target() {
    let bra = enc("1010dddddddddddd", &[('d', 0x10)]);
    let (c, mut m) = run(&[bra, bra], 1, |_, _| {});
    assert_eq!(c.pc, handler(vector::SLOT_ILLEGAL));
    assert_eq!(m.read32(STACK - 8), CODE + 4 + 0x20);
    let (c, _) = run(&[bra, 0xFFFF], 1, |_, _| {});
    assert_eq!(
        c.pc,
        handler(vector::SLOT_ILLEGAL),
        "undefined code in a slot"
    );
}

#[test]
fn trapa_saves_the_next_instruction() {
    let (c, mut m) = run(&[enc("11000011iiiiiiii", &[('i', 40)])], 1, |_, _| {});
    assert_eq!(c.pc, handler(40));
    assert_eq!(m.read32(STACK - 8), CODE + 2);
    assert_eq!(c.cycles, 8);
}

#[test]
fn misaligned_access_raises_an_address_error_after_the_instruction() {
    let movl = nm("0110nnnnmmmm0110", 1, 2); // MOV.L @Rm+,Rn
    let (c, mut m) = run(&[movl], 1, |c, _| c.r[2] = 0x2002);
    assert_eq!(c.pc, handler(vector::CPU_ADDRESS_ERROR));
    assert_eq!(c.r[2], 0x2006, "the instruction completes");
    assert_eq!(m.read32(STACK - 8), CODE + 2);
    let (c, _) = run(&[nm("0010nnnnmmmm0001", 1, 2)], 1, |c, _| c.r[1] = 0x2001);
    assert_eq!(c.pc, handler(vector::CPU_ADDRESS_ERROR));
    let (c, _) = run(&[NOP], 1, |c, _| c.pc = CODE + 1);
    assert_eq!(
        c.pc,
        handler(vector::CPU_ADDRESS_ERROR),
        "odd instruction fetch"
    );
}

#[test]
fn interrupts_respect_the_mask_and_blocking_instructions() {
    let (mut c, mut m) = machine(&[NOP, NOP]);
    c.sr = 0x0000_0050; // mask 5
    assert!(!c.interrupt(&mut m, 5, 72));
    assert!(c.interrupt(&mut m, 6, 72));
    assert_eq!(c.pc, handler(72));
    assert_eq!(c.interrupt_mask(), 6);
    assert_eq!(m.read32(STACK - 4), 0x50, "the old SR is saved");
    assert_eq!(m.read32(STACK - 8), CODE);
    // NMI is taken whatever the mask, and sets it to 15.
    let (mut c, mut m) = machine(&[NOP]);
    c.sr = 0xF0;
    assert!(c.interrupt(&mut m, 16, vector::NMI));
    assert_eq!(c.interrupt_mask(), 15);
    // Not right after LDC.
    let ldc = enc("0100mmmm00001110", &[('m', 1)]);
    let (mut c, mut m) = machine(&[ldc, NOP]);
    c.step(&mut m);
    assert!(!c.accepts_interrupt(10));
    c.step(&mut m);
    assert!(c.accepts_interrupt(10));
}

#[test]
fn sleep_waits_for_an_interrupt() {
    let sleep = enc("0000000000011011", &[]);
    let (mut c, mut m) = machine(&[sleep, NOP]);
    c.step(&mut m);
    assert!(c.sleeping);
    assert_eq!(c.run(&mut m, 10), 0);
    assert!(c.interrupt(&mut m, 3, 80));
    assert!(!c.sleeping);
    assert_eq!(m.read32(STACK - 8), CODE + 2, "resumes after SLEEP");
}

// ---------------------------------------------------------------- system control

#[test]
fn control_and_system_registers() {
    let (c, _) = one(enc("0100mmmm00001110", &[('m', 1)]), |c, _| {
        c.r[1] = 0xFFFF_FFFF
    });
    assert_eq!(c.sr, 0x3F3, "LDC to SR keeps only M, Q, I, S, T");
    for (p, get) in [
        ("0100mmmm00011110", (|c: &Cpu| c.gbr) as fn(&Cpu) -> u32),
        ("0100mmmm00101110", |c| c.vbr),
        ("0100mmmm00001010", |c| c.mach),
        ("0100mmmm00011010", |c| c.macl),
        ("0100mmmm00101010", |c| c.pr),
    ] {
        let (c, _) = one(enc(p, &[('m', 1)]), |c, _| c.r[1] = 0x1357_9BDF);
        assert_eq!(get(&c), 0x1357_9BDF, "{p}");
    }
    for (p, set) in [
        (
            "0000nnnn00000010",
            (|c: &mut Cpu| c.sr = 0x0F3) as fn(&mut Cpu),
        ),
        ("0000nnnn00010010", |c| c.gbr = 0x0F3),
        ("0000nnnn00100010", |c| c.vbr = 0x0F3),
        ("0000nnnn00001010", |c| c.mach = 0x0F3),
        ("0000nnnn00011010", |c| c.macl = 0x0F3),
        ("0000nnnn00101010", |c| c.pr = 0x0F3),
    ] {
        let (c, _) = one(enc(p, &[('n', 1)]), |c, _| set(c));
        assert_eq!(c.r[1], 0x0F3, "{p}");
    }
}

#[test]
fn control_registers_through_the_stack() {
    // STC.L / STS.L push; LDC.L / LDS.L pop.
    for (st, ld, set, get) in [
        (
            "0100nnnn00000011",
            "0100mmmm00000111",
            (|c: &mut Cpu| c.sr = 0x3F3) as fn(&mut Cpu),
            (|c: &Cpu| c.sr) as fn(&Cpu) -> u32,
        ),
        (
            "0100nnnn00010011",
            "0100mmmm00010111",
            |c| c.gbr = 0xAB,
            |c| c.gbr,
        ),
        (
            "0100nnnn00100011",
            "0100mmmm00100111",
            |c| c.vbr = 0xAB,
            |c| c.vbr,
        ),
        (
            "0100nnnn00000010",
            "0100mmmm00000110",
            |c| c.mach = 0xAB,
            |c| c.mach,
        ),
        (
            "0100nnnn00010010",
            "0100mmmm00010110",
            |c| c.macl = 0xAB,
            |c| c.macl,
        ),
        (
            "0100nnnn00100010",
            "0100mmmm00100110",
            |c| c.pr = 0xAB,
            |c| c.pr,
        ),
    ] {
        let (c, mut m) = one(enc(st, &[('n', 15)]), |c, _| set(c));
        assert_eq!(c.r[15], STACK - 4, "{st}");
        let saved = m.read32(STACK - 4);
        assert_eq!(saved, get(&c), "{st}");
        let (c, _) = one(enc(ld, &[('m', 15)]), |c, m| {
            c.r[15] = STACK - 4;
            m.write32(STACK - 4, saved);
        });
        assert_eq!((get(&c), c.r[15]), (saved, STACK), "{ld}");
    }
}

#[test]
fn flags_and_mac_clear() {
    let (c, _) = one(enc("0000000000011000", &[]), |_, _| {});
    assert_eq!(c.t(), 1);
    let (c, _) = one(enc("0000000000001000", &[]), |c, _| c.sr |= SR_T);
    assert_eq!(c.t(), 0);
    let (c, _) = one(enc("0000000000101000", &[]), |c, _| {
        c.mach = 1;
        c.macl = 2;
    });
    assert_eq!((c.mach, c.macl), (0, 0));
}

// ---------------------------------------------------------------- FPU instructions

#[test]
fn fpu_moves() {
    let (c, _) = one(enc("1111nnnnmmmm1000", &[('n', 3), ('m', 1)]), |c, m| {
        c.r[1] = 0x2000;
        m.write32(0x2000, 0x4049_0FDB);
    });
    assert_eq!(c.fr[3], 0x4049_0FDB);
    let (c, _) = one(enc("1111nnnnmmmm1001", &[('n', 3), ('m', 1)]), |c, m| {
        c.r[1] = 0x2000;
        m.write32(0x2000, 7);
    });
    assert_eq!((c.fr[3], c.r[1]), (7, 0x2004));
    let (c, _) = one(enc("1111nnnnmmmm0110", &[('n', 3), ('m', 1)]), |c, m| {
        c.r[0] = 4;
        c.r[1] = 0x2000;
        m.write32(0x2004, 9);
    });
    assert_eq!(c.fr[3], 9);
    let (_, mut m) = one(enc("1111nnnnmmmm1010", &[('n', 1), ('m', 3)]), |c, _| {
        c.r[1] = 0x2000;
        c.fr[3] = 0x1111_2222;
    });
    assert_eq!(m.read32(0x2000), 0x1111_2222);
    let (c, mut m) = one(enc("1111nnnnmmmm1011", &[('n', 1), ('m', 3)]), |c, _| {
        c.r[1] = 0x2004;
        c.fr[3] = 0x1111_2222;
    });
    assert_eq!((c.r[1], m.read32(0x2000)), (0x2000, 0x1111_2222));
    let (_, mut m) = one(enc("1111nnnnmmmm0111", &[('n', 1), ('m', 3)]), |c, _| {
        c.r[0] = 8;
        c.r[1] = 0x2000;
        c.fr[3] = 0x3333_4444;
    });
    assert_eq!(m.read32(0x2008), 0x3333_4444);
    let (c, _) = one(enc("1111nnnnmmmm1100", &[('n', 2), ('m', 3)]), |c, _| {
        c.fr[3] = 0x0000_0001
    });
    assert_eq!(c.fr[2], 1, "FMOV copies a denormal unchanged");
    let (c, _) = one(enc("1111nnnn10011101", &[('n', 2)]), |_, _| {});
    assert_eq!(c.fr[2], 0x3F80_0000);
    let (c, _) = one(enc("1111nnnn10001101", &[('n', 2)]), |c, _| c.fr[2] = 5);
    assert_eq!(c.fr[2], 0);
    let (c, _) = one(enc("1111mmmm00011101", &[('m', 2)]), |c, _| {
        c.fr[2] = 0x1234
    });
    assert_eq!(c.fpul, 0x1234);
    let (c, _) = one(enc("1111nnnn00001101", &[('n', 2)]), |c, _| c.fpul = 0x5678);
    assert_eq!(c.fr[2], 0x5678);
}

#[test]
fn fpu_arithmetic_through_instructions() {
    let f = |x: f32| x.to_bits();
    let op = |p: &str, a: f32, b: f32| {
        let (c, _) = one(enc(p, &[('n', 1), ('m', 2)]), |c, _| {
            c.fr[1] = f(a);
            c.fr[2] = f(b);
            c.fr[0] = f(2.0);
        });
        c.fr[1]
    };
    assert_eq!(op("1111nnnnmmmm0000", 1.5, 2.25), f(3.75));
    assert_eq!(op("1111nnnnmmmm0001", 1.5, 2.25), f(-0.75));
    assert_eq!(op("1111nnnnmmmm0010", 1.5, 2.0), f(3.0));
    assert_eq!(op("1111nnnnmmmm0011", 3.0, 2.0), f(1.5));
    assert_eq!(
        op("1111nnnnmmmm1110", 1.0, 3.0),
        f(7.0),
        "FMAC: FR0*FRm + FRn"
    );
    let (c, _) = one(enc("1111nnnn01001101", &[('n', 1)]), |c, _| {
        c.fr[1] = f(2.0)
    });
    assert_eq!(c.fr[1], f(-2.0));
    let (c, _) = one(enc("1111nnnn01011101", &[('n', 1)]), |c, _| {
        c.fr[1] = f(-2.0)
    });
    assert_eq!(c.fr[1], f(2.0));
    let (c, _) = one(enc("1111nnnn00101101", &[('n', 1)]), |c, _| {
        c.fpul = (-7i32) as u32
    });
    assert_eq!(c.fr[1], f(-7.0));
    let (c, _) = one(enc("1111mmmm00111101", &[('m', 1)]), |c, _| {
        c.fr[1] = f(-7.9)
    });
    assert_eq!(c.fpul, (-7i32) as u32);
    let cmp = |p: &str, a: f32, b: f32| {
        let (c, _) = one(enc(p, &[('n', 1), ('m', 2)]), |c, _| {
            c.fr[1] = f(a);
            c.fr[2] = f(b);
        });
        c.t()
    };
    assert_eq!(cmp("1111nnnnmmmm0100", 1.0, 1.0), 1);
    assert_eq!(cmp("1111nnnnmmmm0101", 2.0, 1.0), 1);
    assert_eq!(cmp("1111nnnnmmmm0101", 1.0, 2.0), 0);
    assert_eq!(cmp("1111nnnnmmmm0101", 1.0, 1.0), 0, "FCMP/GT is strict");
    assert_eq!(cmp("1111nnnnmmmm0100", 1.0, 2.0), 0);
}

#[test]
fn fpscr_keeps_its_fixed_bits() {
    let (c, _) = one(enc("0100mmmm01101010", &[('m', 1)]), |c, _| {
        c.r[1] = 0xFFFF_FFFF
    });
    assert_eq!(c.fpscr, crate::FPSCR_WRITABLE | crate::FPSCR_FIXED);
    let (c, _) = one(enc("0100mmmm01101010", &[('m', 1)]), |c, _| c.r[1] = 0);
    assert_eq!(c.fpscr, 0x0004_0001, "RM = 01 and DN = 1 cannot be cleared");
    let (c, _) = one(enc("0000nnnn01101010", &[('n', 2)]), |_, _| {});
    assert_eq!(c.r[2], 0x0004_0001);
    let (c, _) = one(enc("0100mmmm01011010", &[('m', 1)]), |c, _| c.r[1] = 0xAB);
    assert_eq!(c.fpul, 0xAB);
    let (c, _) = one(enc("0000nnnn01011010", &[('n', 2)]), |c, _| c.fpul = 0xCD);
    assert_eq!(c.r[2], 0xCD);
    let (c, mut m) = one(enc("0100nnnn01010010", &[('n', 15)]), |c, _| c.fpul = 0xEF);
    assert_eq!((c.r[15], m.read32(STACK - 4)), (STACK - 4, 0xEF));
    let (c, mut m) = one(enc("0100nnnn01100010", &[('n', 15)]), |_, _| {});
    assert_eq!(m.read32(c.r[15]), 0x0004_0001);
    let (c, _) = one(enc("0100mmmm01010110", &[('m', 15)]), |c, m| {
        c.r[15] = STACK - 4;
        m.write32(STACK - 4, 0x99);
    });
    assert_eq!((c.fpul, c.r[15]), (0x99, STACK));
    let (c, _) = one(enc("0100mmmm01100110", &[('m', 15)]), |c, m| {
        c.r[15] = STACK - 4;
        m.write32(STACK - 4, 0);
    });
    assert_eq!(c.fpscr, 0x0004_0001);
}

#[test]
fn enabled_fpu_exceptions_leave_the_destination_unchanged() {
    let fdiv = enc("1111nnnnmmmm0011", &[('n', 1), ('m', 2)]);
    // Disabled: x / 0 = inf, flag and cause set.
    let (c, _) = one(fdiv, |c, _| {
        c.fr[1] = 1.0f32.to_bits();
        c.fr[2] = 0;
    });
    assert_eq!(c.fr[1], 0x7F80_0000);
    assert_eq!(c.fpscr & (1 << 15 | 1 << 5), 1 << 15 | 1 << 5);
    // Enabled (EZ): exception through vector 13, FR1 unchanged, next PC saved.
    let (c, mut m) = one(fdiv, |c, _| {
        c.fr[1] = 1.0f32.to_bits();
        c.fr[2] = 0;
        c.fpscr |= 1 << 10;
    });
    assert_eq!(c.fr[1], 1.0f32.to_bits());
    assert_eq!(c.pc, handler(vector::FPU));
    assert_eq!(m.read32(STACK - 8), CODE + 2);
}

#[test]
fn rz_vectors_from_the_exact_oracle() {
    // Generated by tools/fpu_vectors.py with exact rational arithmetic:
    // op a b [c] expected, all as hex bit patterns.
    let text = include_str!("testdata/fpu_rz.txt");
    let mut c = Cpu::new();
    let mut checked = 0;
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    {
        let f: Vec<&str> = line.split_whitespace().collect();
        let h = |s: &str| u32::from_str_radix(s, 16).unwrap();
        let (got, want) = match f[0] {
            "add" => (c.fadd_bits(h(f[1]), h(f[2])), h(f[3])),
            "sub" => (c.fsub_bits(h(f[1]), h(f[2])), h(f[3])),
            "mul" => (c.fmul_bits(h(f[1]), h(f[2])), h(f[3])),
            "div" => (c.fdiv_bits(h(f[1]), h(f[2])), h(f[3])),
            "mac" => (c.fmac_bits(h(f[1]), h(f[2]), h(f[3])), h(f[4])),
            "float" => (c.float_bits(h(f[1])), h(f[2])),
            other => panic!("unknown op {other}"),
        };
        assert_eq!(got, want, "{line}: got {got:08X}");
        checked += 1;
    }
    assert!(checked > 1000, "only {checked} vectors");
}

// ---------------------------------------------------------------- from the software manual

#[test]
fn predecrement_store_with_the_same_register_stores_the_original() {
    // Software manual 7.2.30: Write_Long(R[n]-4, R[m]); R[n] -= 4.
    let (c, mut m) = one(nm("0010nnnnmmmm0110", 1, 1), |c, _| c.r[1] = 0x2008);
    assert_eq!(c.r[1], 0x2004);
    assert_eq!(m.read32(0x2004), 0x2008);
}

#[test]
fn pc_relative_in_a_delay_slot_sees_the_target_plus_2() {
    // Software manual, MOVA: "the PC must point to (the branch destination) + 2".
    // BRA to CODE + 0x22; slot MOVA @(0,PC),R0 -> (target + 2) & ~3 = target + 2 here.
    let bra = enc("1010dddddddddddd", &[('d', 0x0F)]); // CODE + 4 + 0x1E = CODE + 0x22
    let mova = enc("11000111dddddddd", &[('d', 0)]);
    let (c, _) = run(&[bra, mova], 1, |_, _| {});
    assert_eq!(c.pc, CODE + 0x22);
    assert_eq!(c.r[0], CODE + 0x24);
    // MOV.W @(disp,PC),Rn in the slot reads target + 2 + disp * 2, without alignment.
    let movw = enc("1001nnnndddddddd", &[('n', 3), ('d', 1)]);
    let (c, _) = run(&[bra, movw], 1, |_, m| m.write16(CODE + 0x26, 0x1234));
    assert_eq!(c.r[3], 0x1234);
}

#[test]
fn fneg_and_fabs_follow_the_special_case_tables() {
    let neg = enc("1111nnnn01001101", &[('n', 1)]);
    let abs = enc("1111nnnn01011101", &[('n', 1)]);
    let run1 = |code: u16, x: u32| {
        let (c, _) = one(code, |c, _| c.fr[1] = x);
        (c.fr[1], c.fpscr & (1 << 16) != 0)
    };
    assert_eq!(
        run1(neg, 0x7F80_0001),
        (crate::QNAN, false),
        "qNaN -> default qNaN"
    );
    assert_eq!(
        run1(neg, 0x7FC0_0000),
        (crate::QNAN, true),
        "sNaN -> invalid"
    );
    assert_eq!(run1(abs, 0xFFC0_0000), (crate::QNAN, true));
    assert_eq!(
        run1(neg, 0x0000_0001),
        (0x8000_0000, false),
        "denormal counts as +0"
    );
    assert_eq!(run1(abs, 0x8000_0001), (0, false));
    assert_eq!(run1(neg, 0xFF80_0000), (0x7F80_0000, false));
}
