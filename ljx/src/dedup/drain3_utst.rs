use crate::dedup::drain3::{Drain, DrainConfig};

/// Port of the Go reference test. Same 24 Kafka log lines, same extra
/// delimiter ("_"), default config. Expects 5 clusters.
#[test]
fn go_reference_kafka_logs() {
    let cfg = DrainConfig { extra_delimiters: vec!["_".into()], ..DrainConfig::default() };
    let mut drain = Drain::new(cfg);

    let logs = [
        "[ProducerStateManager partition=__consumer_offsets-48] Writing producer snapshot at offset 4339939698 (kafka.log.ProducerStateManager)",
        "[Log partition=__consumer_offsets-48, dir=/home1/irteam/apps/data/kafka/kafka-logs] Rolled new log segment at offset 4339939698 in 3 ms. (kafka.log.Log)",
        "[Log partition=__consumer_offsets-48, dir=/home1/irteam/apps/data/kafka/kafka-logs] Deleting segment files LogSegment(baseOffset=0, size=0, lastModifiedTime=1645674584000, largestRecordTimestamp=None) (kafka.log.Log)",
        "Deleted log /home1/irteam/apps/data/kafka/kafka-logs/__consumer_offsets-48/00000000000000000000.log.deleted. (kafka.log.LogSegment)",
        "Deleted offset index /home1/irteam/apps/data/kafka/kafka-logs/__consumer_offsets-48/00000000000000000000.index.deleted. (kafka.log.LogSegment)",
        "Deleted time index /home1/irteam/apps/data/kafka/kafka-logs/__consumer_offsets-48/00000000000000000000.timeindex.deleted. (kafka.log.LogSegment)",
        "[Log partition=__consumer_offsets-48, dir=/home1/irteam/apps/data/kafka/kafka-logs] Deleting segment files LogSegment(baseOffset=2147429227, size=0, lastModifiedTime=1710735195000, largestRecordTimestamp=None) (kafka.log.Log)",
        "Deleted log /home1/irteam/apps/data/kafka/kafka-logs/__consumer_offsets-48/00000000002147429227.log.deleted. (kafka.log.LogSegment)",
        "Deleted offset index /home1/irteam/apps/data/kafka/kafka-logs/__consumer_offsets-48/00000000002147429227.index.deleted. (kafka.log.LogSegment)",
        "Deleted time index /home1/irteam/apps/data/kafka/kafka-logs/__consumer_offsets-48/00000000002147429227.timeindex.deleted. (kafka.log.LogSegment)",
        "[ProducerStateManager partition=__consumer_offsets-49] Writing producer snapshot at offset 4339698 (kafka.log.ProducerStateManager)",
        "[Log partition=__consumer_offsets-48, dir=/home1/irteam/apps/data/kafka/kafka-logs] Deleting segment files LogSegment(baseOffset=4294790577, size=2703, lastModifiedTime=1711832815000, largestRecordTimestamp=Some(1710827112244)) (kafka.log.Log)",
        "[Log partition=__consumer_offsets-48, dir=/home1/irteam/apps/data/kafka/kafka-logs] Deleting segment files LogSegment(baseOffset=4338631022, size=641, lastModifiedTime=1711849197000, largestRecordTimestamp=Some(1711849197921)) (kafka.log.Log)",
        "Deleted log /home1/irteam/apps/data/kafka/kafka-logs/__consumer_offsets-48/00000000004294790577.log.deleted. (kafka.log.LogSegment)",
        "Deleted log /home1/irteam/apps/data/kafka/kafka-logs/__consumer_offsets-48/00000000004338631022.log.deleted. (kafka.log.LogSegment)",
        "Deleted offset index /home1/irteam/apps/data/kafka/kafka-logs/__consumer_offsets-48/00000000004294790577.index.deleted. (kafka.log.LogSegment)",
        "Deleted offset index /home1/irteam/apps/data/kafka/kafka-logs/__consumer_offsets-48/00000000004338631022.index.deleted. (kafka.log.LogSegment)",
        "Deleted time index /home1/irteam/apps/data/kafka/kafka-logs/__consumer_offsets-48/00000000004294790577.timeindex.deleted. (kafka.log.LogSegment)",
        "Deleted time index /home1/irteam/apps/data/kafka/kafka-logs/__consumer_offsets-48/00000000004338631022.timeindex.deleted. (kafka.log.LogSegment)",
        "[Log partition=__consumer_offsets-48, dir=/home1/irteam/apps/data/kafka/kafka-logs] Deleting segment files LogSegment(baseOffset=4339285360, size=104857589, lastModifiedTime=1711865580000, largestRecordTimestamp=Some(1711865580112)) (kafka.log.Log)",
        "Deleted log /home1/irteam/apps/data/kafka/kafka-logs/__consumer_offsets-48/00000000004339285360.log.deleted. (kafka.log.LogSegment)",
        "Deleted offset index /home1/irteam/apps/data/kafka/kafka-logs/__consumer_offsets-48/00000000004339285360.index.deleted. (kafka.log.LogSegment)",
        "Deleted time index /home1/irteam/apps/data/kafka/kafka-logs/__consumer_offsets-48/00000000004339285360.timeindex.deleted. (kafka.log.LogSegment)",
        "[Log partition=__consumer_offsets-49, dir=/home1/irteam/apps/data/kafka/kafka-logs] Rolled new log segment at offset 432939698 in 2 ms. (kafka.log.Log)",
    ];

    for log in &logs {
        drain.add_log_message(log);
    }

    assert_eq!(drain.clusters().len(), 5);
}

