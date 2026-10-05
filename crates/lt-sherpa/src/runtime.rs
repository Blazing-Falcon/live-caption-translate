//! Reuse sherpa's shared ONNX Runtime rather than linking a second runtime.

use lt_core::error::{Error, Result};
use std::{
    ffi::CStr,
    path::{Path, PathBuf},
    sync::Mutex,
};

static INITIALIZED: Mutex<Option<PathBuf>> = Mutex::new(None);
// sherpa-onnx 1.13.8's Windows shared C++ library requests this API even though
// its published archive mistakenly contains an ONNX Runtime 1.17.1 DLL.
const SENSEVOICE_RUNTIME_API: u32 = 28;

pub fn default_runtime_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("LT_ONNXRUNTIME") {
        return Ok(path.into());
    }
    let executable = std::env::current_exe()?;
    let directory = executable
        .parent()
        .ok_or_else(|| Error::Engine("Cannot find the application runtime directory".into()))?;
    let name = if cfg!(target_os = "windows") {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    };
    // Cargo tests live in deps/ whereas normal binaries live in the profile
    // directory. The sherpa build script supplies the adjacent shared runtime.
    let adjacent = directory.join(name);
    if adjacent.is_file() {
        return Ok(adjacent);
    }
    if let Some(parent) = directory.parent() {
        let candidate = parent.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(Error::Engine(format!("ONNX Runtime is missing beside the application ({name}); reinstall the shared sherpa runtime or set LT_ONNXRUNTIME")))
}

/// Call before either engine creates a session. Its explicit environment
/// disables telemetry before sherpa creates its own ONNX Runtime environment.
pub fn initialize_shared_runtime(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    require_file(path, "ONNX Runtime library")?;
    let path = path.canonicalize()?;
    let mut initialized = INITIALIZED
        .lock()
        .map_err(|_| Error::Engine("ONNX Runtime initialization was interrupted".into()))?;
    if let Some(existing) = initialized.as_ref() {
        return if existing == &path {
            Ok(())
        } else {
            Err(Error::Engine(
                "A different ONNX Runtime library is already active in this process".into(),
            ))
        };
    }
    let environment = ort::init_from(&path)
        .map_err(|error| engine_error("Cannot load the shared ONNX Runtime", error))?;
    let base = linked_api_base()?;
    // SAFETY: the linked library owns this static API base. An unsupported
    // version returns null and is checked before any API table dereference.
    let linked_api = unsafe { (base.GetApi)(ort::sys::ORT_API_VERSION) };
    require_api(linked_api, ort::sys::ORT_API_VERSION, "Silero VAD")?;
    // A different absolute DLL path may load a second runtime on Windows.
    // Environments/telemetry are shared only when both wrappers use the exact
    // same static API table, not merely the same reported version number.
    require_same_api(linked_api, ort::api(), &path)?;
    if !environment.with_telemetry(false).commit() {
        return Err(Error::Engine("ONNX Runtime was initialized before its offline environment; initialize_shared_runtime must run before any ort session".into()));
    }
    // Commit config is lazy. Materialize the environment before calling into
    // sherpa, whose C++ model constructor also creates an Ort::Env.
    ort::environment::Environment::current().map_err(|error| {
        engine_error("Cannot create the offline ONNX Runtime environment", error)
    })?;
    *initialized = Some(path);
    Ok(())
}

/// Check the linked native runtime before sherpa's C++ constructor can request
/// an unsupported API and dereference its null result. This loads no model.
pub fn verify_sensevoice_runtime() -> Result<()> {
    let base = linked_api_base()?;
    // SAFETY: GetApi is a version negotiation entry point; unsupported versions
    // return null. We never dereference the returned API 28 table here.
    let api = unsafe { (base.GetApi)(SENSEVOICE_RUNTIME_API) };
    require_api(
        api,
        SENSEVOICE_RUNTIME_API,
        "SenseVoice (sherpa-onnx 1.13.8)",
    )
}

fn linked_api_base() -> Result<&'static ort::sys::OrtApiBase> {
    // SAFETY: sherpa's shared feature links ONNX Runtime. This entry point
    // returns a static table and does not create a model or environment.
    let pointer = unsafe { ort::sys::OrtGetApiBase() };
    // SAFETY: the table is static when nonnull; a broken library is rejected.
    unsafe { pointer.as_ref() }
        .ok_or_else(|| Error::Engine("The linked ONNX Runtime returned no API entry point".into()))
}

fn require_api(api: *const ort::sys::OrtApi, required: u32, consumer: &str) -> Result<()> {
    if api.is_null() {
        return Err(Error::Engine(format!(
            "{consumer} requires ONNX Runtime API {required}, but the linked runtime is {}; install the pinned ONNX Runtime 1.30.0 DLL beside the application before starting it",
            onnxruntime_version()
        )));
    }
    Ok(())
}

fn require_same_api(
    linked: *const ort::sys::OrtApi,
    dynamic: *const ort::sys::OrtApi,
    path: &Path,
) -> Result<()> {
    if !std::ptr::eq(linked, dynamic) {
        return Err(Error::Engine(format!(
            "Silero and sherpa loaded different ONNX Runtime libraries; {} must resolve to the same DLL that is beside the application. Replace the adjacent DLL and restart the process",
            path.display()
        )));
    }
    Ok(())
}

pub fn onnxruntime_version() -> String {
    // SAFETY: the C API returns a static, NUL-terminated version string.
    let pointer = unsafe { sherpa_onnx_sys::SherpaOnnxGetOnnxruntimeVersionStr() };
    if pointer.is_null() {
        return "unknown".into();
    }
    // SAFETY: the nonnull pointer above remains valid for the library lifetime.
    unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn require_file(path: &Path, description: &str) -> Result<()> {
    match path.metadata() {
        Ok(metadata) if metadata.is_file() && metadata.len() > 0 => Ok(()),
        _ => Err(Error::Engine(format!(
            "{description} is missing or empty: {}",
            path.display()
        ))),
    }
}

pub(crate) fn engine_error(context: &str, error: impl std::fmt::Display) -> Error {
    Error::Engine(format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_api_is_rejected_without_dereferencing_it() {
        let error = require_api(std::ptr::null(), 28, "SenseVoice").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("requires ONNX Runtime API 28"));
        assert!(message.contains("beside the application"));
    }

    #[test]
    fn api_identity_requires_the_same_table_even_for_identical_versions() {
        let tables = [0_u8, 0_u8];
        let left = std::ptr::from_ref(&tables[0]).cast::<ort::sys::OrtApi>();
        let right = std::ptr::from_ref(&tables[1]).cast::<ort::sys::OrtApi>();
        for (dynamic, compatible) in [(left, true), (right, false)] {
            assert_eq!(
                require_same_api(left, dynamic, Path::new("onnxruntime.dll")).is_ok(),
                compatible
            );
        }
    }
}
