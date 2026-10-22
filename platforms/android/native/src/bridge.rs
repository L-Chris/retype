//! Opaque numeric handles, never raw Rust pointers supplied by Java.
#![allow(unsafe_code)]
use super::*;
use jni::{
    objects::{JClass, JObject, JObjectArray, JString, JValue},
    sys::{jboolean, jint, jlong, jobject, jobjectArray, jstring},
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
fn strings<'local>(
    env: &mut JNIEnv<'local>,
    values: &[String],
) -> Result<JObjectArray<'local>, String> {
    let array = env
        .new_object_array(values.len() as i32, "java/lang/String", JObject::null())
        .map_err(|e| e.to_string())?;
    for (index, value) in values.iter().enumerate() {
        let string = env.new_string(value).map_err(|e| e.to_string())?;
        env.set_object_array_element(&array, index as i32, &string)
            .map_err(|e| e.to_string())?;
        env.delete_local_ref(string).map_err(|e| e.to_string())?;
    }
    Ok(array)
}
#[no_mangle]
pub extern "system" fn Java_io_github_retype_ime_NativeBridge_dispatchInput(
    mut env: JNIEnv,
    _: JClass,
    id: jlong,
    operation: jint,
    text: JString,
    argument: jint,
    generation: jlong,
    flags: jint,
) -> jobject {
    guard(&mut env, |env| {
        let text: String = env.get_string(&text).map_err(|e| e.to_string())?.into();
        let command =
            Command::from_input(operation, text, argument, generation as u64, flags as u8)?;
        let update = {
            let mut all = sessions().lock().map_err(|e| e.to_string())?;
            all.get_mut(&id)
                .ok_or("closed retype session")?
                .dispatch_compact(command)
        };
        let composition: JObject = env
            .new_string(&update.composition)
            .map_err(|e| e.to_string())?
            .into();
        let candidates: JObject = match update.candidates {
            Some(ref values) => strings(env, values)?.into(),
            None => JObject::null(),
        };
        let commits: JObject = strings(env, &update.commits)?.into();
        Ok(env
            .new_object(
                "io/github/retype/ime/NativeUpdate",
                "(JLjava/lang/String;[Ljava/lang/String;IIIZ[Ljava/lang/String;Z)V",
                &[
                    JValue::Long(update.generation as i64),
                    JValue::Object(&composition),
                    JValue::Object(&candidates),
                    JValue::Int(update.candidate_start as i32),
                    JValue::Int(update.candidate_total as i32),
                    JValue::Int(update.selected as i32),
                    JValue::Bool(u8::from(update.chinese)),
                    JValue::Object(&commits),
                    JValue::Bool(u8::from(update.pass_through)),
                ],
            )
            .map_err(|e| e.to_string())?
            .into_raw())
    })
}
#[no_mangle]
pub extern "system" fn Java_io_github_retype_ime_NativeBridge_candidates(
    mut env: JNIEnv,
    _: JClass,
    id: jlong,
    generation: jlong,
    start: jint,
    limit: jint,
) -> jobjectArray {
    guard(&mut env, |env| {
        let page = {
            let all = sessions().lock().map_err(|e| e.to_string())?;
            let session = all.get(&id).ok_or("closed retype session")?;
            session.candidate_page(generation as u64, start as usize, limit as usize)
        };
        match page {
            Some(ref values) => Ok(strings(env, values)?.into_raw()),
            None => Ok(std::ptr::null_mut()),
        }
    })
}
#[no_mangle]
pub extern "system" fn Java_io_github_retype_ime_NativeBridge_acknowledge(
    mut env: JNIEnv,
    _: JClass,
    id: jlong,
    generation: jlong,
    accepted: jboolean,
) {
    guard(&mut env, |_| {
        let mut all = sessions().lock().map_err(|e| e.to_string())?;
        all.get_mut(&id)
            .ok_or("closed retype session")?
            .acknowledge(generation as u64, accepted != 0);
        Ok(())
    });
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
