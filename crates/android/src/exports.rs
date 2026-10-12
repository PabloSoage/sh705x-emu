//! The JNI bridge to `com.varuna.sh705x.Native`: one [`Live`] at a time, held here. Each
//! export runs inside `with_env`, which catches a panic before it can unwind into the
//! JVM; a JNI error becomes a `RuntimeException` on the Kotlin side.

// `#[unsafe(no_mangle)]` is how the JVM finds each export by name; nothing else here is
// unsafe.
#![allow(unsafe_code)]

use crate::{Live, Order, lock, traces};
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JByteArray, JClass, JDoubleArray, JString};
use jni::refs::Reference;
use jni::sys::{jboolean, jdouble, jint, jstring};
use jni::{EnvUnowned, Outcome};
use session::PlantMode;
use session::dtcs::read_texts;
use session::names::{read_labels, read_profiles};
use std::sync::Mutex;

/// The ECU the app is running, if any.
static LIVE: Mutex<Option<Live>> = Mutex::new(None);

/// Sends `o` to the running ECU, if there is one.
fn send(o: Order) {
    if let Some(l) = lock(&LIVE).as_ref() {
        l.send(o);
    }
}

/// Runs `f` with the JNI environment and returns its string to Java (null on a failure).
fn string<'l>(
    env: &mut EnvUnowned<'l>,
    f: impl FnOnce(&mut jni::Env<'l>) -> jni::errors::Result<String>,
) -> jstring {
    let o = env.with_env(|env| -> jni::errors::Result<jstring> {
        let s = f(env)?;
        Ok(env.new_string(s)?.into_raw())
    });
    match o.into_outcome() {
        Outcome::Ok(s) => s,
        _ => std::ptr::null_mut(),
    }
}

/// Runs `f` with the JNI environment, for an export that returns nothing.
fn unit<'l>(
    env: &mut EnvUnowned<'l>,
    f: impl FnOnce(&mut jni::Env<'l>) -> jni::errors::Result<()>,
) {
    env.with_env(f).resolve::<ThrowRuntimeExAndDefault>()
}

/// Powers on an ECU on `flash` and `eeprom` (null for none), replacing the one running:
/// an empty string, or what is wrong with the images.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_open<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    flash: JByteArray<'l>,
    eeprom: JByteArray<'l>,
) -> jstring {
    string(&mut env, |env| {
        let flash = env.convert_byte_array(&flash)?;
        let eeprom = if eeprom.is_null() {
            None
        } else {
            Some(env.convert_byte_array(&eeprom)?)
        };
        let mut live = lock(&LIVE);
        // The one running ends before the next one starts.
        *live = None;
        Ok(match Live::open(&flash, eeprom.as_deref()) {
            Ok(l) => {
                *live = Some(l);
                String::new()
            }
            Err(e) => e,
        })
    })
}

/// Ends the running ECU.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_close<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
) {
    unit(&mut env, |_| {
        *lock(&LIVE) = None;
        Ok(())
    })
}

/// The traces, one per line: name, a tab, unit.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_traces<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
) -> jstring {
    string(&mut env, |_| Ok(traces()))
}

/// [`Live::frame`]; empty with no ECU running.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_frame<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    window_us: jdouble,
    columns: jint,
) -> JDoubleArray<'l> {
    env.with_env(|env| -> jni::errors::Result<JDoubleArray<'l>> {
        let f = lock(&LIVE)
            .as_ref()
            .map(|l| l.frame(window_us, columns.max(1) as usize))
            .unwrap_or_default();
        let a = env.new_double_array(f.len())?;
        a.set_region(env, 0, &f)?;
        Ok(a)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// Why the ECU last stopped other than by being paused, or an empty string.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_stop<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
) -> jstring {
    string(&mut env, |_| {
        Ok(lock(&LIVE)
            .as_ref()
            .and_then(|l| l.status().stop)
            .unwrap_or_default())
    })
}

/// Runs or pauses.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_run<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    run: jboolean,
) {
    unit(&mut env, |_| {
        send(Order::Run(run));
        Ok(())
    })
}

/// ECU time per wall-clock time.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_speed<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    speed: jdouble,
) {
    unit(&mut env, |_| {
        send(Order::Speed(speed));
        Ok(())
    })
}

/// The engine speed, rpm.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_rpm<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    rpm: jdouble,
) {
    unit(&mut env, |_| {
        send(Order::Rpm(rpm));
        Ok(())
    })
}

/// A sensor by key, in physical units.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_sensor<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    key: JString<'l>,
    value: jdouble,
) {
    unit(&mut env, |env| {
        let key = key.try_to_string(env)?;
        send(Order::Sensor(key, value));
        Ok(())
    })
}

/// The plants: 0 off, 1 held, 2 turning on the firmware's torque.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_plants<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    mode: jint,
) {
    unit(&mut env, |_| {
        send(Order::Plants(match mode {
            1 => PlantMode::Held,
            2 => PlantMode::Turning,
            _ => PlantMode::Off,
        }));
        Ok(())
    })
}

/// All sensors and the speed at once: ignition on and still, or idling.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_preset<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    idle: jboolean,
) {
    unit(&mut env, |_| {
        send(Order::Preset(idle));
        Ok(())
    })
}

/// Paused, runs to the next tooth position (6°).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_nextTooth<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
) {
    unit(&mut env, |_| {
        send(Order::NextTooth);
        Ok(())
    })
}

/// Paused, runs to the next edge on the input pins.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_nextEdge<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
) {
    unit(&mut env, |_| {
        send(Order::NextEdge);
        Ok(())
    })
}

