//! Integration tests for `forge_core::history`.

use std::time::Duration;

use chrono::Utc;
use forge_core::exec::{ExecutionResult, Sizes, TimingBreakdown};
use forge_core::history::{
    diff_entries, diff_text, HistoryFilter, HistoryRecord, HistoryStore, NewEntry,
};

fn exec_result(status: u16, body: &[u8], headers: Vec<(&str, &str)>) -> ExecutionResult {
    ExecutionResult {
        status,
        status_text: "OK".to_string(),
        http_version: "HTTP/1.1".to_string(),
        headers: headers
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        body: body.to_vec(),
        timing: TimingBreakdown {
            dns: None,
            connect_tls: None,
            connect: None,
            tls: None,
            ttfb: Duration::from_millis(10),
            download: Duration::from_millis(5),
            total: Duration::from_millis(15),
        },
        size: Sizes {
            request_bytes: 100,
            header_bytes: 50,
            body_bytes: body.len() as u64,
        },
        effective_url: "https://example.test/thing".to_string(),
        redirect_chain: Vec::new(),
        cookies_set: Vec::new(),
        executed_at: Utc::now(),
    }
}

fn store() -> HistoryStore {
    HistoryStore::open_in_memory().expect("open in-memory history store")
}

fn ok_entry<'a>(exec: &'a ExecutionResult, name: &str, url: &str, method: &str) -> NewEntry<'a> {
    NewEntry {
        request_id: format!("req-{name}"),
        name: name.to_string(),
        method: method.to_string(),
        url: url.to_string(),
        env: Some("dev".to_string()),
        outcome: Ok(exec),
        request_headers: vec![("Authorization".to_string(), "Bearer xyz".to_string())],
        request_body: Some(b"{\"hello\":\"world\"}".to_vec()),
    }
}

#[test]
fn record_list_get_roundtrip() {
    let store = store();
    let exec = exec_result(
        200,
        b"{\"ok\":true}",
        vec![("Content-Type", "application/json"), ("X-Trace", "abc")],
    );
    let id = store
        .record(ok_entry(
            &exec,
            "Get Widget",
            "https://example.test/widgets/1",
            "GET",
        ))
        .unwrap();
    assert!(id > 0);

    let summaries = store.list(&HistoryFilter::default()).unwrap();
    assert_eq!(summaries.len(), 1);
    let summary = &summaries[0];
    assert_eq!(summary.id, id);
    assert_eq!(summary.name, "Get Widget");
    assert_eq!(summary.method, "GET");
    assert_eq!(summary.status, Some(200));
    assert_eq!(summary.error, None);

    let full = store.get(id).unwrap().expect("entry present");
    assert_eq!(full.id, id);
    assert_eq!(full.request_id, "req-Get Widget");
    assert_eq!(full.method, "GET");
    assert_eq!(full.url, "https://example.test/widgets/1");
    assert_eq!(full.status, Some(200));
    assert_eq!(full.env.as_deref(), Some("dev"));
    assert_eq!(
        full.request_headers,
        vec![("Authorization".to_string(), "Bearer xyz".to_string())]
    );
    assert_eq!(
        full.request_body.as_deref(),
        Some(&b"{\"hello\":\"world\"}"[..])
    );
    assert_eq!(
        full.response_headers,
        vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("X-Trace".to_string(), "abc".to_string()),
        ]
    );
    assert_eq!(full.response_body.as_deref(), Some(&b"{\"ok\":true}"[..]));
    assert!(!full.truncated);
    assert_eq!(full.duration_ms, 15);

    assert!(store.get(id + 1).unwrap().is_none());
}

#[test]
fn owned_adapter_record_uses_the_same_store() {
    let store = store();
    let id = store
        .record_raw(HistoryRecord {
            executed_at: Utc::now().to_rfc3339(),
            request_id: "v1.users".to_string(),
            name: "List users".to_string(),
            method: "GET".to_string(),
            url: "${env.baseUrl}/users".to_string(),
            status: Some(200),
            duration_ms: 7,
            request_headers: Vec::new(),
            request_body: None,
            response_headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            response_body: Some(b"[]".to_vec()),
            error: None,
            env: Some("local".to_string()),
        })
        .unwrap();

    let entry = store.get(id).unwrap().unwrap();
    assert_eq!(entry.request_id, "v1.users");
    assert_eq!(entry.response_body.as_deref(), Some(&b"[]"[..]));
}

