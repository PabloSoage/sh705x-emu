//! `sh705x touched`: parsing, and a short run against a real dump.
//!
//! Set SH705X_FLASH and SH705X_EEPROM to an Astra J (A17DTJ) dump to run the second
//! test: its interpolation routines are at 0x7128 (3D) and 0x70B4 (2D). Dumps never
//! enter this repository.

use sh705x::{Command, parse, run};

#[test]
fn touched_options() {
    let args: Vec<String> = [
        "touched",
        "--flash",
        "a.bin",
        "--masters",
        "3d=7128,2D=0x70B4",
        "--catalog",
        "m3.csv",
        "--catalog",
        "m2.csv",
        "--labels",
        "tags.txt",
        "--millis",
        "300",
        "--compare",
        "b.bin",
    ]
    .map(String::from)
    .to_vec();
    match parse(&args).unwrap() {
        Command::Touched(o) => {
            assert_eq!(o.masters, [(0x7128, 3), (0x70B4, 2)]);
            assert_eq!(o.catalog.len(), 2);
            assert_eq!(o.labels.len(), 1);
            assert_eq!(o.millis, 300);
            assert!(o.compare.is_some());
        }
        other => panic!("{other:?}"),
    }
    for bad in [
        vec!["touched", "--flash", "a.bin"],
        vec!["touched", "--flash", "a.bin", "--masters", "4d=10"],
        vec!["touched", "--flash", "a.bin", "--masters", "3d=xyz"],
    ] {
        let args: Vec<String> = bad.into_iter().map(String::from).collect();
        assert!(parse(&args).is_err(), "{args:?}");
    }
}

#[test]
fn the_firmware_looks_up_its_maps() {
    let (Ok(flash), Ok(eeprom)) = (
        std::env::var("SH705X_FLASH"),
        std::env::var("SH705X_EEPROM"),
    ) else {
        eprintln!("SH705X_FLASH or SH705X_EEPROM not set: skipped");
        return;
    };
    let args: Vec<String> = [
        "touched",
        "--flash",
        &flash,
        "--eeprom",
        &eeprom,
        "--rpm",
        "750",
        "--masters",
        "3d=7128,2d=70B4",
        "--millis",
        "200",
    ]
    .map(String::from)
    .to_vec();
    let out = run(&parse(&args).unwrap()).unwrap();
    let first = out.lines().next().unwrap();
    let maps: usize = first
        .split(" maps")
        .next()
        .and_then(|s| s.rsplit(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap();
    assert!(maps > 100, "{first}");
    // Some 3D map is looked up at the engine speed set.
    assert!(
        out.lines()
            .any(|l| l.contains(" 3D ") && l.contains(" 750 / ")),
        "{out}"
    );
}
