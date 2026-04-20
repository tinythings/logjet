use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use liblogjet::export::{LJX_EXPORTER_ABI_MAJOR, LJX_EXPORTER_ABI_MINOR, LjxAbiString, LjxExporterDescriptorV1};

use crate::exporter::{ExporterRegistry, validate_descriptor};

#[test]
fn descriptor_rejects_too_small_struct_size() {
    let mut descriptor = descriptor("parquet");
    descriptor.struct_size = 1;
    let err = validate_descriptor(Path::new("fake.so"), &descriptor).expect_err("must reject undersized descriptor");
    assert!(err.contains("descriptor size"));
}

#[test]
fn descriptor_rejects_abi_major_mismatch() {
    let mut descriptor = descriptor("parquet");
    descriptor.abi_major = LJX_EXPORTER_ABI_MAJOR + 1;
    let err = validate_descriptor(Path::new("fake.so"), &descriptor).expect_err("must reject mismatched ABI major");
    assert!(err.contains("uses ABI"));
}

#[test]
fn descriptor_rejects_newer_abi_minor() {
    let mut descriptor = descriptor("parquet");
    descriptor.abi_minor = LJX_EXPORTER_ABI_MINOR + 1;
    let err = validate_descriptor(Path::new("fake.so"), &descriptor).expect_err("must reject newer ABI minor");
    assert!(err.contains("newer ABI minor"));
}

#[test]
fn descriptor_rejects_invalid_format_name() {
    let descriptor = descriptor("Parquet Boom");
    let err = validate_descriptor(Path::new("fake.so"), &descriptor).expect_err("must reject invalid format names");
    assert!(err.contains("invalid format name"));
}

#[test]
fn descriptor_accepts_valid_names_and_defaults() {
    let mut descriptor = descriptor("parquet");
    descriptor.display_name = LjxAbiString::from_static("Parquet");
    descriptor.default_extension = LjxAbiString::from_static("parquet");
    let validated = validate_descriptor(Path::new("fake.so"), &descriptor).expect("descriptor validates");
    assert_eq!(validated.format_name, "parquet");
    assert_eq!(validated.display_name, "Parquet");
    assert_eq!(validated.default_extension, "parquet");
}

#[test]
fn registry_discovers_first_plugin_and_ignores_duplicate() -> io::Result<()> {
    let plugin = parquet_plugin_path();
    if !plugin.is_file() {
        return Err(io::Error::other(format!("missing test plugin {}. build it first with: cargo build -p ljx-parquet-exporter", plugin.display())));
    }

    let dir = TempDir::new("exporter-discover")?;
    let first = dir.path.join(shared_library_name("aaa_parquet"));
    let second = dir.path.join(shared_library_name("zzz_parquet"));
    let broken = dir.path.join(shared_library_name("broken"));
    fs::copy(&plugin, &first)?;
    fs::copy(&plugin, &second)?;
    fs::write(&broken, b"not a shared library")?;

    let registry = ExporterRegistry::discover_from_roots(vec![dir.path.clone()], Vec::new());
    assert!(registry.plugin("parquet").is_some());
    assert!(registry.available_formats().iter().any(|name| name == "ndjson"));
    assert!(registry.available_formats().iter().any(|name| name == "parquet"));
    assert!(registry.diagnostics.iter().any(|entry| entry.contains("already provided")));
    assert!(registry.diagnostics.iter().any(|entry| entry.contains("dlopen") || entry.contains("symbol")));
    Ok(())
}

fn descriptor(format_name: &'static str) -> LjxExporterDescriptorV1 {
    let mut descriptor = LjxExporterDescriptorV1::header();
    descriptor.abi_major = LJX_EXPORTER_ABI_MAJOR;
    descriptor.abi_minor = LJX_EXPORTER_ABI_MINOR;
    descriptor.format_name = LjxAbiString::from_static(format_name);
    descriptor
}

fn parquet_plugin_path() -> PathBuf {
    target_dir().join("debug").join(shared_library_name("ljx_parquet_exporter"))
}

fn shared_library_name(stem: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{stem}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{stem}.dylib")
    } else {
        format!("lib{stem}.so")
    }
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("target"))
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> io::Result<Self> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("logjet-{label}-{nanos}-{}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
