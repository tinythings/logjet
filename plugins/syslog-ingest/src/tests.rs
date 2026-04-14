use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

#[test]
fn parse_rfc3164_basic() {
    let p = parse_syslog("<13>Oct 11 22:14:15 myhost su: pam_unix failed");
    assert_eq!(p.severity, LJ_SEVERITY_INFO); // facility=1(user), sev=5(notice)
    assert_eq!(p.facility_text, "user");
    assert_eq!(p.hostname, "myhost");
    assert_eq!(p.app_name, "su");
    assert_eq!(p.body, "pam_unix failed");
}

#[test]
fn parse_rfc3164_with_pid() {
    let p = parse_syslog("<34>Oct 11 22:14:15 box sshd[1234]: accepted key");
    assert_eq!(p.severity, LJ_SEVERITY_FATAL); // facility=4(auth), sev=2(crit)
    assert_eq!(p.facility_text, "auth");
    assert_eq!(p.hostname, "box");
    assert_eq!(p.app_name, "sshd");
    assert_eq!(p.body, "accepted key");
}

#[test]
fn parse_rfc5424_prefix() {
    let p = parse_syslog("<165>1 2023-10-11T22:14:15Z myhost myapp - - - boom");
    assert_eq!(p.severity, LJ_SEVERITY_INFO); // facility=20(local4), sev=5(notice)
    assert_eq!(p.facility_text, "local4");
}

#[test]
fn parse_no_pri() {
    let p = parse_syslog("just some random text");
    assert_eq!(p.severity, LJ_SEVERITY_INFO);
    assert_eq!(p.body, "just some random text");
}

#[test]
fn facility_table_bounds() {
    assert_eq!(facility_name(0), "kern");
    assert_eq!(facility_name(23), "local7");
    assert_eq!(facility_name(24), "unknown");
    assert_eq!(facility_name(999), "unknown");
}

#[test]
fn severity_mapping() {
    assert_eq!(map_syslog_severity(0).0, LJ_SEVERITY_FATAL);
    assert_eq!(map_syslog_severity(3).0, LJ_SEVERITY_ERROR);
    assert_eq!(map_syslog_severity(4).0, LJ_SEVERITY_WARN);
    assert_eq!(map_syslog_severity(6).0, LJ_SEVERITY_INFO);
    assert_eq!(map_syslog_severity(7).0, LJ_SEVERITY_DEBUG);
}

static FEED_COUNT: AtomicU32 = AtomicU32::new(0);
static PARTIAL_COUNT: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn feed_cb(_user: *mut c_void, _record: *const LjLogRecord) {
    FEED_COUNT.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "C" fn partial_cb(_user: *mut c_void, _record: *const LjLogRecord) {
    PARTIAL_COUNT.fetch_add(1, Ordering::Relaxed);
}

#[test]
fn feed_splits_lines() {
    FEED_COUNT.store(0, Ordering::Relaxed);
    let ctx = lj_ingest_create();
    assert!(!ctx.is_null());

    unsafe {
        lj_ingest_set_callback(ctx, feed_cb, std::ptr::null_mut());
        let data = b"<13>Oct 11 22:14:15 h app: line one\n<13>Oct 11 22:14:15 h app: line two\n";
        let rc = lj_ingest_feed(ctx, data.as_ptr(), data.len());
        assert_eq!(rc, 0);
        assert_eq!(FEED_COUNT.load(Ordering::Relaxed), 2);
        lj_ingest_free(ctx);
    }
}

#[test]
fn feed_partial_line() {
    PARTIAL_COUNT.store(0, Ordering::Relaxed);
    let ctx = lj_ingest_create();
    assert!(!ctx.is_null());

    unsafe {
        lj_ingest_set_callback(ctx, partial_cb, std::ptr::null_mut());

        let part1 = b"<13>Oct 11 22:14:15 h app: partial";
        lj_ingest_feed(ctx, part1.as_ptr(), part1.len());
        assert_eq!(PARTIAL_COUNT.load(Ordering::Relaxed), 0);

        let part2 = b" message\n";
        lj_ingest_feed(ctx, part2.as_ptr(), part2.len());
        assert_eq!(PARTIAL_COUNT.load(Ordering::Relaxed), 1);

        lj_ingest_free(ctx);
    }
}
