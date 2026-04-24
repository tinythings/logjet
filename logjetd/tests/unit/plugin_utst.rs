use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{
    ingest_plugin_label, legacy_ingest_plugin_name, legacy_ingest_plugin_name_matches, list_visible_ingest_plugins, normalise_legacy_plugin_stem,
    read_ingest_descriptor, resolve_ingest_plugin, resolve_ingest_plugin_path_from_roots,
};

#[test]
fn ingest_plugin_resolver_finds_bare_filename_in_search_roots() -> io::Result<()> {
    let dir = TempDir::new("ingest-plugin-resolve")?;
    let plugin = dir.path.join("liblj_syslog_ingest.so");
    fs::write(&plugin, b"fake")?;

    let resolved = resolve_ingest_plugin_path_from_roots(Path::new("liblj_syslog_ingest.so"), std::slice::from_ref(&dir.path));
    assert_eq!(resolved, plugin);
    Ok(())
}

#[test]
fn ingest_plugin_resolver_preserves_explicit_relative_path() {
    let path = Path::new("./plugins/liblj_syslog_ingest.so");
    let roots = [PathBuf::from("/usr/lib/logjet/ingestors")];

    assert_eq!(resolve_ingest_plugin_path_from_roots(path, &roots), path);
}

#[test]
fn built_in_logcat_plugin_exposes_descriptor_name_when_built() -> io::Result<()> {
    let plugin = target_dir().join("debug").join(shared_library_name("lj_logcat_ingest"));
    if !plugin.is_file() {
        return Ok(());
    }

    let descriptor = read_ingest_descriptor(&plugin).map_err(io::Error::other)?;
    assert_eq!(descriptor.name, "logcat");
    assert_eq!(descriptor.display_name, "Android logcat");
    assert_eq!(ingest_plugin_label(&plugin), "logcat (Android logcat)");
    Ok(())
}

#[test]
fn ingest_plugin_resolver_selects_by_descriptor_name_from_directory() -> io::Result<()> {
    let plugin = target_dir().join("debug").join(shared_library_name("lj_logcat_ingest"));
    if !plugin.is_file() {
        return Ok(());
    }
    let dir = plugin.parent().expect("plugin has parent");

    let resolved = resolve_ingest_plugin(None, Some(dir), Some("logcat"))?;
    assert_eq!(resolved, plugin);
    Ok(())
}

#[test]
fn legacy_ingest_plugin_filename_stems_match_requested_name() {
    assert_eq!(normalise_legacy_plugin_stem("liblj_logcat_ingest"), "logcat");
    assert_eq!(normalise_legacy_plugin_stem("lj-syslog-ingest"), "syslog");
    assert_eq!(normalise_legacy_plugin_stem("libcustom_ingest"), "custom");
    assert!(legacy_ingest_plugin_name_matches(Path::new("libcustom_ingest.so"), "custom"));
    assert_eq!(legacy_ingest_plugin_name(Path::new("libcustom_ingest.so")).as_deref(), Some("custom"));
    assert_eq!(legacy_ingest_plugin_name(Path::new("libcustom.so")), None);
    assert_eq!(ingest_plugin_label(Path::new("libcustom_ingest.so")), "custom");
}

#[test]
fn visible_ingest_listing_keeps_legacy_ingestors_but_skips_unrelated_libraries() -> io::Result<()> {
    let dir = TempDir::new("visible-ingest-plugins")?;
    fs::write(dir.path.join("libcustom_ingest.so"), b"fake")?;
    fs::write(dir.path.join("libunrelated.so"), b"fake")?;

    let plugins = list_visible_ingest_plugins(Some(&dir.path), None);
    assert!(plugins.iter().any(|plugin| plugin.name == "custom"));
    assert!(!plugins.iter().any(|plugin| plugin.name == "unrelated"));
    Ok(())
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
