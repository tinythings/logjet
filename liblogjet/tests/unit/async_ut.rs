use std::ptr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::runtime::Builder;
use tokio::sync::{Semaphore, oneshot};

use super::{
    Backend, HttpClient, HttpEndpoint, HttpPool, Logger, lj_logger, LjLogRecord,
    LJ_BACKPRESSURE_BLOCK, LJ_BACKPRESSURE_DROP, LJ_BACKPRESSURE_UNBOUNDED, DEFAULT_BACKPRESSURE_CAPACITY,
    AsyncEngine, enqueue_async, flush_engine,
    lj_logger_flush, lj_logger_free,
    lj_logger_async_errors, lj_logger_async_dropped, lj_logger_async_inflight,
    lj_logger_log_async, lj_logger_set_backpressure,
};

fn test_runtime() -> tokio::runtime::Runtime {
    Builder::new_multi_thread().enable_time().build().unwrap()
}

fn test_logger() -> Logger {
    let pool = Arc::new(HttpPool {
        endpoint: HttpEndpoint { authority: "127.0.0.1:4318".to_string(), host_header: "127.0.0.1:4318".to_string(), path: "/v1/logs".to_string() },
        idle: std::sync::Mutex::new(Vec::new()),
    });
    Logger {
        backend: Backend::Http(HttpClient { runtime: std::sync::OnceLock::new(), engine: AsyncEngine::new(), pool }),
        service_name: "svc".to_string(),
        timeout: Duration::from_millis(1000),
    }
}

fn test_logger_ptr() -> *mut lj_logger {
    Box::into_raw(Box::new(lj_logger { inner: test_logger() }))
}

fn set_engine_capacity(engine: &AsyncEngine, model: i32, capacity: usize) {
    let mut bp = engine.backpressure.lock().unwrap();
    bp.model = model;
    bp.semaphore = Arc::new(Semaphore::new(capacity));
}

//
// AsyncEngine defaults
//

#[test]
fn engine_defaults_to_drop_model() {
    let engine = AsyncEngine::new();
    let bp = engine.backpressure.lock().unwrap();
    assert_eq!(bp.model, LJ_BACKPRESSURE_DROP);
}

#[test]
fn engine_default_capacity_is_1024() {
    let engine = AsyncEngine::new();
    let bp = engine.backpressure.lock().unwrap();
    assert_eq!(bp.semaphore.available_permits(), DEFAULT_BACKPRESSURE_CAPACITY);
}

#[test]
fn engine_counters_start_at_zero() {
    let engine = AsyncEngine::new();
    assert_eq!(engine.errors.load(Ordering::Relaxed), 0);
    assert_eq!(engine.dropped.load(Ordering::Relaxed), 0);
    assert_eq!(engine.inflight.load(Ordering::SeqCst), 0);
}

//
// enqueue_async: inflight / errors
//

#[test]
fn enqueue_increments_inflight() {
    let runtime = test_runtime();
    let engine = AsyncEngine::new();
    let (tx, rx) = oneshot::channel::<()>();

    enqueue_async(&runtime, &engine, move || async move {
        let _ = rx.await;
        Ok(())
    }).unwrap();

    assert_eq!(engine.inflight.load(Ordering::SeqCst), 1);
    tx.send(()).unwrap();
}

#[test]
fn enqueue_decrements_inflight_on_completion() {
    let runtime = test_runtime();
    let engine = AsyncEngine::new();

    enqueue_async(&runtime, &engine, || async { Ok(()) }).unwrap();
    std::thread::sleep(Duration::from_millis(100));

    assert_eq!(engine.inflight.load(Ordering::SeqCst), 0);
}

#[test]
fn enqueue_counts_task_errors() {
    let runtime = test_runtime();
    let engine = AsyncEngine::new();

    enqueue_async(&runtime, &engine, || async { Err("fail".to_string()) }).unwrap();
    std::thread::sleep(Duration::from_millis(100));

    assert_eq!(engine.errors.load(Ordering::Relaxed), 1);
    assert_eq!(engine.inflight.load(Ordering::SeqCst), 0);
}

//
// Backpressure: DROP
//

#[test]
fn drop_drops_when_semaphore_exhausted() {
    let runtime = test_runtime();
    let engine = AsyncEngine::new();
    set_engine_capacity(&engine, LJ_BACKPRESSURE_DROP, 1);

    let (tx, rx) = oneshot::channel::<()>();

    enqueue_async(&runtime, &engine, move || async move {
        let _ = rx.await;
        Ok(())
    }).unwrap();
    assert_eq!(engine.inflight.load(Ordering::SeqCst), 1);

    let result = enqueue_async(&runtime, &engine, || async { Ok(()) });
    assert!(result.is_ok());
    assert_eq!(engine.dropped.load(Ordering::Relaxed), 1);

    tx.send(()).unwrap();
}

#[test]
fn drop_returns_ok_even_when_record_is_dropped() {
    let runtime = test_runtime();
    let engine = AsyncEngine::new();
    set_engine_capacity(&engine, LJ_BACKPRESSURE_DROP, 0);

    let result = enqueue_async(&runtime, &engine, || async { Ok(()) });
    assert!(result.is_ok());
    assert_eq!(engine.dropped.load(Ordering::Relaxed), 1);
}

//
// Backpressure: BLOCK
//

