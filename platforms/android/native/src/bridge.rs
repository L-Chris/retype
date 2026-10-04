//! Opaque numeric handles, never raw Rust pointers supplied by Java.
#![allow(unsafe_code)]
use super::*;
use jni::{
    objects::{JClass, JString},
    sys::{jboolean, jlong, jstring},
    JNIEnv,
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicI64, Ordering},
        Mutex, OnceLock,
    },
};
static SESSIONS: OnceLock<Mutex<HashMap<i64, Session>>> = OnceLock::new();
static NEXT: AtomicI64 = AtomicI64::new(1);
fn sessions() -> &'static Mutex<HashMap<i64, Session>> {
    SESSIONS.get_or_init(Default::default)
}
fn guard<T: Default>(
    env: &mut JNIEnv<'_>,
    f: impl FnOnce(&mut JNIEnv<'_>) -> Result<T, String>,
) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(env))) {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            let _ = env.throw_new("java/lang/IllegalStateException", error);
            T::default()
        }
        Err(_) => {
            let _ = env.throw_new(
                "java/lang/IllegalStateException",
                "retype native session failed",
            );
            T::default()
        }
    }
}
#[no_mangle]
pub extern "system" fn Java_io_github_retype_ime_NativeBridge_create(
    mut env: JNIEnv,
    _: JClass,
    dictionary: JString,
    database: JString,
    flypy: jboolean,
    chinese: jboolean,
    learn: jboolean,
    packs: JString,
) -> jlong {
    guard(&mut env, |env| {
        let dictionary: String = env
            .get_string(&dictionary)
            .map_err(|e| e.to_string())?
            .into();
        let database: String = env.get_string(&database).map_err(|e| e.to_string())?.into();
        let packs: String = env.get_string(&packs).map_err(|e| e.to_string())?.into();
        let packs: Vec<String> = serde_json::from_str(&packs).map_err(|e| e.to_string())?;
        let session = Session::open_with_packs(
            Path::new(&dictionary),
            Path::new(&database),
            flypy != 0,
            chinese != 0,
            learn != 0,
            &packs,
        )?;
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        sessions()
            .lock()
            .map_err(|e| e.to_string())?
            .insert(id, session);
        Ok(id)
    })
}
#[no_mangle]
pub extern "system" fn Java_io_github_retype_ime_NativeBridge_feature(
    mut env: JNIEnv,
    _: JClass,
    operation: JString,
) -> jstring {
    guard(&mut env, |env| {
        let operation: String = env
            .get_string(&operation)
            .map_err(|e| e.to_string())?
            .into();
        let result =
            features::perform(serde_json::from_str(&operation).map_err(|e| e.to_string())?)?;
        Ok(env
            .new_string(result.to_string())
            .map_err(|e| e.to_string())?
            .into_raw())
    })
}
#[no_mangle]
pub extern "system" fn Java_io_github_retype_ime_NativeBridge_dispatch(
    mut env: JNIEnv,
    _: JClass,
    id: jlong,
    command: JString,
) -> jstring {
    guard(&mut env, |env| {
        let command: String = env.get_string(&command).map_err(|e| e.to_string())?.into();
        let command: Command = serde_json::from_str(&command).map_err(|e| e.to_string())?;
        let mut all = sessions().lock().map_err(|e| e.to_string())?;
        let session = all.get_mut(&id).ok_or("closed retype session")?;
        let json = serde_json::to_string(&session.dispatch(command)).map_err(|e| e.to_string())?;
        Ok(env.new_string(json).map_err(|e| e.to_string())?.into_raw())
    })
}
#[no_mangle]
pub extern "system" fn Java_io_github_retype_ime_NativeBridge_destroy(
    mut env: JNIEnv,
    _: JClass,
    id: jlong,
) {
    guard(&mut env, |_| {
        let session = sessions().lock().map_err(|e| e.to_string())?.remove(&id);
        drop(session);
        Ok(())
    });
}