#[test]
fn similarity_threshold_respected() {
    // High sim_th (0.9) means very similar messages needed to merge.
    let cfg = DrainConfig { sim_th: 0.9, ..DrainConfig::default() };
    let mut drain = Drain::new(cfg);

    drain.add_log_message("error in module alpha at line 42");
    drain.add_log_message("error in module beta at line 99");
    // Only "alpha"→"beta" and "42"→"99" differ = 3/7 match = 0.43, below 0.9.
    // Actually 5/7 match = 0.71... let me recalculate:
    // "error" "in" "module" differ "at" "line" differ = 5 match, 2 differ = 5/7 ≈ 0.71
    // Still below 0.9 → separate clusters.
    assert_eq!(drain.clusters().len(), 2);
}

#[test]
fn low_threshold_merges_dissimilar() {
    let cfg = DrainConfig { sim_th: 0.3, ..DrainConfig::default() };
    let mut drain = Drain::new(cfg);

    drain.add_log_message("error in module alpha at line 42");
    drain.add_log_message("error in module beta at line 99");
    // 5/7 ≈ 0.71 > 0.3 → should merge.
    assert_eq!(drain.clusters().len(), 1);
}

#[test]
fn different_token_counts_stay_separate() {
    let cfg = DrainConfig::default();
    let mut drain = Drain::new(cfg);

    drain.add_log_message("short message");
    drain.add_log_message("this is a much longer message with more tokens");
    assert_eq!(drain.clusters().len(), 2);
}

#[test]
fn template_has_wildcards() {
    let cfg = DrainConfig { sim_th: 0.4, ..DrainConfig::default() };
    let mut drain = Drain::new(cfg);

    drain.add_log_message("connection from 192.168.1.1 accepted");
    drain.add_log_message("connection from 10.0.0.5 accepted");

    assert_eq!(drain.clusters().len(), 1);
    let cluster = drain.clusters().values().next().unwrap();
    assert_eq!(cluster.template(), "connection from <*> accepted");
}

#[test]
fn empty_input() {
    let cfg = DrainConfig::default();
    let drain = Drain::new(cfg);
    assert_eq!(drain.clusters().len(), 0);
}

#[test]
fn single_message() {
    let cfg = DrainConfig::default();
    let mut drain = Drain::new(cfg);
    let (id, is_new) = drain.add_log_message("hello world");
    assert!(is_new);
    assert_eq!(id, 1);
    assert_eq!(drain.clusters().len(), 1);
}

#[test]
fn exact_duplicate_increments_size() {
    let cfg = DrainConfig::default();
    let mut drain = Drain::new(cfg);

    drain.add_log_message("exactly the same");
    drain.add_log_message("exactly the same");
    drain.add_log_message("exactly the same");

    assert_eq!(drain.clusters().len(), 1);
    let cluster = drain.clusters().values().next().unwrap();
    assert_eq!(cluster.size, 3);
}

#[test]
fn numeric_first_token_uses_wildcard_branch() {
    let cfg = DrainConfig::default();
    let mut drain = Drain::new(cfg);

    drain.add_log_message("req123 error in module route update");
    drain.add_log_message("req456 error in module route update");

    assert_eq!(drain.clusters().len(), 1);
    let cluster = drain.clusters().values().next().unwrap();
    assert_eq!(cluster.template(), "<*> error in module route update");
}

#[test]
fn numeric_middle_token_uses_wildcard_branch() {
    let cfg = DrainConfig::default();
    let mut drain = Drain::new(cfg);

    drain.add_log_message("route 123 failed in module alpha");
    drain.add_log_message("route 456 failed in module alpha");

    assert_eq!(drain.clusters().len(), 1);
    let cluster = drain.clusters().values().next().unwrap();
    assert_eq!(cluster.template(), "route <*> failed in module alpha");
}

#[test]
fn fallback_search_recovers_when_tree_path_misses() {
    let cfg = DrainConfig { sim_th: 0.7, ..DrainConfig::default() };
    let mut drain = Drain::new(cfg);

    drain.add_log_message("alpha error in module route update");
    drain.add_log_message("beta error in module route update");

    assert_eq!(drain.clusters().len(), 1);
    let cluster = drain.clusters().values().next().unwrap();
    assert_eq!(cluster.template(), "<*> error in module route update");
}

#[test]
fn fallback_search_still_merges_when_numeric_parametrization_is_disabled() {
    let cfg = DrainConfig { sim_th: 0.7, parametrize_numeric_tokens: false, ..DrainConfig::default() };
    let mut drain = Drain::new(cfg);

    drain.add_log_message("req123 error in module route update");
    drain.add_log_message("req456 error in module route update");

    assert_eq!(drain.clusters().len(), 1);
    let cluster = drain.clusters().values().next().unwrap();
    assert_eq!(cluster.template(), "<*> error in module route update");
}

#[test]
fn extra_delimiters_split_kv_like_messages() {
    let cfg = DrainConfig { extra_delimiters: vec!["=".into(), ",".into()], ..DrainConfig::default() };
    let mut drain = Drain::new(cfg);

    drain.add_log_message("agentID=117, max_queue_size_seen=10");
    drain.add_log_message("agentID=257, max_queue_size_seen=8");

    assert_eq!(drain.clusters().len(), 1);
    let cluster = drain.clusters().values().next().unwrap();
    assert_eq!(cluster.template(), "agentID <*> max_queue_size_seen <*>");
}
