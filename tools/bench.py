"""Interleaved speed comparison of sh705x builds on the same flash image.

    python tools/bench.py --flash <flash.bin> --runs 5 --max 100000000 A=path/a.exe B=path/b.exe

Runs every build once per round, in turn, so slow drifts of the machine (clock,
temperature, other jobs) hit all of them alike, and prints the median and spread of
M instructions/s for each, and each build's median relative to the first one.
"""

import argparse
import re
import statistics
import subprocess


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--flash", required=True)
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--max", type=int, default=100_000_000)
    ap.add_argument("builds", nargs="+", help="NAME=path to sh705x executable")
    a = ap.parse_args()
    builds = [b.split("=", 1) for b in a.builds]
    speeds = {name: [] for name, _ in builds}
    cycles = {}
    for r in range(a.runs):
        for name, exe in builds:
            out = subprocess.run(
                [exe, "trace", "--flash", a.flash, "--max", str(a.max)],
                capture_output=True, text=True, check=True).stdout
            m = re.search(r"(\d+) cycles .* ([\d.]+) M instructions/s", out)
            cycles.setdefault(name, set()).add(m.group(1))
            speeds[name].append(float(m.group(2)))
        print(f"round {r + 1}: " + "  ".join(f"{n} {speeds[n][-1]:.1f}" for n, _ in builds), flush=True)
    base = statistics.median(speeds[builds[0][0]])
    for name, _ in builds:
        s = speeds[name]
        med = statistics.median(s)
        print(f"{name}: median {med:.2f} M instr/s, min {min(s):.2f}, max {max(s):.2f}, "
              f"{100 * (med / base - 1):+.1f} % vs {builds[0][0]}; cycles {sorted(cycles[name])}")


if __name__ == "__main__":
    main()
