#!/usr/bin/env python3
"""Summarise a scanmatik-bench route into cells for `sh705x replay`.

A route is logged long: one row per (ms, parameter, identifier, unit, value). This bins the
running part of it by engine speed and pedal, and for each occupied cell writes the median
of the inputs to drive and of the setpoints the car logged there. `sh705x replay --cells`
then drives those inputs into the firmware and compares.

Usage: python route_cells.py <route.csv.gz> [--out cells.csv] [--rpm-step 250]
       [--pedal-step 10] [--min 200]

Columns written: rpm,pedal,coolant,intake,boost,baro,maf,rail,boost_cmd,vgt,air,torque,n
The first seven are driven (physical units, as the ECU reports them); the next five are the
recorded setpoints to compare; n is how many samples the cell holds.
"""

import argparse
import collections
import csv
import gzip
import statistics
import sys

# Identifier of each field, in a scanmatik-bench route of this car.
IDENT = {
    "rpm": "0x000C",
    "pedal": "0x0049",  # APP Sensor 1 Position, %
    "coolant": "0x0005",  # ECT Sensor
    "intake": "0x000F",  # IAT Sensor 1
    "boost": "0x2002",  # Boost Pressure Sensor (MAP), kPa
    "baro": "0x0033",  # BARO, kPa
    "maf": "0x0010",  # MAF Sensor, g/s
    "rail": "0x308A",  # Desired Fuel Rail Pressure
    "boost_cmd": "0x328A",  # Desired Boost Pressure
    "vgt": "0x2041",  # Turbocharger Vane Position Solenoid Valve Command
    "air": "0x327B",  # Desired Air Flow
    "torque": "0x1A2D",  # Torque Delivered Signal
}
FIELDS = list(IDENT)


def load(path):
    """Rows where every field is present and the engine is running, newest wins per ms."""
    by_ms = collections.defaultdict(dict)
    with gzip.open(path, "rt", encoding="utf-8", errors="replace") as f:
        for row in csv.DictReader(f):
            try:
                t = int(row["ms"])
            except (ValueError, KeyError, TypeError):
                continue
            by_ms[t][row["identifier"]] = row["value"]
    want = set(IDENT.values())
    out = []
    for t in sorted(by_ms):
        r = by_ms[t]
        if not want <= r.keys():
            continue
        try:
            vals = {k: float(r[IDENT[k]]) for k in FIELDS}
        except ValueError:
            continue
        # 0x0049 is APP sensor 1's position (% of its supply), which the emulator's `pedal`
        # key does not take: it wants pedal travel (%). The firmware reads 0 % travel at
        # sensor 1 = 19.6 % and 100 % at 70 % (engine::sensors), so invert that here.
        vals["pedal"] = max(0.0, min(100.0, (vals["pedal"] - 19.6) / (70.0 - 19.6) * 100.0))
        if vals["rpm"] > 500:  # running
            out.append(vals)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("route")
    ap.add_argument("--out")
    ap.add_argument("--rpm-step", type=int, default=250)
    ap.add_argument("--pedal-step", type=int, default=10)
    ap.add_argument("--min", type=int, default=200)
    a = ap.parse_args()

    rows = load(a.route)
    cells = collections.defaultdict(list)
    for v in rows:
        rpm_bin = int(v["rpm"] // a.rpm_step * a.rpm_step)
        pedal_bin = int(max(0.0, min(100.0, v["pedal"])) // a.pedal_step * a.pedal_step)
        cells[(rpm_bin, pedal_bin)].append(v)

    out = open(a.out, "w", newline="", encoding="utf-8") if a.out else sys.stdout
    w = csv.writer(out)
    w.writerow(FIELDS + ["n"])
    kept = 0
    for (rpm_bin, pedal_bin) in sorted(cells):
        pts = cells[(rpm_bin, pedal_bin)]
        if len(pts) < a.min:
            continue
        med = [round(statistics.median(p[k] for p in pts), 3) for k in FIELDS]
        w.writerow(med + [len(pts)])
        kept += 1
    if a.out:
        out.close()
    print(f"{kept} cells from {len(rows)} running samples", file=sys.stderr)


if __name__ == "__main__":
    main()
