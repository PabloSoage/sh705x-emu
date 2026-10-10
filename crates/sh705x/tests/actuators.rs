//! `sh705x actuators`: parsing, and a short closed-loop run against a real dump.
//!
//! Set SH705X_FLASH to an Astra J (A17DTJ) flash dump to run the second test. Dumps never
//! enter this repository. The test runs without the EEPROM on purpose: with the car's, the
//! firmware also runs its output stage checks, finds the throttle (P2103) and the EGR
//! cooler bypass (P245C) without a plant, and parks the SCV at rest after about three
//! seconds, before its loop can be seen settle (resultados fase 8).

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

#[test]
fn the_two_loops_close_against_the_firmware() {
    let Ok(flash) = std::env::var("SH705X_FLASH") else {
        eprintln!("SH705X_FLASH not set: skipped");
        return;
    };
    let args: Vec<String> = [
        "actuators",
        "--flash",
        &flash,
        "--millis",
        "4000",
        "--step",
        "50",
    ]
    .map(String::from)
    .to_vec();
    let out = run(&parse(&args).unwrap()).unwrap();
    let last = out
        .lines()
        .find(|l| l.starts_with("SCV settled"))
        .expect("summary");
    // The EGR loop closes onto a shut valve at idle, as the car does (fase 5 §6): it
    // settles near 0 % and stops moving.
    let egr = last.split("EGR at ").nth(1).unwrap();
    let egr_pct: f64 = egr.split_whitespace().next().unwrap().parse().unwrap();
    let egr_span: f64 = egr
        .split("spans ")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        egr_pct < 2.0 && egr_span < 1.0,
        "EGR did not settle shut: {last}"
    );
    // The SCV current loop regulates: it holds a steady current well inside the sensor's
    // range (0..3570 mA), not pinned at either rail.
    let scv_ma: f64 = last
        .split("settled at ")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let scv_span: f64 = last
        .split("spans ")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        scv_ma > 500.0 && scv_ma < 3400.0,
        "SCV not regulated: {last}"
    );
    assert!(scv_span < 50.0, "SCV did not settle: {last}");
    // And the firmware was actually driving the duty, not leaving it at a rail.
    let moved = out
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .filter_map(|d| d.parse::<f64>().ok())
        .any(|d| (0.01..0.99).contains(&d));
    assert!(moved, "the SCV duty never left its rail:\n{out}");
}
