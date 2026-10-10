//! `sh705x maps`: parsing, and a small grid against a real dump.
//!
//! Set SH705X_FLASH and SH705X_EEPROM to run the second test; dumps never enter this
//! repository.

use sh705x::{Command, parse, run};

#[test]
fn maps_options() {
    let args: Vec<String> = [
        "maps",
        "--flash",
        "a.bin",
        "--compare",
        "b.bin",
        "--rpm-list",
        "1000, 2000",
        "--pedal-list",
        "0,100",
        "--sensor",
        "coolant=40",
    ]
    .map(String::from)
    .to_vec();
    match parse(&args).unwrap() {
        Command::Maps(o) => {
            assert_eq!(o.rpm, [1000, 2000]);
            assert_eq!(o.pedal, [0, 100]);
            assert_eq!(o.compare.unwrap().to_str(), Some("b.bin"));
            assert_eq!(o.ecu.boot, 120_000_000, "longer than obd, to settle");
            assert_eq!(o.ecu.sensor, ["coolant=40"]);
        }
        other => panic!("{other:?}"),
    }
    let bad: Vec<String> = ["maps", "--flash", "a.bin", "--rpm-list", "fast"]
        .map(String::from)
        .to_vec();
    assert!(parse(&bad).is_err());
}

#[test]
fn an_image_against_itself_differs_nowhere() {
    let (Ok(flash), Ok(eeprom)) = (
        std::env::var("SH705X_FLASH"),
        std::env::var("SH705X_EEPROM"),
    ) else {
        eprintln!("SH705X_FLASH or SH705X_EEPROM not set: skipped");
        return;
    };
    let args: Vec<String> = [
        "maps",
        "--flash",
        &flash,
        "--compare",
        &flash,
        "--eeprom",
        &eeprom,
        "--rpm-list",
        "2000",
        "--pedal-list",
        "0,50",
    ]
    .map(String::from)
    .to_vec();
    let out = run(&parse(&args).unwrap()).unwrap();
    assert!(out.contains("0 of 6 setpoints differ"), "{out}");
    // And alone it gives numbers, not dashes: the ECU answered.
    let args: Vec<String> = [
        "maps",
        "--flash",
        &flash,
        "--eeprom",
        &eeprom,
        "--rpm-list",
        "2000",
        "--pedal-list",
        "50",
    ]
    .map(String::from)
    .to_vec();
    let out = run(&parse(&args).unwrap()).unwrap();
    let rail = out
        .lines()
        .skip_while(|l| !l.starts_with("desired rail pressure"))
        .nth(2)
        .unwrap();
    let v: f64 = rail.split_whitespace().nth(1).unwrap().parse().unwrap();
    assert!(v > 30.0 && v < 200.0, "{out}");
}
