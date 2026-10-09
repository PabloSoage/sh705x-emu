//! `sh705x info` on synthetic images, and on a real dump when one is given.
//!
//! Set SH705X_FLASH (and optionally SH705X_EEPROM) to the path of a dump to run the
//! last test against it. Dumps never enter this repository.

use sh705x::{Command, parse, run};
use std::path::{Path, PathBuf};

fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join("sh705x-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn flash(pc: u32, sp: u32) -> Vec<u8> {
    let mut f = vec![0xFF; sh7059::ROM_SIZE];
    f[0..4].copy_from_slice(&pc.to_be_bytes());
    f[4..8].copy_from_slice(&sp.to_be_bytes());
    f
}

fn info(flash: &Path, eeprom: Option<&Path>) -> Result<String, String> {
    run(&Command::Info {
        flash: flash.to_path_buf(),
        eeprom: eeprom.map(Path::to_path_buf),
    })
}

#[test]
fn parses_the_info_command() {
    let args: Vec<String> = ["info", "--flash", "f.bin", "--eeprom", "e.bin"]
        .map(String::from)
        .to_vec();
    assert_eq!(
        parse(&args).unwrap(),
        Command::Info {
            flash: "f.bin".into(),
            eeprom: Some("e.bin".into())
        }
    );
    assert!(parse(&["info".to_string()]).is_err());
    assert!(parse(&["run".to_string()]).is_err());
    assert_eq!(parse(&[]).unwrap(), Command::Help);
}

#[test]
fn shows_vectors_and_reset_state() {
    let f = temp_file("good.bin", &flash(0x0000_1000, 0xFFFF_BFA0));
    let e = temp_file("good.eep", &[0xFF; 8192]);
    let out = info(&f, Some(&e)).unwrap();
    assert!(
        out.contains("0  power-on reset PC  0x00001000  flash ROM"),
        "{out}"
    );
    assert!(
        out.contains("1  power-on reset SP  0xFFFFBFA0  RAM"),
        "{out}"
    );
    assert!(out.contains("SR 0x000000F0"), "{out}");
    assert!(out.contains("FPSCR 0x00040001"), "{out}");
    assert!(out.contains("8192 bytes, 8192 erased"), "{out}");
    assert!(out.ends_with("check   ok\n"), "{out}");
}

#[test]
fn an_sp_at_the_end_of_ram_is_valid() {
    let f = temp_file("sp-top.bin", &flash(0x0000_1000, 0xFFFF_C000));
    assert!(info(&f, None).is_ok());
}

#[test]
fn flags_vectors_outside_their_regions() {
    let f = temp_file("bad.bin", &flash(0xFFFF_FFFF, 0x0000_0000));
    let err = info(&f, None).unwrap_err();
    assert!(
        err.contains("reset PC 0xFFFFFFFF is not an even flash address"),
        "{err}"
    );
    assert!(
        err.contains("reset SP 0x00000000 does not point into RAM"),
        "{err}"
    );
}

#[test]
fn rejects_wrong_sizes() {
    let f = temp_file("small.bin", &[0; 1024]);
    assert!(info(&f, None).unwrap_err().contains("1.5 MB"));
    let f = temp_file("ok.bin", &flash(0x1000, 0xFFFF_C000));
    let e = temp_file("odd.eep", &[0; 1000]);
    assert!(info(&f, Some(&e)).unwrap_err().contains("power of two"));
}

#[test]
fn a_real_dump_passes_the_checks() {
    let Ok(path) = std::env::var("SH705X_FLASH") else {
        eprintln!("SH705X_FLASH not set: skipped");
        return;
    };
    let eeprom = std::env::var("SH705X_EEPROM").ok().map(PathBuf::from);
    let out = info(Path::new(&path), eeprom.as_deref()).unwrap();
    assert!(out.ends_with("check   ok\n"), "{out}");
}
