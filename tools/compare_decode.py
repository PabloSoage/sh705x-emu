#!/usr/bin/env python3
"""Compares the emulator's decoder with Ghidra's SuperH disassembler, code by code.

For every 16-bit code it checks, against Ghidra's output:
  - the mnemonic;
  - the registers, in operand order (which catches swapped or misplaced n/m fields);
  - the numbers: immediates, scaled displacements, and PC-relative targets computed
    with the code at address 2 * code (which catches scaling and PC-relative mistakes).

Ghidra is run with the SH-2A language, a superset of the SH-2E, so codes that Ghidra
decodes and the emulator calls illegal are listed apart: they must all be SH-2A-only
instructions, absent from tables 2.12 to 2.19 of the SH7059 manual.

Usage:
  cargo run -q --release -p sh2e --example decode_table > ours.txt
  (Ghidra) tools/ghidra/DecodeAll.py on all_codes.bin  > ghidra.txt
  compare_decode.py ours.txt ghidra.txt
all_codes.bin holds each code 0x0000..0xFFFF, big-endian, at address 2 * code.
"""
import re
import sys
from collections import Counter, defaultdict


def fields(c):
    n, m = (c >> 8) & 15, (c >> 4) & 15
    i8 = c & 0xFF
    s8 = i8 - 256 if i8 & 0x80 else i8
    d4 = c & 15
    s12 = c & 0xFFF
    s12 = s12 - 4096 if s12 & 0x800 else s12
    return n, m, i8, s8, d4, s12