/// Traces the maps the firmware reads, or stops.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_traceMaps<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    on: jboolean,
) {
    unit(&mut env, |_| {
        send(Order::TraceMaps(on));
        Ok(())
    })
}

/// Stops at the next lookup of the map `desc`, or no longer (negative).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_breakOnMap<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    desc: jint,
) {
    unit(&mut env, |_| {
        send(Order::BreakOnMap((desc >= 0).then_some(desc as u32)));
        Ok(())
    })
}

/// How the map trace stands, in one line; empty while the maps are not traced.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_mapsStatus<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
) -> jstring {
    string(&mut env, |_| {
        Ok(lock(&LIVE)
            .as_ref()
            .map(Live::maps_status)
            .unwrap_or_default())
    })
}

/// [`Live::map_list`].
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_mapList<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    filter: JString<'l>,
) -> jstring {
    string(&mut env, |env| {
        let f = filter.try_to_string(env)?;
        Ok(lock(&LIVE)
            .as_ref()
            .map(|l| l.map_list(&f))
            .unwrap_or_default())
    })
}

/// [`Live::map_data`].
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_mapData<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    desc: jint,
) -> JDoubleArray<'l> {
    env.with_env(|env| -> jni::errors::Result<JDoubleArray<'l>> {
        let d = lock(&LIVE)
            .as_ref()
            .map(|l| l.map_data(desc as u32))
            .unwrap_or_default();
        let a = env.new_double_array(d.len())?;
        a.set_region(env, 0, &d)?;
        Ok(a)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// [`Live::map_text`].
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_mapText<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    desc: jint,
) -> jstring {
    string(&mut env, |_| {
        Ok(lock(&LIVE)
            .as_ref()
            .map(|l| l.map_text(desc as u32))
            .unwrap_or_default())
    })
}

/// [`Live::map_sources`].
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_mapSources<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    desc: jint,
) -> jstring {
    string(&mut env, |_| {
        Ok(lock(&LIVE)
            .as_ref()
            .map(|l| l.map_sources(desc as u32))
            .unwrap_or_default())
    })
}

/// Names the variables the maps read by moving the sensors.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_nameSensors<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
) {
    unit(&mut env, |_| {
        send(Order::NameVariables(None));
        Ok(())
    })
}

/// The profiles in an opendash profile file, one name per line.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_profileNames<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    text: JString<'l>,
) -> jstring {
    string(&mut env, |env| {
        let t = text.try_to_string(env)?;
        Ok(read_profiles(&t)
            .iter()
            .map(|p| p.name.clone())
            .collect::<Vec<_>>()
            .join("\n"))
    })
}

/// Names the variables from profile `index` of an opendash profile file (and the
/// sensors).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_nameFromProfile<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    text: JString<'l>,
    index: jint,
) {
    unit(&mut env, |env| {
        let t = text.try_to_string(env)?;
        if let Some(p) = read_profiles(&t).into_iter().nth(index.max(0) as usize) {
            send(Order::NameVariables(Some(p)));
        }
        Ok(())
    })
}

/// Adds the map labels in a label file (transferred-tags format); returns how many.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_labelMaps<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    text: JString<'l>,
) -> jint {
    env.with_env(|env| -> jni::errors::Result<jint> {
        let l = read_labels(&text.try_to_string(env)?);
        let n = l.len() as jint;
        send(Order::LabelMaps(l));
        Ok(n)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// The fault codes in one line; empty while they are not traced or not known.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_codesStatus<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
) -> jstring {
    string(&mut env, |_| {
        Ok(lock(&LIVE)
            .as_ref()
            .map(Live::codes_status)
            .unwrap_or_default())
    })
}

/// [`Live::code_list`].
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_codeList<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    filter: JString<'l>,
    failing: jboolean,
    disabled: jboolean,
) -> jstring {
    string(&mut env, |env| {
        let f = filter.try_to_string(env)?;
        Ok(lock(&LIVE)
            .as_ref()
            .map(|l| l.code_list(&f, failing, disabled))
            .unwrap_or_default())
    })
}

/// [`Live::code_text`].
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_codeText<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    index: jint,
) -> jstring {
    string(&mut env, |_| {
        Ok(lock(&LIVE)
            .as_ref()
            .map(|l| l.code_text(index.max(0) as u16))
            .unwrap_or_default())
    })
}

/// [`Live::code_maps`].
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_codeMaps<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    index: jint,
) -> jstring {
    string(&mut env, |_| {
        Ok(lock(&LIVE)
            .as_ref()
            .map(|l| l.code_maps(index.max(0) as u16))
            .unwrap_or_default())
    })
}

/// Stops at the next test of fault code `index`, or no longer (negative).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_breakOnTest<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    index: jint,
) {
    unit(&mut env, |_| {
        send(Order::BreakOnTest((index >= 0).then_some(index as u16)));
        Ok(())
    })
}

/// Asks the ECU for its confirmed and pending codes.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_readCodes<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
) {
    unit(&mut env, |_| {
        send(Order::ReadCodes);
        Ok(())
    })
}

/// Adds code descriptions from a file (a code, then its text, per line); returns how many.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_varuna_sh705x_Native_describeCodes<'l>(
    mut env: EnvUnowned<'l>,
    _class: JClass<'l>,
    text: JString<'l>,
) -> jint {
    env.with_env(|env| -> jni::errors::Result<jint> {
        let t = read_texts(&text.try_to_string(env)?);
        let n = t.len() as jint;
        send(Order::DescribeCodes(t));
        Ok(n)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}
