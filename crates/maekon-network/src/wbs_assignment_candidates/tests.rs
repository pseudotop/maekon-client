//! Synthetic, source-derived assignment-board.v1 controls (#12133).
//! These exercise real local HTTP; they are not live WD backend or native-cell evidence.

use std::sync::Arc;
use std::time::Duration;

use maekon_core::config::TlsConfig;
use maekon_core::models::wbs_assignment_candidates::local::WbsLocalInputRevision;
use maekon_core::models::wbs_assignment_candidates::{WbsAssignmentContext, WbsCandidateState};
use mockito::Matcher;
use serde_json::{json, Value};

use super::*;
use crate::auth::TokenManager;

const PATH: &str = "/api/v1/organizations/org-fixture/assignment-board";
const TOKEN: &str = "synthetic-bearer-12133";

/// Per-run login password. The mocked token endpoint does not read the request
/// body, so any value works; a literal reads to CodeQL as a hard-coded
/// credential (#8685).
fn fixture_password() -> String {
    format!("synthetic-{}", uuid::Uuid::new_v4())
}
const HASH_A: &str = "2eff0883b9c75cb52b6648186f4d6c4562daeb7377defd5431e903f31a92b210";
const HASH_B: &str = "343b2310386d0768aba98eb2d40543783ee71cf31261461cb40d0c359a796468";

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn loopback_tls() -> TlsConfig {
    TlsConfig {
        enabled: false,
        ..TlsConfig::default()
    }
}

fn query(item: &str) -> WbsCandidateQuery {
    WbsCandidateQuery::new(
        WbsAssignmentContext::new("org-fixture".into(), item.into(), Some("v2".into())).unwrap(),
        7,
        2,
    )
    .unwrap()
}

// Hash vectors were produced independently with the server's Python sorted
// compact JSON algorithm, not with validate_candidate_binding under test.
fn fixture(item: &str) -> Value {
    let hash = match item {
        "row-a" => HASH_A,
        "row-b" => HASH_B,
        _ => panic!("unknown synthetic fixture row"),
    };
    let slots: Vec<_> = (0..10)
        .map(|day| {
            json!({
                "slot_id": format!("slot-{day}"), "capacity_hours": 8.0,
                "allocated_hours": 2.0, "remaining_hours": 6.0, "evidence_status": "ready"
            })
        })
        .collect();
    json!({
        "contract_version": "assignment-board.v1", "state": "ready",
        "organization_id": "org-fixture", "synthetic": true,
        "provenance": ["source-derived-synthetic-test"],
        "snapshot": {
            "snapshot_id": "board-fixture", "snapshot_version": "v1", "snapshot_hash": "a".repeat(64),
            "capacity_snapshot_id": "capacity-fixture", "capacity_snapshot_version": "v1",
            "capacity_snapshot_hash": "c".repeat(64), "wbs_version_id": "v2",
            "wbs_content_hash": "b".repeat(64), "capacity_wbs_binding_hash": "d".repeat(64),
            "approval_id": "approval-a"
        },
        "approval": {
            "approval_id": "approval-a", "version_id": "v2", "content_hash": "b".repeat(64),
            "approved_at": "2026-07-01T00:00:00Z", "decision": "approved",
            "decision_summary": "Ignored server presentation detail"
        },
        "wbs_items": [
            {"item_id": "row-a", "skill_id": "skill-be"},
            {"item_id": "row-b", "skill_id": "skill-be"}
        ],
        "candidates": [{
            "candidate_id": format!("wd-candidate-{}", &hash[..24]), "candidate_hash": hash,
            "rank": 1, "user_id": "user-a", "display_name": "Synthetic Candidate",
            "skill_ids": ["skill-be"], "required_skill_id": "skill-be", "skill_match": true,
            "total_remaining_hours": 60.0, "total_allocated_hours": 20.0, "overlap_hours": 20.0,
            "eligible": true, "rank_reason": "skill_match=true;remaining_hours=60.0;overlap_hours=20.0",
            "algorithm_id": "wd-assignment-candidate-ranker", "algorithm_version": "1.0.0",
            "algorithm_as_of": "2026-07-02T00:00:00Z",
            "source_snapshot_id": "board-fixture", "source_snapshot_version": "v1",
            "source_snapshot_hash": "a".repeat(64), "slots": slots
        }],
        "page": {
            "limit": 2, "total_candidates": 1, "returned_candidates": 1,
            "truncated": false, "next_cursor": null
        }
    })
}