#[test]
fn block_waits_for_permit_release() {
    let runtime = test_runtime();
    let engine = AsyncEngine::new();
    set_engine_capacity(&engine, LJ_BACKPRESSURE_BLOCK, 1);

    let (tx, rx) = oneshot::channel::<()>();
    let (ready_tx, ready_rx) = oneshot::channel::<()>();

    // First enqueue holds the only permit.
    enqueue_async(&runtime, &engine, move || async move {
        let _ = rx.await;
        Ok(())
    }).unwrap();

    // Second enqueue should block until the permit is released.
    let engine2 = engine.clone();
    let handle = std::thread::spawn(move || {
        let rt = Builder::new_current_thread().enable_time().build().unwrap();
        let _ = ready_tx.send(());
        enqueue_async(&rt, &engine2, || async { Ok(()) })
    });

    // Wait until the thread is ready (blocked on semaphore).
    ready_rx.blocking_recv().unwrap();
    std::thread::sleep(Duration::from_millis(30));

    // Release the first task; the blocked thread should now succeed.
    tx.send(()).unwrap();
    let result = handle.join().unwrap();
    assert!(result.is_ok());
}

//
// Backpressure: UNBOUNDED
//

#[test]
fn unbounded_ignores_capacity_limit() {
    let runtime = test_runtime();
    let engine = AsyncEngine::new();
    set_engine_capacity(&engine, LJ_BACKPRESSURE_UNBOUNDED, 1);

    let (tx1, rx1) = oneshot::channel::<()>();
    let (tx2, rx2) = oneshot::channel::<()>();

    enqueue_async(&runtime, &engine, move || async move {
        let _ = rx1.await;
        Ok(())
    }).unwrap();
    enqueue_async(&runtime, &engine, move || async move {
        let _ = rx2.await;
        Ok(())
    }).unwrap();

    assert_eq!(engine.inflight.load(Ordering::SeqCst), 2);
    assert_eq!(engine.dropped.load(Ordering::Relaxed), 0);

    tx1.send(()).unwrap();
    tx2.send(()).unwrap();
}

//
// flush_engine
//

#[test]
fn flush_engine_returns_true_when_idle() {
    let runtime = test_runtime();
    let engine = AsyncEngine::new();

    assert!(flush_engine(&runtime, &engine, Duration::from_millis(100)));
}

#[test]
fn flush_engine_returns_false_on_timeout() {
    let runtime = test_runtime();
    let engine = AsyncEngine::new();

    let (tx, rx) = oneshot::channel::<()>();
    enqueue_async(&runtime, &engine, move || async move {
        let _ = rx.await;
        Ok(())
    }).unwrap();

    assert!(!flush_engine(&runtime, &engine, Duration::from_millis(10)));

    tx.send(()).unwrap();
}

#[test]
fn flush_engine_wakes_when_inflight_hits_zero() {
    let runtime = test_runtime();
    let engine = AsyncEngine::new();

    let (tx, rx) = oneshot::channel::<()>();
    enqueue_async(&runtime, &engine, move || async move {
        let _ = rx.await;
        Ok(())
    }).unwrap();

    // Drop the sender from another thread after a short delay.
    let engine2 = engine.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        let _ = tx.send(());
    });

    assert!(flush_engine(&runtime, &engine2, Duration::from_millis(5000)));
}

//
// lj_logger_set_backpressure
//

#[test]
fn set_backpressure_null_logger_fails() {
    assert!(!unsafe { lj_logger_set_backpressure(ptr::null_mut(), LJ_BACKPRESSURE_DROP, 128) });
}

#[test]
fn set_backpressure_invalid_model_fails() {
    let logger = test_logger_ptr();
    assert!(!unsafe { lj_logger_set_backpressure(logger, 99, 128) });
    unsafe { lj_logger_free(logger) };
}

#[test]
fn set_backpressure_zero_capacity_bounded_fails() {
    let logger = test_logger_ptr();
    assert!(!unsafe { lj_logger_set_backpressure(logger, LJ_BACKPRESSURE_DROP, 0) });
    assert!(!unsafe { lj_logger_set_backpressure(logger, LJ_BACKPRESSURE_BLOCK, 0) });
    unsafe { lj_logger_free(logger) };
}

#[test]
fn set_backpressure_zero_capacity_unbounded_succeeds() {
    let logger = test_logger_ptr();
    assert!(unsafe { lj_logger_set_backpressure(logger, LJ_BACKPRESSURE_UNBOUNDED, 0) });
    unsafe { lj_logger_free(logger) };
}

//
// FFI null safety
//

#[test]
fn log_async_null_logger_returns_false() {
    let body = std::ffi::CString::new("hi").unwrap();
    let record = LjLogRecord {
        timestamp_unix_ns: 1, severity_number: 9, severity_text: ptr::null(),
        body: body.as_ptr(), attributes: ptr::null(), attributes_len: 0,
        event_name: ptr::null(), service_name: ptr::null(), scope_name: ptr::null(),
        resource_attrs: ptr::null(), resource_attrs_len: 0,
        scope_attrs: ptr::null(), scope_attrs_len: 0,
    };
    assert!(!unsafe { lj_logger_log_async(ptr::null_mut(), &record) });
}

#[test]
fn log_async_null_record_returns_false() {
    let logger = test_logger_ptr();
    assert!(!unsafe { lj_logger_log_async(logger, ptr::null()) });
    unsafe { lj_logger_free(logger) };
}

#[test]
fn flush_null_logger_returns_false() {
    assert!(!unsafe { lj_logger_flush(ptr::null_mut(), 100) });
}

#[test]
fn async_errors_null_logger_returns_zero() {
    assert_eq!(unsafe { lj_logger_async_errors(ptr::null_mut()) }, 0);
}

#[test]
fn async_dropped_null_logger_returns_zero() {
    assert_eq!(unsafe { lj_logger_async_dropped(ptr::null_mut()) }, 0);
}

#[test]
fn async_inflight_null_logger_returns_zero() {
    assert_eq!(unsafe { lj_logger_async_inflight(ptr::null_mut()) }, 0);
}

#[test]
fn free_null_logger_does_not_crash() {
    unsafe { lj_logger_free(ptr::null_mut()) };
}
