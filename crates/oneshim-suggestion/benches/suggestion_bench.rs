use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use oneshim_core::models::suggestion::{Priority, Suggestion, SuggestionSource, SuggestionType};
use oneshim_suggestion::queue::SuggestionQueue;
use oneshim_suggestion::scorer::FeedbackScorer;
use std::hint::black_box;

fn make_suggestion(id: &str, priority: Priority, content: &str) -> Suggestion {
    Suggestion {
        suggestion_id: id.to_string(),
        suggestion_type: SuggestionType::WorkGuidance,
        content: content.to_string(),
        priority,
        confidence_score: 0.9,
        relevance_score: 0.8,
        is_actionable: true,
        created_at: chrono::Utc::now(),
        expires_at: None,
        source: SuggestionSource::LlmServer,
        reasoning: None,
    }
}

fn bench_queue_push(c: &mut Criterion) {
    let mut group = c.benchmark_group("queue_push");

    for size in [10, 25, 50] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("unique", size), &size, |b, &size| {
            b.iter(|| {
                let mut queue = SuggestionQueue::new(50);
                for i in 0..size {
                    queue.push(make_suggestion(
                        &format!("s{i}"),
                        Priority::Medium,
                        &format!("unique content {i}"),
                    ));
                }
                black_box(queue.len())
            });
        });
    }

    group.finish();
}

fn bench_queue_dedup(c: &mut Criterion) {
    let mut group = c.benchmark_group("queue_dedup");

    for size in [10, 25, 50] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::new("reject_duplicates", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let mut queue = SuggestionQueue::new(50);
                    // Fill with unique items
                    for i in 0..size {
                        queue.push(make_suggestion(
                            &format!("s{i}"),
                            Priority::Medium,
                            &format!("content {i}"),
                        ));
                    }
                    // Try to push duplicates (should be rejected)
                    let mut rejected = 0u32;
                    for i in 0..size {
                        if !queue.push(make_suggestion(
                            &format!("dup{i}"),
                            Priority::High,
                            &format!("content {i}"), // same content
                        )) {
                            rejected += 1;
                        }
                    }
                    black_box(rejected)
                });
            },
        );
    }

    group.finish();
}

fn bench_scorer_predict(c: &mut Criterion) {
    let mut group = c.benchmark_group("feedback_scorer");

    // Pre-train scorer with some data
    let mut scorer = FeedbackScorer::new();
    for _ in 0..20 {
        scorer.record(
            SuggestionType::WorkGuidance,
            SuggestionSource::LlmServer,
            &oneshim_core::models::suggestion::FeedbackType::Rejected,
        );
        scorer.record(
            SuggestionType::EmailDraft,
            SuggestionSource::RuleBased,
            &oneshim_core::models::suggestion::FeedbackType::Accepted,
        );
    }

    group.bench_function("score_lookup", |b| {
        b.iter(|| {
            black_box(scorer.score(&SuggestionType::WorkGuidance, &SuggestionSource::LlmServer))
        });
    });

    group.bench_function("adjust", |b| {
        b.iter(|| {
            let mut relevance = 0.7f64;
            black_box(scorer.adjust(
                &SuggestionType::WorkGuidance,
                &SuggestionSource::LlmServer,
                &mut relevance,
            ))
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_queue_push,
    bench_queue_dedup,
    bench_scorer_predict
);
criterion_main!(benches);