fn project(value: Value, item: &str) -> Result<WbsAssignmentCandidates, WbsCandidateError> {
    project_board(serde_json::from_value(value).unwrap(), &query(item))
}

async fn authenticated(server: &mut mockito::ServerGuard, timeout: Duration) -> HttpApiClient {
    let login = server
        .mock("POST", "/api/v1/auth/tokens")
        .with_status(200)
        .with_body(
            json!({"access_token":TOKEN,"refresh_token":"synthetic-refresh","expires_in":3600})
                .to_string(),
        )
        .expect(1)
        .create_async()
        .await;
    let manager = Arc::new(
        TokenManager::new_with_tls(&server.url(), &loopback_tls(), Some(Duration::from_secs(2)))
            .unwrap(),
    );
    manager
        .login("synthetic@example.invalid", &fixture_password())
        .await
        .unwrap();
    login.assert_async().await;
    HttpApiClient::new_with_tls(&server.url(), manager, timeout, &loopback_tls()).unwrap()
}

fn request(server: &mut mockito::ServerGuard, item: &str) -> mockito::Mock {
    server
        .mock("GET", PATH)
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("wbs_item_id".into(), item.into()),
            Matcher::UrlEncoded("limit".into(), "2".into()),
        ]))
        .match_header("authorization", format!("Bearer {TOKEN}").as_str())
        .expect(1)
}

#[tokio::test]
async fn authenticated_get_preserves_distinct_same_skill_task_pins_and_ranker() -> TestResult {
    let mut server = mockito::Server::new_async().await;
    let client = authenticated(&mut server, Duration::from_secs(2)).await;
    let mut identities = Vec::new();
    for item in ["row-a", "row-b"] {
        let response = request(&mut server, item)
            .with_body(fixture(item).to_string())
            .create_async()
            .await;
        let input = query(item);
        let board = client.recommend_assignees(&input).await?;
        assert_eq!(board.state(), WbsCandidateState::Ready);
        assert_eq!(board.query(), &input);
        assert_eq!(board.candidates().len(), 1);
        assert!(board.provenance().synthetic());
        assert_eq!(
            board.provenance().sources(),
            &["source-derived-synthetic-test"]
        );
        let snapshot = board.snapshot().unwrap();
        assert_eq!(snapshot.wbs_version_id(), "v2");
        assert_eq!(snapshot.wbs_content_hash(), "b".repeat(64));
        assert_eq!(snapshot.approval_id(), "approval-a");
        assert_eq!(snapshot.reference().snapshot_hash(), "a".repeat(64));
        let candidate = &board.candidates()[0];
        assert!(candidate.eligible());
        assert_eq!(candidate.identity().user_id(), "user-a");
        assert_eq!(candidate.display_name(), "Synthetic Candidate");
        assert_eq!(
            candidate.algorithm().algorithm_id(),
            "wd-assignment-candidate-ranker"
        );
        assert_eq!(candidate.algorithm().algorithm_version(), "1.0.0");
        identities.push(candidate.identity().candidate_hash().to_string());
        response.assert_async().await;
    }
    assert_eq!(identities, [HASH_A, HASH_B]);
    Ok(())
}

#[test]
fn same_skill_response_cannot_be_attached_to_a_different_requested_row() {
    assert_eq!(
        project(fixture("row-a"), "row-b"),
        Err(WbsCandidateError::Stale)
    );
    assert_eq!(
        project(fixture("row-b"), "row-a"),
        Err(WbsCandidateError::Stale)
    );
}

