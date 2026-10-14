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
    assert!(out.contains("0 of 13 setpoints differ"), "{out}");
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

#[test]
fn replay_options() {
    let args: Vec<String> = ["replay", "--flash", "a.bin", "--cells", "c.csv"]
        .map(String::from)
        .to_vec();
    match parse(&args).unwrap() {
        Command::Replay(o) => {
            assert_eq!(o.cells.to_str(), Some("c.csv"));
            assert_eq!(o.ecu.flash.to_str(), Some("a.bin"));
            assert_eq!(o.ecu.boot, 120_000_000, "longer than obd, to settle");
        }
        other => panic!("{other:?}"),
    }
    // Without --cells it is an error.
    let bad: Vec<String> = ["replay", "--flash", "a.bin"].map(String::from).to_vec();
    assert!(parse(&bad).is_err());
}

#[test]
fn replay_drives_a_dump() {
    let (Ok(flash), Ok(eeprom)) = (
        std::env::var("SH705X_FLASH"),
        std::env::var("SH705X_EEPROM"),
    ) else {
        eprintln!("SH705X_FLASH or SH705X_EEPROM not set: skipped");
        return;
    };
    // Two idle-ish cells, written where the test can read them back.
    let cells = std::env::temp_dir().join(format!("sh705x-replay-{}.csv", std::process::id()));
    std::fs::write(
        &cells,
        "rpm,pedal,coolant,intake,boost,baro,maf,rail,boost_cmd,vgt,air,torque,n\n\
         750,0,62,26,98,99,5.5,24.0,99,58,5.3,15,4000\n\
         1150,0,62,20,104,99,10.6,25.4,114,59,8.6,-15,4600\n",
    )
    .unwrap();
    let args: Vec<String> = [
        "replay",
        "--flash",
        &flash,
        "--eeprom",
        &eeprom,
        "--cells",
        cells.to_str().unwrap(),
    ]
    .map(String::from)
    .to_vec();
    let out = run(&parse(&args).unwrap()).unwrap();
    std::fs::remove_file(&cells).ok();
    assert!(out.contains("Replayed 2 cells"), "{out}");
    // The rail-pressure error line is there and its mean is a sane MPa, not a dash.
    let rail = out.lines().find(|l| l.starts_with("rail (MPa)")).unwrap();
    let mean: f64 = rail.split_whitespace().nth(2).unwrap().parse().unwrap();
    assert!(mean < 30.0, "rail error implausibly large: {out}");
}