def expect(op, c):
    """(mnemonic, registers, numbers) the emulator's reading of `c` implies."""
    a = 2 * c
    n, m, i8, s8, d4, s12 = fields(c)
    rn, rm, frn, frm = f"r{n}", f"r{m}", f"fr{n}", f"fr{m}"
    pc4 = a + 4
    two = {  # Rm,Rn instructions: mnemonic
        "Mov": "mov", "Add": "add", "Addc": "addc", "Addv": "addv", "CmpEq": "cmp/eq",
        "CmpHs": "cmp/hs", "CmpGe": "cmp/ge", "CmpHi": "cmp/hi", "CmpGt": "cmp/gt",
        "CmpStr": "cmp/str", "Div1": "div1", "Div0s": "div0s", "DmulsL": "dmuls.l",
        "DmuluL": "dmulu.l", "ExtsB": "exts.b", "ExtsW": "exts.w", "ExtuB": "extu.b",
        "ExtuW": "extu.w", "MacL": "mac.l", "MacW": "mac.w", "MulL": "mul.l",
        "MulsW": "muls.w", "MuluW": "mulu.w", "Neg": "neg", "Negc": "negc", "Sub": "sub",
        "Subc": "subc", "Subv": "subv", "And": "and", "Not": "not", "Or": "or",
        "Tst": "tst", "Xor": "xor", "SwapB": "swap.b", "SwapW": "swap.w", "Xtrct": "xtrct",
        "MovBStore": "mov.b", "MovWStore": "mov.w", "MovLStore": "mov.l",
        "MovBLoad": "mov.b", "MovWLoad": "mov.w", "MovLLoad": "mov.l",
        "MovBStoreDec": "mov.b", "MovWStoreDec": "mov.w", "MovLStoreDec": "mov.l",
        "MovBLoadInc": "mov.b", "MovWLoadInc": "mov.w", "MovLLoadInc": "mov.l",
    }
    if op in two:
        return two[op], [rm, rn], []
    one = {  # Rn instructions
        "Movt": "movt", "CmpPl": "cmp/pl", "CmpPz": "cmp/pz", "Dt": "dt", "TasB": "tas.b",
        "Rotl": "rotl", "Rotr": "rotr", "Rotcl": "rotcl", "Rotcr": "rotcr", "Shal": "shal",
        "Shar": "shar", "Shll": "shll", "Shlr": "shlr", "Shll2": "shll2", "Shlr2": "shlr2",
        "Shll8": "shll8", "Shlr8": "shlr8", "Shll16": "shll16", "Shlr16": "shlr16",
        "Braf": "braf", "Bsrf": "bsrf", "Jmp": "jmp", "Jsr": "jsr",
        "LdcSr": "ldc", "LdcGbr": "ldc", "LdcVbr": "ldc", "LdcLSr": "ldc.l",
        "LdcLGbr": "ldc.l", "LdcLVbr": "ldc.l", "LdsMach": "lds", "LdsMacl": "lds",
        "LdsPr": "lds", "LdsLMach": "lds.l", "LdsLMacl": "lds.l", "LdsLPr": "lds.l",
        "StcSr": "stc", "StcGbr": "stc", "StcVbr": "stc", "StcLSr": "stc.l",
        "StcLGbr": "stc.l", "StcLVbr": "stc.l", "StsMach": "sts", "StsMacl": "sts",
        "StsPr": "sts", "StsLMach": "sts.l", "StsLMacl": "sts.l", "StsLPr": "sts.l",
        "LdsFpscr": "lds", "LdsFpul": "lds", "LdsLFpscr": "lds.l", "LdsLFpul": "lds.l",
        "StsFpscr": "sts", "StsFpul": "sts", "StsLFpscr": "sts.l", "StsLFpul": "sts.l",
    }
    if op in one:
        return one[op], [rn], []
    none = {"Nop": "nop", "Rts": "rts", "Rte": "rte", "Clrt": "clrt", "Sett": "sett",
            "Clrmac": "clrmac", "Div0u": "div0u", "Sleep": "sleep"}
    if op in none:
        return none[op], [], []
    fone = {"Fneg": "fneg", "Fabs": "fabs", "Fldi0": "fldi0", "Fldi1": "fldi1",
            "Fsts": "fsts", "Flds": "flds", "Float": "float", "Ftrc": "ftrc"}
    if op in fone:
        return fone[op], [frn], []
    ftwo = {"Fadd": "fadd", "Fsub": "fsub", "Fmul": "fmul", "Fdiv": "fdiv",
            "FcmpEq": "fcmp/eq", "FcmpGt": "fcmp/gt", "Fmov": "fmov"}
    if op in ftwo:
        return ftwo[op], [frm, frn], []
    size = {"B": 1, "W": 2, "L": 4}
    table = {
        "MovImm": ("mov", [rn], [s8]),
        "MovWPc": ("mov.w", [rn], [pc4 + i8 * 2]),
        "MovLPc": ("mov.l", [rn], [(pc4 & ~3) + i8 * 4]),
        "Mova": ("mova", ["r0"], [(pc4 & ~3) + i8 * 4]),
        "MovBStoreDisp": ("mov.b", ["r0", rm], [d4]),
        "MovWStoreDisp": ("mov.w", ["r0", rm], [d4 * 2]),
        "MovLStoreDisp": ("mov.l", [rm, rn], [d4 * 4]),
        "MovBLoadDisp": ("mov.b", [rm, "r0"], [d4]),
        "MovWLoadDisp": ("mov.w", [rm, "r0"], [d4 * 2]),
        "MovLLoadDisp": ("mov.l", [rm, rn], [d4 * 4]),
        "MovBStoreIdx": ("mov.b", [rm, "r0", rn], []),
        "MovWStoreIdx": ("mov.w", [rm, "r0", rn], []),
        "MovLStoreIdx": ("mov.l", [rm, "r0", rn], []),
        "MovBLoadIdx": ("mov.b", ["r0", rm, rn], []),
        "MovWLoadIdx": ("mov.w", ["r0", rm, rn], []),
        "MovLLoadIdx": ("mov.l", ["r0", rm, rn], []),
        "AddImm": ("add", [rn], [s8]),
        "CmpEqImm": ("cmp/eq", ["r0"], [s8]),
        "AndImm": ("and", ["r0"], [i8]),
        "OrImm": ("or", ["r0"], [i8]),
        "XorImm": ("xor", ["r0"], [i8]),
        "TstImm": ("tst", ["r0"], [i8]),
        "AndB": ("and.b", ["r0"], [i8]),
        "OrB": ("or.b", ["r0"], [i8]),
        "XorB": ("xor.b", ["r0"], [i8]),
        "TstB": ("tst.b", ["r0"], [i8]),
        "Trapa": ("trapa", [], [i8]),
        "Bt": ("bt", [], [pc4 + s8 * 2]),
        "Bf": ("bf", [], [pc4 + s8 * 2]),
        "Bts": ("bt/s", [], [pc4 + s8 * 2]),
        "Bfs": ("bf/s", [], [pc4 + s8 * 2]),
        "Bra": ("bra", [], [pc4 + s12 * 2]),
        "Bsr": ("bsr", [], [pc4 + s12 * 2]),
        "Fmac": ("fmac", ["fr0", frm, frn], []),
        "FmovLoad": ("fmov.s", [rm, frn], []),
        "FmovLoadInc": ("fmov.s", [rm, frn], []),
        "FmovLoadIdx": ("fmov.s", ["r0", rm, frn], []),
        "FmovStore": ("fmov.s", [frm, rn], []),
        "FmovStoreDec": ("fmov.s", [frm, rn], []),
        "FmovStoreIdx": ("fmov.s", [frm, "r0", rn], []),
    }
    for k in ("B", "W", "L"):
        table[f"Mov{k}StoreGbr"] = (f"mov.{k.lower()}", ["r0"], [i8 * size[k]])
        table[f"Mov{k}LoadGbr"] = (f"mov.{k.lower()}", ["r0"], [i8 * size[k]])
    if op in table:
        return table[op]
    raise KeyError(op)