#[test]
fn contract_state_scope_and_approval_fail_closed_independently() {
    use WbsCandidateError::*;
    let cases = [
        (
            "/contract_version",
            json!("assignment-board.v2"),
            UnsupportedContract,
        ),
        ("/state", json!("empty"), NoCandidates),
        ("/state", json!("unavailable"), Unavailable),
        ("/state", json!("stale"), Stale),
        ("/organization_id", json!("org-other"), Stale),
        ("/snapshot", Value::Null, InvalidResponse),
        ("/approval", Value::Null, InvalidResponse),
        ("/approval/decision", json!("pending"), Stale),
        ("/approval/approval_id", json!("other"), Stale),
        ("/approval/version_id", json!("other"), Stale),
        ("/approval/content_hash", json!("f".repeat(64)), Stale),
        (
            "/snapshot/capacity_snapshot_hash",
            json!("bad"),
            InvalidResponse,
        ),
        (
            "/snapshot/capacity_wbs_binding_hash",
            json!("g".repeat(64)),
            InvalidResponse,
        ),
        ("/snapshot/snapshot_id", json!("other"), InvalidResponse),
        (
            "/snapshot/snapshot_version",
            json!("other"),
            InvalidResponse,
        ),
        (
            "/snapshot/snapshot_hash",
            json!("f".repeat(64)),
            InvalidResponse,
        ),
    ];
    for (pointer, replacement, expected) in cases {
        let mut board = fixture("row-a");
        *board.pointer_mut(pointer).unwrap() = replacement;
        assert_eq!(project(board, "row-a"), Err(expected), "{pointer}");
    }
    let mut board = fixture("row-a");
    board["snapshot"]["wbs_version_id"] = json!("v3");
    board["approval"]["version_id"] = json!("v3");
    assert_eq!(project(board, "row-a"), Err(Stale));
}

#[test]
fn membership_and_pagination_are_bounded_and_never_silently_truncated() {
    for items in [
        json!([]),
        json!([{"item_id":"row-b","skill_id":"skill-be"}]),
    ] {
        let mut board = fixture("row-a");
        board["wbs_items"] = items;
        assert_eq!(project(board, "row-a"), Err(WbsCandidateError::Stale));
    }
    for length in [2, 101] {
        let mut board = fixture("row-a");
        board["wbs_items"] = json!(vec![
            json!({"item_id":"row-a","skill_id":"skill-be"});
            length
        ]);
        assert_eq!(
            project(board, "row-a"),
            Err(WbsCandidateError::InvalidResponse)
        );
    }
    for (field, value) in [
        ("limit", json!(1)),
        ("returned_candidates", json!(0)),
        ("total_candidates", json!(0)),
        ("truncated", json!(true)),
        ("next_cursor", json!("user-other")),
    ] {
        let mut board = fixture("row-a");
        board["page"][field] = value;
        assert_eq!(
            project(board, "row-a"),
            Err(WbsCandidateError::InvalidResponse),
            "{field}"
        );
    }
    let mut truncated = fixture("row-a");
    truncated["page"]["total_candidates"] = json!(3);
    truncated["page"]["truncated"] = json!(true);
    truncated["page"]["next_cursor"] = json!("user-a");
    assert_eq!(project(truncated, "row-a").unwrap().candidates().len(), 1);
    let mut over = fixture("row-a");
    over["candidates"] = json!(vec![over["candidates"][0].clone(); 3]);
    over["page"]["total_candidates"] = json!(3);
    over["page"]["returned_candidates"] = json!(3);
    assert_eq!(
        project(over, "row-a"),
        Err(WbsCandidateError::InvalidResponse)
    );
}

