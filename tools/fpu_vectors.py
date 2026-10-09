#!/usr/bin/env python3
"""Test vectors for the SH-2E FPU, computed with exact rational arithmetic.

An independent oracle for crates/sh2e/src/fpu.rs: no floating-point arithmetic is used
here, so the host's rounding cannot leak into the expected results.

Rules (manual REJ09B0177-0500, section 3):
  - round toward zero;
  - denormal operands and results flushed to zero, keeping the sign;
  - overflow gives the largest finite value with the right sign.
Only finite, non-NaN operands and results without invalid operations are generated;
special cases are covered by hand-written tests.

Usage: fpu_vectors.py > crates/sh2e/src/testdata/fpu_rz.txt
"""
import random
from fractions import Fraction

MAX = 0x7F7FFFFF
SIGN = 0x80000000


def value(bits):
    """Exact value of a single, with denormals flushed to zero."""
    s = -1 if bits & SIGN else 1
    e = (bits >> 23) & 0xFF
    f = bits & 0x7FFFFF
    if e == 0:
        return Fraction(0), s
    assert e != 0xFF, "no infinities or NaN here"
    return s * Fraction(0x800000 | f, 1 << 23) * Fraction(2) ** (e - 127), s


def rz(x, zero_sign=1):
    """Round an exact value toward zero to single-precision bits."""
    if x == 0:
        return 0 if zero_sign > 0 else SIGN
    sign = SIGN if x < 0 else 0
    a = abs(x)
    if a >= value(MAX)[0]:
        return sign | MAX
    if a < Fraction(1, 2 ** 126):
        return sign  # below the smallest normal: flushed
    e = 0
    while a >= 2:
        a /= 2
        e += 1
    while a < 1:
        a *= 2
        e -= 1
    frac = int((a - 1) * (1 << 23))  # floor = toward zero for a positive value
    return sign | ((e + 127) << 23) | frac


def rand_single(rng, kind):
    if kind == "near":  # exponents close together, so the operations interact
        e = rng.randint(100, 154)
    elif kind == "wide":
        e = rng.randint(1, 254)
    elif kind == "edge":
        return rng.choice([0x00800000, 0x00800001, 0x7F7FFFFF, 0x7F7FFFFE,
                           0x3F800000, 0x3F800001, 0x3F7FFFFF, 0x00000001,
                           0x007FFFFF, 0x4B800000, 0x4B7FFFFF]) | rng.choice([0, SIGN])
    f = rng.getrandbits(23)
    if rng.random() < 0.2:
        f = rng.choice([0, 1, 0x7FFFFF, 0x400000, 0x3FFFFF])
    return rng.choice([0, SIGN]) | (e << 23) | f


def main():
    rng = random.Random(20261009)
    out = ["# op a b [c] expected (hex). Exact rational arithmetic; see tools/fpu_vectors.py"]
    for _ in range(600):
        kind = rng.choice(["near", "near", "wide", "edge"])
        a, b = rand_single(rng, kind), rand_single(rng, kind)
        va, sa = value(a)
        vb, sb = value(b)
        # Exact zero sums are +0 under round-toward-zero unless both operands are -0.
        zs = -1 if (sa < 0 and sb < 0 and va == 0 and vb == 0) else 1
        out.append(f"add {a:08X} {b:08X} {rz(va + vb, zs):08X}")
        zs = -1 if (sa < 0 and sb > 0 and va == 0 and vb == 0) else 1
        out.append(f"sub {a:08X} {b:08X} {rz(va - vb, zs):08X}")
        out.append(f"mul {a:08X} {b:08X} {rz(va * vb, sa * sb):08X}")
        if vb != 0:
            out.append(f"div {a:08X} {b:08X} {rz(va / vb, sa * sb):08X}")
        c = rand_single(rng, kind)
        vc, sc = value(c)
        p = va * vb
        if p == 0 and vc == 0:
            zs = -1 if (sa * sb < 0 and sc < 0) else 1
        else:
            zs = 1
        out.append(f"mac {a:08X} {b:08X} {c:08X} {rz(p + vc, zs):08X}")
    for _ in range(200):
        i = rng.choice([rng.getrandbits(32), rng.getrandbits(25), rng.getrandbits(12),
                        0x80000000, 0x7FFFFFFF, 0x01000001, 0xFEFFFFFF])
        signed = i - (1 << 32) if i & SIGN else i
        out.append(f"float {i:08X} {rz(Fraction(signed)):08X}")
    print("\n".join(out))


if __name__ == "__main__":
    main()