REG = re.compile(r"(?<![a-z])(f?r\d+)(?![0-9a-z])")
NUM = re.compile(r"#?(-?)0x([0-9a-f]+)|#(-?\d+)")


def parse(operands):
    regs = REG.findall(operands)
    nums = []
    for neg, hx, dec in NUM.findall(operands):
        v = int(hx, 16) if hx else int(dec)
        nums.append(-v if neg else v)
    return regs, nums


def main(ours_path, ghidra_path):
    ours = dict(l.rstrip("\n").split("\t") for l in open(ours_path))
    gh = {}
    for l in open(ghidra_path):
        p = l.rstrip("\n").split("\t")
        gh[p[0]] = (p[1], p[2] if len(p) > 2 else "")
    agree, mismatch, unchecked = 0, [], Counter()
    ghidra_only = defaultdict(list)
    ours_only = defaultdict(list)
    for code_hex in sorted(ours):
        c = int(code_hex, 16)
        op = ours[code_hex]
        gm, gops = gh[code_hex]
        if op == "Illegal":
            if gm == "-":
                agree += 1
            else:
                ghidra_only[gm].append(code_hex)
            continue
        if gm == "-":
            ours_only[op].append(code_hex)
            continue
        em, eregs, enums = expect(op, c)
        regs, nums = parse(gops)
        ok = gm == em and regs == eregs and [x & 0xFFFFFFFF for x in nums] == [
            x & 0xFFFFFFFF for x in enums]
        if ok:
            agree += 1
        else:
            mismatch.append((code_hex, op, f"{em} {eregs} {[hex(x) for x in enums]}",
                             f"{gm} {gops}"))
    print(f"codes: {len(ours)}  agree: {agree}  mismatches: {len(mismatch)}")
    for row in mismatch[:60]:
        print("  MISMATCH", *row)
    print(f"\nGhidra (SH-2A) decodes, emulator says illegal: "
          f"{sum(len(v) for v in ghidra_only.values())} codes")
    for mn, codes in sorted(ghidra_only.items(), key=lambda kv: -len(kv[1])):
        print(f"  {mn:10s} {len(codes):5d}  e.g. {' '.join(codes[:4])}")
    print(f"\nEmulator decodes, Ghidra gives nothing: "
          f"{sum(len(v) for v in ours_only.values())} codes")
    for op, codes in sorted(ours_only.items(), key=lambda kv: -len(kv[1])):
        print(f"  {op:12s} {len(codes):5d}  e.g. {' '.join(codes[:4])}")
    return 0 if not mismatch else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1], sys.argv[2]))