#[test]
fn unsafe_display_text_and_inconsistent_candidate_evidence_are_rejected() {
    for name in [
        "=SUM(A1)",
        "+1",
        "-1",
        "@SUM(A1)",
        "  =SECRET",
        "private\nname",
        "",
    ] {
        let mut board = fixture("row-a");
        board["candidates"][0]["display_name"] = json!(name);
        assert_eq!(
            project(board, "row-a"),
            Err(WbsCandidateError::InvalidResponse),
            "{name:?}"
        );
    }
    for (field, value) in [
        ("eligible", json!(false)),
        ("required_skill_id", json!("skill-other")),
        ("skill_match", json!(false)),
        ("skill_ids", json!([])),
        ("total_remaining_hours", json!(61.0)),
        ("total_allocated_hours", json!(-1.0)),
        ("overlap_hours", json!(19.0)),
        ("rank", json!(0)),
    ] {
        let mut board = fixture("row-a");
        board["candidates"][0][field] = value;
        let expected = if field == "required_skill_id" {
            WbsCandidateError::Stale
        } else {
            WbsCandidateError::InvalidResponse
        };
        assert_eq!(project(board, "row-a"), Err(expected), "{field}");
    }
    for field in [
        "candidate_id",
        "candidate_hash",
        "source_snapshot_id",
        "source_snapshot_version",
        "source_snapshot_hash",
        "user_id",
    ] {
        let mut board = fixture("row-a");
        board["candidates"][0][field] = json!("other");
        assert_eq!(
            project(board, "row-a"),
            Err(WbsCandidateError::Stale),
            "{field}"
        );
    }
}

#[test]
fn ineligible_results_do_not_become_ready_candidates() {
    let mut board = fixture("row-a");
    board["candidates"][0]["skill_ids"] = json!(["skill-other"]);
    board["candidates"][0]["skill_match"] = json!(false);
    board["candidates"][0]["eligible"] = json!(false);
    assert_eq!(
        project(board, "row-a"),
        Err(WbsCandidateError::NoCandidates)
    );
    let mut board = fixture("row-a");
    board["candidates"] = json!([]);
    board["page"]["returned_candidates"] = json!(0);
    board["page"]["total_candidates"] = json!(0);
    assert_eq!(
        project(board, "row-a"),
        Err(WbsCandidateError::NoCandidates)
    );
}

#[test]
fn slot_count_identity_status_and_capacity_must_support_the_candidate_totals() {
    for count in [0, 9, 11] {
        let mut board = fixture("row-a");
        board["candidates"][0]["slots"] =
            json!(vec![board["candidates"][0]["slots"][0].clone(); count]);
        assert_eq!(
            project(board, "row-a"),
            Err(WbsCandidateError::InvalidResponse)
        );
    }
    for (field, value) in [
        ("slot_id", json!("slot-1")),
        ("evidence_status", json!("stale")),
        ("capacity_hours", json!(-1.0)),
        ("allocated_hours", json!(9.0)),
        ("remaining_hours", json!(7.0)),
    ] {
        let mut board = fixture("row-a");
        board["candidates"][0]["slots"][0][field] = value;
        assert_eq!(
            project(board, "row-a"),
            Err(WbsCandidateError::InvalidResponse),
            "{field}"
        );
    }
    let mut board = fixture("row-a");
    board["candidates"]
        .as_array_mut()
        .unwrap()
        .push(fixture("row-a")["candidates"][0].clone());
    board["page"]["total_candidates"] = json!(2);
    board["page"]["returned_candidates"] = json!(2);
    assert_eq!(
        project(board, "row-a"),
        Err(WbsCandidateError::InvalidResponse)
    );
    for non_finite in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
        let source: AssignmentBoardSnapshotDto = serde_json::from_value(fixture("row-a")).unwrap();
        for field in 0..3 {
            let mut candidate = source.candidates[0].clone();
            match field {
                0 => candidate.slots[0].capacity_hours = non_finite,
                1 => candidate.slots[0].allocated_hours = non_finite,
                _ => candidate.slots[0].remaining_hours = non_finite,
            }
            assert_eq!(
                project_candidate(candidate, "skill-be"),
                Err(WbsCandidateError::InvalidResponse)
            );
        }
    }
}

