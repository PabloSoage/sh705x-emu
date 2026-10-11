//! `sh705x actuators`: parsing, and a short closed-loop run against a real dump.
//!
//! Set SH705X_FLASH to an Astra J (A17DTJ) flash dump to run the second test, and
//! SH705X_EEPROM to run it with the car's EEPROM as well. Dumps never enter this
//! repository.

use sh705x::{Command, parse, run};

#[test]
fn actuators_options() {
    let args: Vec<String> = [
        "actuators",
        "--flash",
        "a.bin",
        "--millis",
        "2000",
        "--step",
        "25",
    ]
    .map(String::from)
    .to_vec();
    match parse(&args).unwrap() {
        Command::Actuators(o) => {
            assert_eq!(o.millis, 2000);
            assert_eq!(o.step, 25);
            assert_eq!(o.ecu.rpm, Some(750), "defaults to idling");
        }
        other => panic!("{other:?}"),
    }
    // A zero step is refused; a given speed is kept.
    let bad: Vec<String> = ["actuators", "--flash", "a.bin", "--step", "0"]
        .map(String::from)
        .to_vec();
    assert!(parse(&bad).is_err());
    let args: Vec<String> = ["actuators", "--flash", "a.bin", "--rpm", "900"]
        .map(String::from)
        .to_vec();
    match parse(&args).unwrap() {
        Command::Actuators(o) => assert_eq!(o.ecu.rpm, Some(900)),
        other => panic!("{other:?}"),
    }
}

/// SCV current, its setpoint and spread, EGR position and the setpoint asked for, from
/// the summary line.
fn summary(out: &str) -> [f64; 7] {
    let line = out
        .lines()
        .find(|l| l.starts_with("SCV settled"))
        .expect("summary");
    let nums: Vec<f64> = line
        .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .filter_map(|w| w.trim_end_matches('.').parse().ok())
        .collect();
    // scv, span, asked, egr, span, asked, drive
    nums[..7].try_into().unwrap()
}

#[test]
fn the_loops_close_against_the_firmware() {
    let Ok(flash) = std::env::var("SH705X_FLASH") else {
        eprintln!("SH705X_FLASH not set: skipped");
        return;
    };
    // With and without the car's EEPROM: the board's status chips pass the output checks
    // either way (resultados fase 9).
    let eeproms = std::iter::once(None).chain(std::env::var("SH705X_EEPROM").ok().map(Some));
    for eeprom in eeproms {
        let mut args: Vec<String> = ["actuators", "--flash", &flash, "--millis", "4000"]
            .map(String::from)
            .to_vec();
        if let Some(e) = &eeprom {
            args.extend(["--eeprom", e].map(String::from));
        }
        let out = run(&parse(&args).unwrap()).unwrap();
        let [scv, scv_span, scv_asked, egr, _, egr_asked, _] = summary(&out);
        // The SCV current loop holds its own setpoint, steadily.
        assert!((scv - scv_asked).abs() < 30.0, "{out}");
        assert!(scv_span < 30.0, "{out}");
        // With the engine held at 750 rpm the idle governor asks for a lot of fuel, and the
        // firmware all but shuts the EGR (under 10 %, or below zero: off), as the car does
        // under load; the valve follows.
        assert!(egr_asked < 10.0, "{out}");
        assert!((egr - egr_asked.max(0.0)).abs() < 5.0, "{out}");
    }
}
