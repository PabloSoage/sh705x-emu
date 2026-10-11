//! `sh705x plant`: parsing, and the idle governor closing through the simulated engine.
//!
//! Set SH705X_FLASH to an Astra J (A17DTJ) flash dump to run the engine tests, and
//! SH705X_EEPROM to its EEPROM for the one that needs it. Dumps never enter this
//! repository. The others run without the EEPROM: with the car's, the firmware checks the
//! immobiliser and raises the idle (resultados fase 9).

use sh705x::{Command, parse, run};

#[test]
fn plant_options() {
    let args: Vec<String> = [
        "plant",
        "--flash",
        "a.bin",
        "--millis",
        "2000",
        "--step-us",
        "500",
        "--load",
        "20.5",
    ]
    .map(String::from)
    .to_vec();
    match parse(&args).unwrap() {
        Command::Plant(o) => {
            assert_eq!(o.millis, 2000);
            assert_eq!(o.step_us, 500);
            assert_eq!(o.load, 20.5);
            assert_eq!(o.ecu.rpm, Some(750), "starts idling");
        }
        other => panic!("{other:?}"),
    }
    let args: Vec<String> = ["plant", "--flash", "a.bin"].map(String::from).to_vec();
    match parse(&args).unwrap() {
        Command::Plant(o) => assert_eq!(o.load, 12.0, "the warm idling car's load"),
        other => panic!("{other:?}"),
    }
    for bad in [["--step-us", "0"], ["--load", "x"]] {
        let mut args: Vec<String> = ["plant", "--flash", "a.bin"].map(String::from).to_vec();
        args.extend(bad.map(String::from));
        assert!(parse(&args).is_err(), "{bad:?}");
    }
}

/// The numbers of the summary line: rpm, idle target, torque, fuel, EGR, fresh air.
fn summary(out: &str) -> [f64; 6] {
    let line = out
        .lines()
        .find(|l| l.starts_with("Last quarter"))
        .expect("summary");
    let nums: Vec<f64> = line
        .replace("mm3", "mm")
        .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .filter_map(|w| w.trim_end_matches('.').parse().ok())
        .collect();
    // rpm, from, to, idle target, torque, fuel, EGR, fresh air
    [nums[0], nums[3], nums[4], nums[5], nums[6], nums[7]]
}

#[test]
fn the_engine_idles_where_the_car_does() {
    let Ok(flash) = std::env::var("SH705X_FLASH") else {
        eprintln!("SH705X_FLASH not set: skipped");
        return;
    };
    let args: Vec<String> = ["plant", "--flash", &flash, "--millis", "10000"]
        .map(String::from)
        .to_vec();
    let out = run(&parse(&args).unwrap()).unwrap();
    let [rpm, target, torque, fuel, egr, maf] = summary(&out);
    // The car at warm idle, standing (routes 67, 69, 72): 750.5 rpm, 12.0 N·m, 7.0 mm³,
    // the EGR a third open (median 34 %) and 5.2 g/s of fresh air. Ten seconds and no
    // limp mode: the board's status chips pass the firmware's output checks.
    assert!((rpm - 750.0).abs() < 5.0, "{out}");
    assert_eq!(target, 750.0, "{out}");
    assert!((torque - 12.0).abs() < 1.0, "{out}");
    assert!((fuel - 7.0).abs() < 0.5, "{out}");
    assert!((25.0..45.0).contains(&egr), "{out}");
    assert!((4.5..6.0).contains(&maf), "{out}");
}

#[test]
fn with_the_cars_eeprom_the_immobiliser_check_raises_the_idle() {
    let (Ok(flash), Ok(eeprom)) = (
        std::env::var("SH705X_FLASH"),
        std::env::var("SH705X_EEPROM"),
    ) else {
        eprintln!("SH705X_FLASH or SH705X_EEPROM not set: skipped");
        return;
    };
    let args: Vec<String> = [
        "plant", "--flash", &flash, "--eeprom", &eeprom, "--millis", "8000",
    ]
    .map(String::from)
    .to_vec();
    let out = run(&parse(&args).unwrap()).unwrap();
    let [rpm, target, ..] = summary(&out);
    // With the car's EEPROM the firmware checks the immobiliser's exchange (P160D,
    // `FFFF187x`), which nothing here answers, and its reaction is a 1000 rpm idle
    // (resultados fase 9). That exchange is not emulated, by the owner's rule.
    assert_eq!(target, 1000.0, "{out}");
    assert!((rpm - 1000.0).abs() < 5.0, "{out}");
}

#[test]
fn in_neutral_the_pedal_revs_it_to_the_limit() {
    let Ok(flash) = std::env::var("SH705X_FLASH") else {
        eprintln!("SH705X_FLASH not set: skipped");
        return;
    };
    let args: Vec<String> = [
        "plant", "--flash", &flash, "--millis", "3000", "--sensor", "pedal=50",
    ]
    .map(String::from)
    .to_vec();
    let out = run(&parse(&args).unwrap()).unwrap();
    let [rpm, ..] = summary(&out);
    assert!(rpm > 4000.0 && rpm < 5200.0, "{out}");
}