#[tokio::test]
async fn http_denials_are_safe_exact_codes_and_never_retry_or_fall_back() {
    use WbsCandidateError::*;
    for (status, expected) in [
        (401, Unauthorized),
        (403, Unauthorized),
        (409, Stale),
        (422, InvalidInput),
        (429, RateLimited),
        (503, Unavailable),
        (500, Unavailable),
    ] {
        let mut server = mockito::Server::new_async().await;
        let client = authenticated(&mut server, Duration::from_secs(2)).await;
        let response = request(&mut server, "row-a")
            .with_status(status)
            .with_body("PRIVATE-BODY-CANARY /workbook/secret token=PRIVATE")
            .create_async()
            .await;
        let result = client.recommend_assignees(&query("row-a")).await;
        assert_eq!(result, Err(expected));
        assert!(!format!("{result:?}").contains("PRIVATE"));
        response.assert_async().await;
    }
}

#[tokio::test]
async fn redirect_does_not_forward_authorization_or_follow_a_second_endpoint() {
    let mut server = mockito::Server::new_async().await;
    let mut sink = mockito::Server::new_async().await;
    let client = authenticated(&mut server, Duration::from_secs(2)).await;
    let target = sink.mock("GET", "/capture").expect(0).create_async().await;
    let response = request(&mut server, "row-a")
        .with_status(307)
        .with_header("location", &format!("{}/capture", sink.url()))
        .create_async()
        .await;
    assert_eq!(
        client.recommend_assignees(&query("row-a")).await,
        Err(WbsCandidateError::Unavailable)
    );
    response.assert_async().await;
    target.assert_async().await;
}

#[tokio::test]
async fn malformed_and_oversized_bodies_are_bounded_without_exposing_content() {
    for body in [
        "PRIVATE malformed".into(),
        "x".repeat(MAX_BOARD_BYTES as usize + 1),
    ] {
        let mut server = mockito::Server::new_async().await;
        let client = authenticated(&mut server, Duration::from_secs(2)).await;
        let response = request(&mut server, "row-a")
            .with_body(body)
            .create_async()
            .await;
        assert_eq!(
            client.recommend_assignees(&query("row-a")).await,
            Err(WbsCandidateError::InvalidResponse)
        );
        response.assert_async().await;
    }
    let mut server = mockito::Server::new_async().await;
    let client = authenticated(&mut server, Duration::from_secs(2)).await;
    let response = request(&mut server, "row-a")
        .with_chunked_body(|writer| writer.write_all(&vec![b'x'; MAX_BOARD_BYTES as usize + 1]))
        .create_async()
        .await;
    assert_eq!(
        client.recommend_assignees(&query("row-a")).await,
        Err(WbsCandidateError::InvalidResponse)
    );
    response.assert_async().await;
}

#[tokio::test]
async fn timeout_and_transport_failure_remain_unknown_availability_without_replay() {
    let mut server = mockito::Server::new_async().await;
    let client = authenticated(&mut server, Duration::from_millis(30)).await;
    let response = request(&mut server, "row-a")
        .with_chunked_body(|writer| {
            std::thread::sleep(Duration::from_millis(120));
            writer.write_all(b"{}")
        })
        .create_async()
        .await;
    assert_eq!(
        client.recommend_assignees(&query("row-a")).await,
        Err(WbsCandidateError::Timeout)
    );
    response.assert_async().await;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let unavailable = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let manager = Arc::new(
        TokenManager::new_with_tls(&server.url(), &loopback_tls(), Some(Duration::from_secs(2)))
            .unwrap(),
    );
    let login = server
        .mock("POST", "/api/v1/auth/tokens")
        .with_body(
            json!({"access_token":TOKEN,"refresh_token":"synthetic","expires_in":3600}).to_string(),
        )
        .create_async()
        .await;
    manager
        .login("synthetic@example.invalid", &fixture_password())
        .await
        .unwrap();
    login.assert_async().await;
    let client = HttpApiClient::new_with_tls(
        &unavailable,
        manager,
        Duration::from_secs(1),
        &loopback_tls(),
    )
    .unwrap();
    assert_eq!(
        client.recommend_assignees(&query("row-a")).await,
        Err(WbsCandidateError::Unavailable)
    );
}

