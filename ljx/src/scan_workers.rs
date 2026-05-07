pub(crate) const MAX_SCAN_WORKERS: usize = 8;

pub(crate) fn default_worker_count() -> usize {
    std::thread::available_parallelism().map(|count| count.get()).unwrap_or(1).clamp(1, MAX_SCAN_WORKERS)
}
