#include <liblogjet.h>

#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <dlfcn.h>
#include <iostream>
#include <random>
#include <string>
#include <vector>

namespace {

using version_fn = const char *(*)();
using error_fn = const char *(*)();
using new_http_fn = lj_logger *(*)(const char *, const char *, std::uint64_t);
using new_grpc_fn = lj_logger *(*)(const char *, const char *, std::uint64_t);
using free_fn = void (*)(lj_logger *);
using log_fn = bool (*)(lj_logger *, const lj_log_record *);
using batch_fn = bool (*)(lj_logger *, const lj_log_record *, std::size_t);
using set_bp_fn = bool (*)(lj_logger *, std::int32_t, std::size_t);
using flush_fn = bool (*)(lj_logger *, std::uint64_t);
using counter_fn = std::uint64_t (*)(lj_logger *);

struct api {
    version_fn version;
    error_fn error_message;
    new_http_fn new_http;
    new_grpc_fn new_grpc;
    free_fn free_logger;
    log_fn log_record;
    log_fn log_reuse;
    batch_fn log_batch;
    log_fn log_async;
    set_bp_fn set_backpressure;
    flush_fn flush;
    counter_fn async_errors;
    counter_fn async_dropped;
    counter_fn async_inflight;
};

const std::vector<const char *> kQuotes = {
    "Bender promised a classy fun park financed mostly by blackjack.",
    "Fry pressed the glowing button because hesitation felt off-brand.",
    "Leela requested a routine delivery and got stylish chaos instead.",
    "The Professor called this outage a perfectly normal science moment.",
    "Zoidberg celebrated because nobody had blamed him yet.",
    "Hermes filed the disaster under efficient bureaucratic progress.",
    "Amy said the ship felt stable, which worried everyone instantly.",
    "Nibbler stared into the void like it owed him money.",
    "Scruffy fixed the panel and resumed mopping without commentary.",
    "Calculon demanded better lighting for the emergency landing.",
};

std::string pick_message(std::mt19937 &rng) {
    std::uniform_int_distribution<std::size_t> dist(0, kQuotes.size() - 1);
    return kQuotes[dist(rng)];
}

std::uint64_t unix_time_nanos() {
    auto now = std::chrono::system_clock::now().time_since_epoch();
    return static_cast<std::uint64_t>(std::chrono::duration_cast<std::chrono::nanoseconds>(now).count());
}

void *must_symbol(void *handle, const char *name) {
    dlerror();
    void *symbol = dlsym(handle, name);
    const char *error = dlerror();
    if (error != nullptr) {
        std::cerr << "dlsym failed for " << name << ": " << error << "\n";
        std::exit(1);
    }
    return symbol;
}

api load_api(void *handle) {
    return api{
        reinterpret_cast<version_fn>(must_symbol(handle, "lj_version")),
        reinterpret_cast<error_fn>(must_symbol(handle, "lj_error_message")),
        reinterpret_cast<new_http_fn>(must_symbol(handle, "lj_logger_new_http")),
        reinterpret_cast<new_grpc_fn>(must_symbol(handle, "lj_logger_new_grpc")),
        reinterpret_cast<free_fn>(must_symbol(handle, "lj_logger_free")),
        reinterpret_cast<log_fn>(must_symbol(handle, "lj_logger_log")),
        reinterpret_cast<log_fn>(must_symbol(handle, "lj_logger_log_reuse")),
        reinterpret_cast<batch_fn>(must_symbol(handle, "lj_logger_log_batch")),
        reinterpret_cast<log_fn>(must_symbol(handle, "lj_logger_log_async")),
        reinterpret_cast<set_bp_fn>(must_symbol(handle, "lj_logger_set_backpressure")),
        reinterpret_cast<flush_fn>(must_symbol(handle, "lj_logger_flush")),
        reinterpret_cast<counter_fn>(must_symbol(handle, "lj_logger_async_errors")),
        reinterpret_cast<counter_fn>(must_symbol(handle, "lj_logger_async_dropped")),
        reinterpret_cast<counter_fn>(must_symbol(handle, "lj_logger_async_inflight")),
    };
}

// Sends one record one at a time via the given function (log / reuse / async).
// Safe to use local strings: the library reads them before the call returns
// (async builds its request synchronously, then sends in the background).
void send_one_at_a_time(const api &lib, lj_logger *logger, log_fn send, const char *phase, int count, std::mt19937 &rng) {
    int ok = 0;
    for (int i = 0; i < count; ++i) {
        const std::string message = pick_message(rng);
        const lj_attribute attrs[] = {
            {"appliance.kind", "cpp-demo"},
            {"phase", phase},
        };
        const lj_log_record record{
            unix_time_nanos(), LJ_SEVERITY_INFO, "INFO", message.c_str(), attrs, sizeof(attrs) / sizeof(attrs[0]),
        };
        if (send(logger, &record)) {
            ++ok;
        } else {
            std::cerr << "  " << phase << " send failed: " << lib.error_message() << "\n";
        }
    }
    std::cout << "  [" << phase << "] accepted " << ok << "/" << count << "\n";
}

}  // namespace