#[tokio::test]
async fn missing_auth_and_local_document_query_make_zero_candidate_requests() {
    let mut server = mockito::Server::new_async().await;
    let response = server
        .mock("GET", PATH)
        .match_query(Matcher::Any)
        .expect(0)
        .create_async()
        .await;
    let client = HttpApiClient::new_with_tls(
        &server.url(),
        Arc::new(
            TokenManager::new_with_tls(
                &server.url(),
                &loopback_tls(),
                Some(Duration::from_secs(2)),
            )
            .unwrap(),
        ),
        Duration::from_secs(1),
        &loopback_tls(),
    )
    .unwrap();
    assert_eq!(
        client.recommend_assignees(&query("row-a")).await,
        Err(WbsCandidateError::Unauthorized)
    );
    let input = WbsLocalInputRevision::new(
        "document".into(),
        "v1".into(),
        "a".repeat(64),
        "v1".into(),
        "b".repeat(64),
    )
    .unwrap();
    let local = WbsCandidateQuery::new(
        WbsAssignmentContext::new_local(input, "row-a".into()).unwrap(),
        1,
        2,
    )
    .unwrap();
    assert_eq!(
        client.recommend_assignees(&local).await,
        Err(WbsCandidateError::InvalidInput)
    );
    response.assert_async().await;
}

#[tokio::test]
async fn revalidation_uses_another_get_and_rejects_changed_fixed_result() -> TestResult {
    let mut server = mockito::Server::new_async().await;
    let client = authenticated(&mut server, Duration::from_secs(2)).await;
    let first = request(&mut server, "row-a")
        .expect(2)
        .with_body(fixture("row-a").to_string())
        .create_async()
        .await;
    let original = client.recommend_assignees(&query("row-a")).await?;
    client.revalidate_assignees(&original).await?;
    first.assert_async().await;
    first.remove_async().await;
    let mut changed = fixture("row-a");
    changed["candidates"][0]["algorithm_as_of"] = json!("2026-07-03T00:00:00Z");
    let next = request(&mut server, "row-a")
        .with_body(changed.to_string())
        .create_async()
        .await;
    assert_eq!(
        client.revalidate_assignees(&original).await,
        Err(WbsCandidateError::Stale)
    );
    next.assert_async().await;
    assert_eq!(
        original.candidates()[0].algorithm().as_of().to_rfc3339(),
        "2026-07-02T00:00:00+00:00"
    );
    Ok(())
}

#[test]
fn board_item_limit_is_checked_without_duplicate_membership_as_a_second_rejection() {
    for (count, accepted) in [(100, true), (101, false)] {
        let mut board = fixture("row-a");
        let mut items: Vec<_> = (0..count)
            .map(|index| json!({"item_id":format!("other-row-{index}"),"skill_id":"skill-be"}))
            .collect();
        items[0]["item_id"] = json!("row-a");
        board["wbs_items"] = json!(items);
        let result = project(board, "row-a");
        if accepted {
            assert_eq!(result.unwrap().candidates().len(), 1);
        } else {
            assert_eq!(result, Err(WbsCandidateError::InvalidResponse));
        }
    }
}

#[test]
fn exactly_the_requested_candidate_limit_preserves_both_distinct_identities() {
    let mut board = fixture("row-a");
    let hash = "9b800c6c1f2267a51afad6216a3fff129e17d66920c1d154ed3688faa3772016";
    let mut second = board["candidates"][0].clone();
    second["user_id"] = json!("user-b");
    second["candidate_hash"] = json!(hash);
    second["candidate_id"] = json!(format!("wd-candidate-{}", &hash[..24]));
    second["rank"] = json!(2);
    board["candidates"].as_array_mut().unwrap().push(second);
    board["page"]["returned_candidates"] = json!(2);
    board["page"]["total_candidates"] = json!(2);
    let result = project(board, "row-a").unwrap();
    assert_eq!(result.candidates().len(), 2);
    assert_eq!(result.candidates()[0].identity().candidate_hash(), HASH_A);
    assert_eq!(result.candidates()[1].identity().candidate_hash(), hash);
}

