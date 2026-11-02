# injection-scenarios

A build-it-yourself tool that drives the emulated ECU **off idle**: it holds the firmware at
a running operating point and reads back what the firmware asks for — torque, fuel, boost
and rail pressure — so you can study those maps across rpm and pedal, compare two
calibrations, and plot a simulated dyno pull.

It is **not built into the released binaries**. The emulator's own runtime stops at idle;
this tool is kept separate and off by default so the release stays "the ECU as it is". Build
it yourself to use it.

## Why it is separate

The firmware only computes an off-idle torque/fuel path once it believes the engine is
running. The emulator has no body-control module in front of the ECU, so this tool sets the
engine-state flags a running car would carry — **engine running, fuelling enabled, in gear**
(`runner.rs`, `addrs`) — and then sweeps the operating point. That stands in for a running
engine; it is not a bypass of any vehicle security function, and none is modelled here.

Because only those engine-state flags are held, a few operating points can still hit a
firmware fuel cut and read 0 torque / −10 mm³ (the overrun-cut sentinel): other guards in the
firmware are left to do whatever they do. The tool is for studying the maps where the
firmware fuels, not for producing a cut-free curve at every point.

## Build and run

```
# from the repository root
cargo build --release --manifest-path injection-scenarios/Cargo.toml

BIN=injection-scenarios/target/release/injection-scenarios
$BIN scenario --flash flash.bin --rpm 2500 --pedal 60 --gear 4
$BIN grid     --flash flash.bin --gear 4
$BIN dyno     --flash flash.bin --start 1400 --end 4200 --gear 4
```

`--flash` points at a flash image you own (default `./flash.bin`); no image ships here.
`--compare <other.bin>` adds a second image to `grid` and `dyno` and prints the difference.

## What it reports

- **scenario**: one operating point — driver-demand torque, delivered/net torque, the smoke
  limiter, fuel, boost and rail setpoints, VGT duty, SCV current, EGR position, shaft power.
- **grid**: 2-D tables (torque, fuel, boost, rail) over rpm × pedal.
- **dyno**: a full-throttle pull, torque and power vs rpm.

The figures are what the **firmware computes** given the inputs; they are a study aid, not a
measurement of the car. In particular the simulated power keeps climbing past the engine's
rated ~110 CV at high rpm — the model does not capture the real high-rpm roll-off — so read
the top end as indicative only.
