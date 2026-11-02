# Your own flash image

The `real-boot` feature boots a real SH7059 flash image from `ecu.bin` in this directory.
No image ships with this repository — supply your own dump of a chip you own:

```
cp /path/to/your-sh7059-dump.bin rp2040/firmware/flash/ecu.bin
cargo build --release --features real-boot
```

`ecu.bin` is git-ignored and must never be committed. Without the feature, the default
build runs only the synthetic SH-2/SH-2E benchmarks and needs no image.