#[test]
fn filter_by_text_method_and_status() {
    let store = store();

    let e1 = exec_result(200, b"one", vec![]);
    let e2 = exec_result(404, b"two", vec![]);
    let e3 = exec_result(500, b"three", vec![]);

    store
        .record(ok_entry(&e1, "List Users", "https://api.test/users", "GET"))
        .unwrap();
    store
        .record(ok_entry(
            &e2,
            "Get Missing",
            "https://api.test/users/999",
            "GET",
        ))
        .unwrap();
    store
        .record(ok_entry(
            &e3,
            "Create Order",
            "https://api.test/orders",
            "POST",
        ))
        .unwrap();

    // text match against name
    let by_text = store
        .list(&HistoryFilter {
            text: Some("order".to_string()),
            ..HistoryFilter::default()
        })
        .unwrap();
    assert_eq!(by_text.len(), 1);
    assert_eq!(by_text[0].name, "Create Order");

    // text match against url
    let by_url = store
        .list(&HistoryFilter {
            text: Some("users".to_string()),
            ..HistoryFilter::default()
        })
        .unwrap();
    assert_eq!(by_url.len(), 2);

    // method filter
    let by_method = store
        .list(&HistoryFilter {
            method: Some("POST".to_string()),
            ..HistoryFilter::default()
        })
        .unwrap();
    assert_eq!(by_method.len(), 1);
    assert_eq!(by_method[0].name, "Create Order");

    // status range filter
    let by_status = store
        .list(&HistoryFilter {
            status_min: Some(400),
            status_max: Some(499),
            ..HistoryFilter::default()
        })
        .unwrap();
    assert_eq!(by_status.len(), 1);
    assert_eq!(by_status[0].name, "Get Missing");

    // combined: no match
    let none = store
        .list(&HistoryFilter {
            text: Some("order".to_string()),
            method: Some("GET".to_string()),
            ..HistoryFilter::default()
        })
        .unwrap();
    assert!(none.is_empty());
}

#[test]
fn filter_by_request_id() {
    let store = store();
    let e1 = exec_result(200, b"a", vec![]);
    let e2 = exec_result(200, b"b", vec![]);

    let mut entry1 = ok_entry(&e1, "First", "https://api.test/a", "GET");
    entry1.request_id = "fixed-id".to_string();
    store.record(entry1).unwrap();

    let mut entry2 = ok_entry(&e2, "Second", "https://api.test/b", "GET");
    entry2.request_id = "other-id".to_string();
    store.record(entry2).unwrap();

    let results = store
        .list(&HistoryFilter {
            request_id: Some("fixed-id".to_string()),
            ..HistoryFilter::default()
        })
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "First");
}

#[test]
fn prune_keeps_newest_n() {
    let store = store();
    for i in 0..10 {
        let exec = exec_result(200, b"body", vec![]);
        store
            .record(ok_entry(
                &exec,
                &format!("Req {i}"),
                "https://api.test/x",
                "GET",
            ))
            .unwrap();
    }
    assert_eq!(store.count().unwrap(), 10);

    let deleted = store.prune(4).unwrap();
    assert_eq!(deleted, 6);
    assert_eq!(store.count().unwrap(), 4);

    let remaining = store.list(&HistoryFilter::default()).unwrap();
    assert_eq!(remaining.len(), 4);
    // Newest first, so the last 4 recorded ("Req 6".."Req 9") survive.
    let names: Vec<&str> = remaining.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["Req 9", "Req 8", "Req 7", "Req 6"]);
}

#[test]
fn delete_and_clear() {
    let store = store();
    let exec = exec_result(200, b"body", vec![]);
    let id = store
        .record(ok_entry(&exec, "One", "https://api.test/x", "GET"))
        .unwrap();
    let exec2 = exec_result(200, b"body2", vec![]);
    store
        .record(ok_entry(&exec2, "Two", "https://api.test/y", "GET"))
        .unwrap();

    assert_eq!(store.count().unwrap(), 2);
    store.delete(id).unwrap();
    assert_eq!(store.count().unwrap(), 1);
    assert!(store.get(id).unwrap().is_none());

    store.clear().unwrap();
    assert_eq!(store.count().unwrap(), 0);
}

#[test]
fn large_body_is_capped_and_flagged() {
    let store = store();
    let big_body = vec![b'x'; 512 * 1024 + 100];
    let exec = exec_result(200, &big_body, vec![]);
    let mut entry = ok_entry(&exec, "Big", "https://api.test/big", "GET");
    entry.request_body = Some(vec![b'y'; 512 * 1024 + 1]);

    let id = store.record(entry).unwrap();
    let full = store.get(id).unwrap().unwrap();
    assert!(full.truncated);
    assert_eq!(full.response_body.as_ref().unwrap().len(), 512 * 1024);
    assert_eq!(full.request_body.as_ref().unwrap().len(), 512 * 1024);
}

#[test]
fn small_body_is_not_flagged_truncated() {
    let store = store();
    let exec = exec_result(200, b"tiny", vec![]);
    let id = store
        .record(ok_entry(&exec, "Small", "https://api.test/small", "GET"))
        .unwrap();
    let full = store.get(id).unwrap().unwrap();
    assert!(!full.truncated);
}

