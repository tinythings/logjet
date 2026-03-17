#include "../../liblogjet/include/liblogjet.h"

#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <dlfcn.h>
#include <iostream>
#include <random>
#include <string>
#include <thread>
#include <vector>

namespace {

using version_fn = const char *(*)();
using error_fn = const char *(*)();
using new_http_fn = lj_logger *(*)(const char *, const char *, std::uint64_t);
using new_grpc_fn = lj_logger *(*)(const char *, const char *, std::uint64_t);
using free_fn = void (*)(lj_logger *);
using log_fn = bool (*)(lj_logger *, const lj_log_record *);

struct api {
    version_fn version;
    error_fn error_message;
    new_http_fn new_http;
    new_grpc_fn new_grpc;
    free_fn free_logger;
    log_fn log_record;
};

std::int32_t info_severity() {
    return LJ_SEVERITY_INFO;
}

const char *pick(const std::vector<const char *> &values, std::mt19937 &rng) {
    std::uniform_int_distribution<std::size_t> dist(0, values.size() - 1);
    return values[dist(rng)];
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
    };
}

}  // namespace

int main(int argc, char **argv) {
    const std::string so_path = argc > 1 ? argv[1] : "./liblogjet.so";
    const std::string endpoint = argc > 2 ? argv[2] : "127.0.0.1:4317";
    const int message_count = argc > 3 ? std::atoi(argv[3]) : 25;

    void *handle = dlopen(so_path.c_str(), RTLD_NOW | RTLD_LOCAL);
    if (handle == nullptr) {
        std::cerr << "dlopen failed: " << dlerror() << "\n";
        return 1;
    }

    const api lib = load_api(handle);
    std::cout << "loaded liblogjet version " << lib.version() << "\n";

    std::mt19937 rng(std::random_device{}());
    const std::vector<const char *> characters = {
        "Bender", "Fry", "Leela", "Professor Farnsworth", "Zoidberg",
        "Amy", "Hermes", "Nibbler", "Scruffy", "Calculon",
    };
    const std::vector<const char *> locations = {
        "Planet Express", "New New York", "The Moon", "Mars University",
        "Robot Hell", "Slurm factory", "Omicron Persei 8", "Bender's fun park",
    };
    const std::vector<const char *> attractions = {
        "blackjack dome", "dark matter coaster", "hooker-bot lounge",
        "slurm chute", "robot petting zoo", "delivery cannon",
    };
    const std::vector<const char *> moods = {
        "greedy", "heroic", "dramatic", "sleepy",
        "chaotic", "optimistic", "hungry", "unbothered",
    };
    const std::vector<const char *> schemes = {
        "casino expansion", "fun park launch", "delivery detour",
        "robot uprising rehearsal", "slurm promotion", "budget evaporation",
    };
    const std::vector<const char *> quotes = {
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
        "Bender unveiled a premium attraction featuring hooker-bots and bad odds.",
        "The crew found a shortcut through poor planning and dark matter.",
        "Mission control agreed this was still cheaper than preparation.",
        "Someone ordered suspicious robot bees and called it innovation.",
        "The delivery manifest now includes one crate of dramatic overreaction.",
    };

    lj_logger *logger = lib.new_grpc(endpoint.c_str(), "cpp-appliance", 2000);
    if (logger == nullptr) {
        std::cerr << "lj_logger_new_grpc failed: " << lib.error_message() << "\n";
        dlclose(handle);
        return 1;
    }

    for (int index = 1; index <= message_count; ++index) {
        const std::string sequence = std::to_string(index);
        const std::string character = pick(characters, rng);
        const std::string location = pick(locations, rng);
        const std::string attraction = pick(attractions, rng);
        const std::string mood = pick(moods, rng);
        const std::string scheme = pick(schemes, rng);
        const std::string message =
            std::string(pick(quotes, rng)) + " character=" + character + " location=" + location;
        const lj_attribute attributes[] = {
            {"appliance.kind", "cpp-demo"},
            {"appliance.sequence", sequence.c_str()},
            {"character", character.c_str()},
            {"location", location.c_str()},
            {"attraction", attraction.c_str()},
            {"mood", mood.c_str()},
            {"scheme", scheme.c_str()},
        };
        const lj_log_record record{
            unix_time_nanos(),
            info_severity(),
            "INFO",
            message.c_str(),
            attributes,
            sizeof(attributes) / sizeof(attributes[0]),
        };

        if (!lib.log_record(logger, &record)) {
            std::cerr << "lj_logger_log failed: " << lib.error_message() << "\n";
            lib.free_logger(logger);
            dlclose(handle);
            return 1;
        }

        std::cout << "sent: " << message << "\n";
        std::this_thread::sleep_for(std::chrono::milliseconds(20));
    }

    lib.free_logger(logger);
    dlclose(handle);
    return 0;
}
