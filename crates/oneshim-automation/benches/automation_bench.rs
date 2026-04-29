use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use oneshim_automation::audit::AuditLogger;
use oneshim_automation::policy::{AuditLevel, ExecutionPolicy, PolicyClient};

fn fill_logger(n: usize) -> AuditLogger {
    let mut logger = AuditLogger::new(10_000, 50);
    for i in 0..n {
        logger.log_start(&format!("cmd-{}", i), "sess-1", "click");
    }
    logger
}

fn create_policies(n: usize) -> Vec<ExecutionPolicy> {
    (0..n)
        .map(|i| ExecutionPolicy {
            policy_id: format!("policy-{}", i),
            process_name: format!("process-{}", i),
            process_hash: None,
            allowed_args: vec![format!("--config=*"), format!("--output=/tmp/*")],
            requires_sudo: false,
            max_execution_time_ms: 30_000,
            audit_level: AuditLevel::Basic,
            sandbox_profile: None,
            allowed_paths: vec![],
            allow_network: None,
            require_signed_token: false,
            confirmation: Default::default(),
        })
        .collect()
}

fn bench_audit_log_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("audit_log_insert");

    let batch_sizes = [10, 100, 1000];

    for batch_size in batch_sizes {
        group.throughput(Throughput::Elements(batch_size as u64));

        group.bench_with_input(
            BenchmarkId::new("log_start", batch_size),
            &batch_size,
            |b, &n| {
                b.iter_with_setup(
                    || AuditLogger::new(10_000, 50),
                    |mut logger| {
                        for i in 0..n {
                            logger.log_start(&format!("cmd-{}", i), "sess-1", "click");
                        }
                        black_box(logger.pending_count())
                    },
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("log_complete_with_time", batch_size),
            &batch_size,
            |b, &n| {
                b.iter_with_setup(
                    || AuditLogger::new(10_000, 50),
                    |mut logger| {
                        for i in 0..n {
                            logger.log_complete_with_time(
                                AuditLevel::Detailed,
                                &format!("cmd-{}", i),
                                "sess-1",
                                "success",
                                42,
                            );
                        }
                        black_box(logger.pending_count())
                    },
                );
            },
        );
    }

    group.finish();
}

fn bench_audit_drain(c: &mut Criterion) {
    let mut group = c.benchmark_group("audit_drain");

    let buffer_sizes = [100, 500, 1000];

    for buffer_size in buffer_sizes {
        group.throughput(Throughput::Elements(buffer_size as u64));

        group.bench_with_input(
            BenchmarkId::new("drain_batch", buffer_size),
            &buffer_size,
            |b, &n| {
                b.iter_with_setup(
                    || fill_logger(n),
                    |mut logger| {
                        while logger.has_pending_batch() {
                            black_box(logger.drain_batch());
                        }
                    },
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("drain_all", buffer_size),
            &buffer_size,
            |b, &n| {
                b.iter_with_setup(
                    || fill_logger(n),
                    |mut logger| {
                        black_box(logger.drain_all());
                    },
                );
            },
        );
    }

    group.finish();
}

fn bench_audit_query(c: &mut Criterion) {
    let mut group = c.benchmark_group("audit_query");

    let sizes = [100, 500, 1000];

    for size in sizes {
        let logger = fill_logger(size);

        group.bench_with_input(
            BenchmarkId::new("recent_entries_10", size),
            &logger,
            |b, logger| {
                b.iter(|| black_box(logger.recent_entries(10)));
            },
        );

        group.bench_with_input(BenchmarkId::new("stats", size), &logger, |b, logger| {
            b.iter(|| black_box(logger.stats()));
        });
    }

    group.finish();
}

fn bench_policy_validate_args(c: &mut Criterion) {
    let mut group = c.benchmark_group("policy_validate_args");

    let policy = ExecutionPolicy {
        policy_id: "test-policy".to_string(),
        process_name: "test".to_string(),
        process_hash: None,
        allowed_args: vec![
            "--config=*".to_string(),
            "--output=/tmp/*".to_string(),
            "--verbose".to_string(),
            "--log-level=*".to_string(),
        ],
        requires_sudo: false,
        max_execution_time_ms: 30_000,
        audit_level: AuditLevel::Basic,
        sandbox_profile: None,
        allowed_paths: vec![],
        allow_network: None,
        require_signed_token: false,
        confirmation: Default::default(),
    };

    let matching_args = vec![
        "--config=production.yaml".to_string(),
        "--output=/tmp/result.json".to_string(),
    ];

    group.bench_function("matching_args", |b| {
        b.iter(|| black_box(PolicyClient::validate_args(&policy, &matching_args)));
    });

    let non_matching_args = vec![
        "--config=production.yaml".to_string(),
        "--delete-all".to_string(),
    ];

    group.bench_function("non_matching_args", |b| {
        b.iter(|| black_box(PolicyClient::validate_args(&policy, &non_matching_args)));
    });

    let empty_args: Vec<String> = vec![];

    group.bench_function("empty_args", |b| {
        b.iter(|| black_box(PolicyClient::validate_args(&policy, &empty_args)));
    });

    group.finish();
}

fn bench_policy_cache(c: &mut Criterion) {
    let mut group = c.benchmark_group("policy_cache");

    let policy_counts = [10, 50, 100];
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    for count in policy_counts {
        let policies = create_policies(count);
        let client = PolicyClient::new();
        rt.block_on(client.update_policies(policies));

        group.bench_with_input(
            BenchmarkId::new("is_process_allowed", count),
            &client,
            |b, client| {
                b.iter(|| {
                    rt.block_on(async { black_box(client.is_process_allowed("process-5").await) })
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("get_policy_for_process", count),
            &client,
            |b, client| {
                b.iter(|| {
                    rt.block_on(async {
                        black_box(client.get_policy_for_process("process-5").await)
                    })
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("is_cache_valid", count),
            &client,
            |b, client| {
                b.iter(|| rt.block_on(async { black_box(client.is_cache_valid().await) }));
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_audit_log_insert,
    bench_audit_drain,
    bench_audit_query,
    bench_policy_validate_args,
    bench_policy_cache,
);
criterion_main!(benches);