#[test]
fn error_outcome_has_null_status_and_error_message() {
    let store = store();
    let entry = NewEntry {
        request_id: "req-err".to_string(),
        name: "Times Out".to_string(),
        method: "GET".to_string(),
        url: "https://api.test/slow".to_string(),
        env: None,
        outcome: Err("timed out after 30s"),
        request_headers: vec![],
        request_body: None,
    };
    let id = store.record(entry).unwrap();

    let summary = store
        .list(&HistoryFilter::default())
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(summary.status, None);
    assert_eq!(summary.error.as_deref(), Some("timed out after 30s"));

    let full = store.get(id).unwrap().unwrap();
    assert_eq!(full.status, None);
    assert_eq!(full.error.as_deref(), Some("timed out after 30s"));
    assert_eq!(full.response_body, None);
    assert!(full.response_headers.is_empty());
}

#[test]
fn diff_text_reports_added_and_removed_lines_with_unified_markers() {
    let old = "line one\nline two\nline three\n";
    let new = "line one\nline TWO\nline three\nline four\n";
    let result = diff_text(old, new);

    assert_eq!(result.removed, 1);
    assert_eq!(result.added, 2);
    assert!(result.unified.contains("--- a"));
    assert!(result.unified.contains("+++ b"));
    assert!(result.unified.contains("-line two"));
    assert!(result.unified.contains("+line TWO"));
    assert!(result.unified.contains("+line four"));
}

#[test]
fn diff_text_identical_is_empty() {
    let result = diff_text("same\ntext\n", "same\ntext\n");
    assert_eq!(result.added, 0);
    assert_eq!(result.removed, 0);
    assert!(result.unified.is_empty());
}

#[test]
fn diff_entries_normalizes_json_key_order() {
    let store = store();
    let exec_a = exec_result(200, br#"{"b":1,"a":2}"#, vec![]);
    let exec_b = exec_result(200, br#"{"a":2,"b":1}"#, vec![]);

    let id_a = store
        .record(ok_entry(&exec_a, "A", "https://api.test/x", "GET"))
        .unwrap();
    let id_b = store
        .record(ok_entry(&exec_b, "B", "https://api.test/x", "GET"))
        .unwrap();

    let a = store.get(id_a).unwrap().unwrap();
    let b = store.get(id_b).unwrap().unwrap();

    let result = diff_entries(&a, &b);
    assert_eq!(result.added, 0, "unified diff:\n{}", result.unified);
    assert_eq!(result.removed, 0, "unified diff:\n{}", result.unified);
    assert!(result.unified.is_empty());
}

#[test]
fn diff_entries_detects_real_value_change() {
    let store = store();
    let exec_a = exec_result(200, br#"{"a":1,"b":2}"#, vec![]);
    let exec_b = exec_result(200, br#"{"a":1,"b":3}"#, vec![]);

    let id_a = store
        .record(ok_entry(&exec_a, "A", "https://api.test/x", "GET"))
        .unwrap();
    let id_b = store
        .record(ok_entry(&exec_b, "B", "https://api.test/x", "GET"))
        .unwrap();

    let a = store.get(id_a).unwrap().unwrap();
    let b = store.get(id_b).unwrap().unwrap();

    let result = diff_entries(&a, &b);
    assert_eq!(result.added, 1);
    assert_eq!(result.removed, 1);
    assert!(result.unified.contains("-  \"b\": 2"));
    assert!(result.unified.contains("+  \"b\": 3"));
}

#[test]
fn diff_entries_falls_back_to_lossy_text_for_non_json() {
    let store = store();
    let exec_a = exec_result(200, b"plain text body one", vec![]);
    let exec_b = exec_result(200, b"plain text body two", vec![]);

    let id_a = store
        .record(ok_entry(&exec_a, "A", "https://api.test/x", "GET"))
        .unwrap();
    let id_b = store
        .record(ok_entry(&exec_b, "B", "https://api.test/x", "GET"))
        .unwrap();

    let a = store.get(id_a).unwrap().unwrap();
    let b = store.get(id_b).unwrap().unwrap();

    let result = diff_entries(&a, &b);
    assert_eq!(result.added, 1);
    assert_eq!(result.removed, 1);
}

#[test]
fn pagination_limit_and_offset() {
    let store = store();
    for i in 0..5 {
        let exec = exec_result(200, b"body", vec![]);
        store
            .record(ok_entry(
                &exec,
                &format!("Req {i}"),
                "https://api.test/x",
                "GET",
            ))
            .unwrap();
    }

    let page1 = store
        .list(&HistoryFilter {
            limit: 2,
            offset: 0,
            ..HistoryFilter::default()
        })
        .unwrap();
    let page2 = store
        .list(&HistoryFilter {
            limit: 2,
            offset: 2,
            ..HistoryFilter::default()
        })
        .unwrap();
    assert_eq!(page1.len(), 2);
    assert_eq!(page2.len(), 2);
    assert_ne!(page1[0].id, page2[0].id);
}

#[test]
fn open_file_backed_store_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite3");

    {
        let store = HistoryStore::open(&path).unwrap();
        let exec = exec_result(201, b"created", vec![]);
        store
            .record(ok_entry(
                &exec,
                "Persisted",
                "https://api.test/create",
                "POST",
            ))
            .unwrap();
    }

    let reopened = HistoryStore::open(&path).unwrap();
    let all = reopened.list(&HistoryFilter::default()).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "Persisted");
}