#[test]
fn zero_capacity_and_allocations_are_valid_evidence_with_exact_eligibility() {
    let mut available = fixture("row-a");
    for slot in available["candidates"][0]["slots"].as_array_mut().unwrap() {
        slot["allocated_hours"] = json!(0.0);
        slot["remaining_hours"] = json!(8.0);
    }
    available["candidates"][0]["total_allocated_hours"] = json!(0.0);
    available["candidates"][0]["overlap_hours"] = json!(0.0);
    available["candidates"][0]["total_remaining_hours"] = json!(80.0);
    assert_eq!(project(available, "row-a").unwrap().candidates().len(), 1);
    let mut unavailable = fixture("row-a");
    for slot in unavailable["candidates"][0]["slots"]
        .as_array_mut()
        .unwrap()
    {
        slot["capacity_hours"] = json!(0.0);
        slot["allocated_hours"] = json!(0.0);
        slot["remaining_hours"] = json!(0.0);
    }
    for field in [
        "total_remaining_hours",
        "total_allocated_hours",
        "overlap_hours",
    ] {
        unavailable["candidates"][0][field] = json!(0.0);
    }
    unavailable["candidates"][0]["eligible"] = json!(false);
    assert_eq!(
        project(unavailable, "row-a"),
        Err(WbsCandidateError::NoCandidates)
    );
    let mut malformed = fixture("row-a");
    malformed["candidates"][0]["slots"][0]["slot_id"] = json!("");
    assert_eq!(
        project(malformed, "row-a"),
        Err(WbsCandidateError::InvalidResponse)
    );
}

#[test]
fn identifier_hash_and_capacity_tolerance_boundaries_are_exact() {
    for value in ["a", "A._:-09", &"x".repeat(128)] {
        assert!(valid_id(value), "valid identifier boundary");
    }
    for value in ["", " ", "slash/id", "non-ascii-é", &"x".repeat(129)] {
        assert!(!valid_id(value), "invalid identifier boundary");
    }
    assert!(valid_hash(&"a".repeat(64)));
    for value in [&"a".repeat(63), &"a".repeat(65), &"z".repeat(64)] {
        assert!(!valid_hash(value));
    }
    assert!(equal_hours(CAPACITY_TOLERANCE, 0.0));
    assert!(!equal_hours(2.0 * CAPACITY_TOLERANCE, 0.0));
    assert!(!equal_hours(f64::INFINITY, f64::INFINITY));
    assert!(!equal_hours(f64::NAN, 0.0));
}

#[test]
fn returned_count_cannot_lie_while_the_page_fields_are_internally_consistent() {
    let mut board = fixture("row-a");
    board["page"]["returned_candidates"] = json!(2);
    board["page"]["total_candidates"] = json!(2);
    assert_eq!(
        project(board, "row-a"),
        Err(WbsCandidateError::InvalidResponse)
    );
}

#[test]
fn tolerance_does_not_turn_zero_capacity_into_an_eligible_result() {
    let mut board = fixture("row-a");
    for slot in board["candidates"][0]["slots"].as_array_mut().unwrap() {
        slot["allocated_hours"] = json!(8.0);
        slot["remaining_hours"] = json!(0.0);
    }
    board["candidates"][0]["total_allocated_hours"] = json!(80.0);
    board["candidates"][0]["overlap_hours"] = json!(80.0);
    board["candidates"][0]["total_remaining_hours"] = json!(CAPACITY_TOLERANCE);
    board["candidates"][0]["eligible"] = json!(false);
    assert_eq!(
        project(board, "row-a"),
        Err(WbsCandidateError::NoCandidates)
    );
}

#[test]
fn balanced_totals_do_not_make_negative_allocations_valid() {
    let mut board = fixture("row-a");
    for slot in board["candidates"][0]["slots"].as_array_mut().unwrap() {
        slot["allocated_hours"] = json!(-1.0);
        slot["remaining_hours"] = json!(9.0);
    }
    board["candidates"][0]["total_allocated_hours"] = json!(-10.0);
    board["candidates"][0]["overlap_hours"] = json!(-10.0);
    board["candidates"][0]["total_remaining_hours"] = json!(90.0);
    assert_eq!(
        project(board, "row-a"),
        Err(WbsCandidateError::InvalidResponse)
    );
}
