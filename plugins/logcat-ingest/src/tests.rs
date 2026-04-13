use super::*;

#[test]
fn parse_threadtime_info() {
    let p = parse_logcat("06-11 22:14:15.123  1234  5678 I MyApp   : hello world");
    assert_eq!(p.severity, LJ_SEVERITY_INFO);
    assert_eq!(p.pid, "1234");
    assert_eq!(p.tid, "5678");
    assert_eq!(p.tag, "MyApp");
    assert_eq!(p.body, "hello world");
}

#[test]
fn parse_threadtime_error() {
    let p = parse_logcat("06-11 22:14:15.123  1234  5678 E CrashHandler: segfault in native code");
    assert_eq!(p.severity, LJ_SEVERITY_ERROR);
    assert_eq!(p.tag, "CrashHandler");
    assert_eq!(p.body, "segfault in native code");
}

#[test]
fn parse_threadtime_fatal() {
    let p = parse_logcat("06-11 22:14:15.123   999   999 F System  : fatal exception");
    assert_eq!(p.severity, LJ_SEVERITY_FATAL);
    assert_eq!(p.tag, "System");
}

#[test]
fn parse_brief_format() {
    let p = parse_logcat("I/MyApp(1234): started successfully");
    assert_eq!(p.severity, LJ_SEVERITY_INFO);
    assert_eq!(p.tag, "MyApp");
    assert_eq!(p.pid, "1234");
    assert_eq!(p.body, "started successfully");
}

#[test]
fn parse_brief_warning() {
    let p = parse_logcat("W/dalvikvm( 5678): GC freed 2048 objects");
    assert_eq!(p.severity, LJ_SEVERITY_WARN);
    assert_eq!(p.tag, "dalvikvm");
    assert_eq!(p.pid, "5678");
}

#[test]
fn parse_unknown_format() {
    let p = parse_logcat("just some random text");
    assert_eq!(p.severity, LJ_SEVERITY_INFO);
    assert_eq!(p.body, "just some random text");
}

#[test]
fn level_mapping() {
    assert_eq!(map_logcat_level(b'V').0, LJ_SEVERITY_TRACE);
    assert_eq!(map_logcat_level(b'D').0, LJ_SEVERITY_DEBUG);
    assert_eq!(map_logcat_level(b'I').0, LJ_SEVERITY_INFO);
    assert_eq!(map_logcat_level(b'W').0, LJ_SEVERITY_WARN);
    assert_eq!(map_logcat_level(b'E').0, LJ_SEVERITY_ERROR);
    assert_eq!(map_logcat_level(b'F').0, LJ_SEVERITY_FATAL);
    assert_eq!(map_logcat_level(b'A').0, LJ_SEVERITY_FATAL);
}
