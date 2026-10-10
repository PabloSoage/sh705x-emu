"""Interleaved speed comparison of sh705x builds on the same flash image.

    python tools/bench.py --flash <flash.bin> --runs 5 --max 100000000 A=path/a.exe B=path/b.exe
    python tools/bench.py --flash <flash.bin> --eeprom <eeprom.bin> --rpm 800 A=... B=...

The first form times `sh705x trace` (the boot, peripherals as plain storage); the
second, chosen by --eeprom, times `sh705x bench` (the ECU booted with its EEPROM, then
five seconds of ECU time with CAN running and, with --rpm, the engine turning) and also
prints the real-time factor.

Runs every build once per round, in turn, so slow drifts of the machine (clock,
temperature, other jobs) hit all of them alike, and prints the median and spread of
M instructions/s for each, and each build's median relative to the first one. The
instruction or cycle count of every run is kept too: builds that should behave the same
must show one and the same.
"""

import argparse
import re
import statistics
import subprocess


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--flash", required=True)
    ap.add_argument("--eeprom")
    ap.add_argument("--rpm", type=int)
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--max", type=int, default=100_000_000)
    ap.add_argument("builds", nargs="+", help="NAME=path to sh705x executable")
    a = ap.parse_args()
    builds = [b.split("=", 1) for b in a.builds]
    if a.eeprom:
        args = ["bench", "--flash", a.flash, "--eeprom", a.eeprom]
        if a.rpm:
            args += ["--rpm", str(a.rpm)]
        pattern = r"factor ([\d.]+); (\d+) instructions, ([\d.]+) M instructions/s"
    else:
        args = ["trace", "--flash", a.flash, "--max", str(a.max)]
        pattern = r"(\d+) cycles .* ([\d.]+) M instructions/s"
    speeds = {name: [] for name, _ in builds}
    factors = {name: [] for name, _ in builds}
    counts = {}
    for r in range(a.runs):
        for name, exe in builds:
            out = subprocess.run([exe] + args, capture_output=True, text=True, check=True).stdout
            m = re.search(pattern, out)
            counts.setdefault(name, set()).add(m.group(m.lastindex - 1))
            speeds[name].append(float(m.group(m.lastindex)))
            if a.eeprom:
                factors[name].append(float(m.group(1)))
        print(f"round {r + 1}: " + "  ".join(f"{n} {speeds[n][-1]:.1f}" for n, _ in builds), flush=True)
    base = statistics.median(speeds[builds[0][0]])
    for name, _ in builds:
        s = speeds[name]
        med = statistics.median(s)
        rt = f", real-time factor {statistics.median(factors[name]):.3f}" if a.eeprom else ""
        print(f"{name}: median {med:.2f} M instr/s, min {min(s):.2f}, max {max(s):.2f}, "
              f"{100 * (med / base - 1):+.1f} % vs {builds[0][0]}; counts {sorted(counts[name])}{rt}")


if __name__ == "__main__":
    main()
