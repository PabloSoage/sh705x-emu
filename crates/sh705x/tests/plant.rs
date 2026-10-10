//! `sh705x plant`: parsing, and the idle governor closing through the simulated engine.
//!
//! Set SH705X_FLASH to an Astra J (A17DTJ) flash dump to run the second test. Dumps never
//! enter this repository. It runs without the EEPROM and stops 4.5 s after power-up on
//! purpose: the throttle has no plant yet, and once the firmware finds that out (P060E
//! at about 5.4 s without the EEPROM, P2103 sooner with the car's) it raises the idle
//! to 1000 rpm, a limp mode (resultados fase 8).

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

/// The numbers of the summary line: rpm, idle target, torque, fuel.
fn summary(out: &str) -> (f64, f64, f64, f64) {
    let line = out
        .lines()
        .find(|l| l.starts_with("Last quarter"))
        .expect("summary");
    let nums: Vec<f64> = line
        .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .filter_map(|w| w.parse().ok())
        .collect();
    // rpm, from, to, idle target, torque, fuel
    (nums[0], nums[3], nums[4], nums[5])
}

#[test]
fn the_idle_governor_settles_where_the_car_idles() {
    let Ok(flash) = std::env::var("SH705X_FLASH") else {
        eprintln!("SH705X_FLASH not set: skipped");
        return;
    };
    let mut args: Vec<String> = [
        "plant",
        "--flash",
        &flash,
        "--boot",
        "120000000",
        "--millis",
        "3000",
    ]
    .map(String::from)
    .to_vec();
    let out = run(&parse(&args).unwrap()).unwrap();
    let (rpm, target, torque, fuel) = summary(&out);
    // The car at warm idle, standing (routes 67, 69, 72): 750.5 rpm, 12.0 N·m, 7.0 mm³.
    assert!((rpm - 750.0).abs() < 5.0, "{out}");
    assert_eq!(target, 750.0, "{out}");
    assert!((torque - 12.0).abs() < 1.0, "{out}");
    assert!((fuel - 7.0).abs() < 0.5, "{out}");

    // Pedal in neutral: it revs up to the firmware's speed limit and holds there.
    args.extend(["--sensor", "pedal=50"].map(String::from));
    let out = run(&parse(&args).unwrap()).unwrap();
    let (rpm, ..) = summary(&out);
    assert!(rpm > 4000.0 && rpm < 5200.0, "{out}");
}