int main(int argc, char **argv) {
    const std::string so_path = argc > 1 ? argv[1] : "./liblogjet.so";
    const std::string endpoint = argc > 2 ? argv[2] : "127.0.0.1:4317";
    const int message_count = argc > 3 ? std::atoi(argv[3]) : 25;
    const std::string protocol = argc > 4 ? argv[4] : "grpc";

    void *handle = dlopen(so_path.c_str(), RTLD_NOW | RTLD_LOCAL);
    if (handle == nullptr) {
        std::cerr << "dlopen failed: " << dlerror() << "\n";
        return 1;
    }

    const api lib = load_api(handle);
    std::cout << "loaded liblogjet version " << lib.version() << " (transport: " << protocol << ", endpoint: " << endpoint << ")\n";

    lj_logger *logger = nullptr;
    if (protocol == "http") {
        logger = lib.new_http(endpoint.c_str(), "cpp-appliance", 2000);
    } else {
        logger = lib.new_grpc(endpoint.c_str(), "cpp-appliance", 2000);
    }
    if (logger == nullptr) {
        std::cerr << "logger creation failed: " << lib.error_message() << "\n";
        dlclose(handle);
        return 1;
    }

    std::mt19937 rng(std::random_device{}());
    const int per_phase = std::max(1, message_count / 4);

    // Phase 1: per-connection (a fresh connection per record).
    send_one_at_a_time(lib, logger, lib.log_record, "per-connection", per_phase, rng);

    // Phase 2: reuse (one persistent connection).
    send_one_at_a_time(lib, logger, lib.log_reuse, "reuse", per_phase, rng);

    // Phase 3: batch (one request carrying many records).
    {
        std::vector<std::string> bodies(static_cast<std::size_t>(per_phase));
        const lj_attribute attrs[] = {
            {"appliance.kind", "cpp-demo"},
            {"phase", "batch"},
        };
        std::vector<lj_log_record> records;
        records.reserve(static_cast<std::size_t>(per_phase));
        for (int i = 0; i < per_phase; ++i) {
            bodies[static_cast<std::size_t>(i)] = pick_message(rng);
            records.push_back(lj_log_record{
                unix_time_nanos(), LJ_SEVERITY_INFO, "INFO", bodies[static_cast<std::size_t>(i)].c_str(), attrs, sizeof(attrs) / sizeof(attrs[0]),
            });
        }
        if (lib.log_batch(logger, records.data(), records.size())) {
            std::cout << "  [batch] sent " << records.size() << " records in one request\n";
        } else {
            std::cerr << "  [batch] send failed: " << lib.error_message() << "\n";
        }
    }

    // Phase 4: async (non-blocking; bounded by backpressure, then drained).
    lib.set_backpressure(logger, LJ_BACKPRESSURE_DROP, 256);
    send_one_at_a_time(lib, logger, lib.log_async, "async", per_phase, rng);
    lib.flush(logger, 5000);
    std::cout << "  [async] errors=" << lib.async_errors(logger) << " dropped=" << lib.async_dropped(logger) << " inflight=" << lib.async_inflight(logger) << "\n";

    lib.free_logger(logger);
    dlclose(handle);
    return 0;
}
