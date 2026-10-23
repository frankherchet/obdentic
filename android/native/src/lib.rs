//! JNI handles use a registry so stale Java handles can never dereference freed memory.
use jni::{
    objects::{GlobalRef, JClass, JObject, JString, JValue},
    sys::{jlong, jstring},
    JNIEnv, JavaVM,
};
use obdentic::mobile::{Exchange, MobileCore};
use std::{
    collections::HashMap,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};

struct JavaExchange {
    vm: JavaVM,
    object: GlobalRef,
}
impl Exchange for JavaExchange {
    fn exchange(&mut self, command: &str, timeout: Duration) -> Result<String, String> {
        let mut env = self.vm.attach_current_thread().map_err(|e| e.to_string())?;
        let command = env.new_string(command).map_err(|e| e.to_string())?;
        let result = env.call_method(
            self.object.as_obj(),
            "exchange",
            "(Ljava/lang/String;J)Ljava/lang/String;",
            &[
                JValue::Object(command.as_ref()),
                JValue::Long(timeout.as_millis().min(i64::MAX as u128) as i64),
            ],
        );
        let object = match result {
            Ok(value) => value.l().map_err(|e| e.to_string())?,
            Err(error) => {
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_clear();
                }
                return Err(format!("Android BLE exchange failed: {error}"));
            }
        };
        if object.is_null() {
            return Err("Android BLE returned no response".into());
        }
        env.get_string(&JString::from(object))
            .map(Into::into)
            .map_err(|e| e.to_string())
    }
}
type Core = Arc<Mutex<MobileCore<JavaExchange>>>;
static CORES: OnceLock<Mutex<HashMap<i64, Core>>> = OnceLock::new();
static NEXT: AtomicI64 = AtomicI64::new(1);
fn cores() -> &'static Mutex<HashMap<i64, Core>> {
    CORES.get_or_init(|| Mutex::new(HashMap::new()))
}

#[no_mangle]
pub extern "system" fn Java_de_obdentic_app_NativeCore_create(
    mut env: JNIEnv,
    _class: JClass,
    exchange: JObject,
) -> jlong {
    catch_unwind(AssertUnwindSafe(|| -> Result<i64, String> {
        let core = MobileCore::new(JavaExchange {
            vm: env.get_java_vm().map_err(|e| e.to_string())?,
            object: env.new_global_ref(exchange).map_err(|e| e.to_string())?,
        })?;
        let handle = NEXT.fetch_add(1, Ordering::Relaxed);
        cores()
            .lock()
            .map_err(|_| "native registry poisoned")?
            .insert(handle, Arc::new(Mutex::new(core)));
        Ok(handle)
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or_else(|| {
        let _ = env.throw_new(
            "java/lang/IllegalStateException",
            "Cannot create OBDentic native session",
        );
        0
    })
}

#[no_mangle]
pub extern "system" fn Java_de_obdentic_app_NativeCore_execute(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    action: JString,
    argument: JString,
) -> jstring {
    let response = catch_unwind(AssertUnwindSafe(|| -> Result<String, String> {
        let action: String = env.get_string(&action).map_err(|e| e.to_string())?.into();
        let argument: String = env.get_string(&argument).map_err(|e| e.to_string())?.into();
        let core = cores()
            .lock()
            .map_err(|_| "native registry poisoned")?
            .get(&handle)
            .cloned()
            .ok_or("native session closed")?;
        let response = core
            .lock()
            .map_err(|_| "native session poisoned")?
            .execute(&action, &argument);
        Ok(response)
    }))
    .unwrap_or_else(|_| Err("native diagnostic panic contained; reconnect adapter".into()));
    // JNI-side errors have constant text; escape arbitrary JNI details without JSON injection.
    let text = match response {
        Ok(text) => text,
        Err(_) => {
            "{\"ok\":false,\"error\":\"Native session unavailable; reconnect adapter\"}".into()
        }
    };
    env.new_string(text)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "system" fn Java_de_obdentic_app_NativeCore_destroy(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Ok(mut cores) = cores().lock() {
            cores.remove(&handle);
        }
    }));
}
