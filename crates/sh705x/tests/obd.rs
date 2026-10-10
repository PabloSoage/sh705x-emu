//! `sh705x obd` against a real dump: the emulated ECU answers diagnostic requests.
//!
//! Set SH705X_FLASH and SH705X_EEPROM to the paths of an Astra J (A17DTJ) dump to run
//! it; dumps never enter this repository. Nothing car-specific is checked here, only
//! the shape of the answers.

use sh705x::{Command, parse, run};

#[test]
fn the_ecu_answers_obd_and_gmlan_requests() {
    let (Ok(flash), Ok(eeprom)) = (
        std::env::var("SH705X_FLASH"),
        std::env::var("SH705X_EEPROM"),
    ) else {
        eprintln!("SH705X_FLASH or SH705X_EEPROM not set: skipped");
        return;
    };
    let args: Vec<String> = [
        "obd", "--flash", &flash, "--eeprom", &eeprom, "0100", "0902", "ATSH7E0", "1A92",
    ]
    .map(String::from)
    .to_vec();
    let cmd = parse(&args).unwrap();
    assert!(matches!(cmd, Command::Obd(..)));
    let out = run(&cmd).unwrap();
    let lines: Vec<&str> = out.lines().map(str::trim).collect();
    let after = |req: &str| lines[lines.iter().position(|l| *l == req).unwrap() + 1];
    assert!(after("0100").starts_with("41 00 "), "{out}");
    // The VIN comes in three frames: 20 bytes, an Opel (W0L) prefix.
    assert_eq!(after("0902"), "014", "{out}");
    assert!(out.contains("0: 49 02 01 57 30 4C"), "{out}");
    // GMLAN 1A 92: the supplier, as GDS2 shows it for this ECU.
    let supplier: String = lines
        .iter()
        .skip_while(|l| **l != "1A92")
        .skip(2)
        .take(2)
        .flat_map(|l| l.split_whitespace().skip(1))
        .map(|h| u8::from_str_radix(h, 16).unwrap() as char)
        .collect();
    assert!(supplier.contains("DENSO"), "{supplier:?} in {out}");
}

#[test]
fn obd_needs_a_command() {
    let args: Vec<String> = ["obd", "--flash", "f.bin"].map(String::from).to_vec();
    assert!(parse(&args).is_err());
    let args: Vec<String> = ["serve", "--flash", "f.bin", "--port", "1234", "--flat-out"]
        .map(String::from)
        .to_vec();
    match parse(&args).unwrap() {
        Command::Serve(o, port, flat) => {
            assert_eq!((port, flat), (1234, true));
            assert_eq!(o.adc, [(14, sh705x::IGNITION_ON)], "ignition on by default");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_ecu_sees_the_engine_turn() {
    let (Ok(flash), Ok(eeprom)) = (
        std::env::var("SH705X_FLASH"),
        std::env::var("SH705X_EEPROM"),
    ) else {
        eprintln!("SH705X_FLASH or SH705X_EEPROM not set: skipped");
        return;
    };
    let args: Vec<String> = [
        "obd", "--flash", &flash, "--eeprom", &eeprom, "--rpm", "2500", "--boot", "80000000",
        "010C", "010C",
    ]
    .map(String::from)
    .to_vec();
    let out = run(&parse(&args).unwrap()).unwrap();
    // The second reading, once settled: 2500 rpm × 4 = 0x2710.
    let last = out.lines().map(str::trim).rfind(|l| l.starts_with("41 0C"));
    assert_eq!(last, Some("41 0C 27 10"), "{out}");
}

#[test]
fn the_ecu_reads_the_sensors_as_set() {
    let (Ok(flash), Ok(eeprom)) = (
        std::env::var("SH705X_FLASH"),
        std::env::var("SH705X_EEPROM"),
    ) else {
        eprintln!("SH705X_FLASH or SH705X_EEPROM not set: skipped");
        return;
    };
    let args: Vec<String> = [
        "obd",
        "--flash",
        &flash,
        "--eeprom",
        &eeprom,
        "--sensor",
        "coolant=90",
        "--sensor",
        "rail=35",
        "--sensor",
        "boost=180",
        "--sensor",
        "battery=13.8",
        "0105",
        "0123",
        "010B",
        "0142",
    ]
    .map(String::from)
    .to_vec();
    let out = run(&parse(&args).unwrap()).unwrap();
    let lines: Vec<&str> = out.lines().map(str::trim).collect();
    let bytes = |req: &str| -> Vec<u32> {
        let l = lines[lines.iter().position(|l| *l == req).unwrap() + 1];
        l.split_whitespace()
            .skip(2)
            .map(|h| u32::from_str_radix(h, 16).unwrap())
            .collect()
    };
    let near = |got: f64, want: f64, tol: f64| {
        assert!((got - want).abs() <= tol, "{got} vs {want}: {out}")
    };
    near(bytes("0105")[0] as f64 - 40.0, 90.0, 1.0);
    let r = bytes("0123");
    near((r[0] * 256 + r[1]) as f64 * 0.01, 35.0, 0.2);
    // The boost sensor is ratiometric: right only with the 5 V references valid.
    near(bytes("010B")[0] as f64, 180.0, 2.0);
    let b = bytes("0142");
    near((b[0] * 256 + b[1]) as f64 / 1000.0, 13.8, 0.05);
}
