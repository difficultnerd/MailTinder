#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! The scans that run across every journey (T-1101c): service logs, Firestore
//! and browser storage hold no fixture corpus canary, address or URL, and the app
//! ran under the shipped CSP with Trusted Types.
//!
//! `scripts/e2e.sh` runs this target in its own `cargo test` call after the
//! journeys, with `MT_E2E_LEAK_SCAN=1`: the evidence is the whole run (every
//! journey's log lines, every dumped document, every browser dump), so a scan
//! that ran inside the journey phase - where a journey's own artefacts may not
//! exist yet - would report a missing file that is not missing (S10 7.3). Each
//! scan returns early without the flag, and fails loudly when a log, dump or
//! document it needs is absent: a missing file is not a pass.

use std::error::Error;
use std::path::PathBuf;

use e2e::{
    documents_in, finished_journey_dumps, firestore_emulator_url, leak_scan_enabled, Canaries,
    FirestoreDump, Stack, E2E_FIRESTORE_PROJECT, FIRESTORE_COLLECTIONS,
};
use serde_json::Value;

/// The collections a card-building request writes: the acceptance criterion
/// FD-01 AC3 ("no message content reaches the server database while building
/// cards") is about these, while INV-1 is about the whole database.
const CARD_PATH_COLLECTIONS: &[&str] = &[
    "users",
    "mailboxes",
    "sessions",
    "jobs",
    "needs_attention",
    "classifier_eval",
];

/// Browser storage keys that hold no card data. Empty, and deliberately so: the
/// session lives in a `HttpOnly` cookie and everything else in memory, so the
/// app writes no key at all in `localStorage`, `sessionStorage` or IndexedDB
/// (S5 "Browser"; ASVS V14.3). A key appearing here is the regression this scan
/// exists for.
const ALLOWED_BROWSER_KEYS: &[&str] = &[];

/// The host header the app is served with, from `firebase.json` (T-006).
const CSP_HEADER: &str = "Content-Security-Policy";

/// The run's log directory (`MT_E2E_LOG_DIR`, default `target/e2e-logs`).
fn log_dir() -> Result<PathBuf, Box<dyn Error>> {
    let dir = std::env::var_os("MT_E2E_LOG_DIR")
        .map_or_else(|| PathBuf::from("target/e2e-logs"), PathBuf::from);
    if !dir.is_dir() {
        return Err(format!("no e2e log directory at {}", dir.display()).into());
    }
    Ok(dir)
}

/// Every line of every `*.jsonl` service log, with the file it came from. No
/// file at all is an error: a scan over nothing is not a pass.
fn service_log_lines() -> Result<Vec<(String, String)>, Box<dyn Error>> {
    let dir = log_dir()?;
    let mut lines = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        let text = std::fs::read_to_string(&path)?;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            lines.push((name.clone(), line.to_owned()));
        }
    }
    if lines.is_empty() {
        return Err(format!("no service log lines under {}", dir.display()).into());
    }
    Ok(lines)
}

/// The `event` type of one log line, when it is JSON with one.
fn log_event(line: &str) -> Option<String> {
    let value: Value = serde_json::from_str(line).ok()?;
    value.get("event")?.as_str().map(str::to_owned)
}

/// Everything the emulator holds, every S5 collection.
async fn firestore_dump() -> Result<Vec<(String, String)>, Box<dyn Error>> {
    let emulator = firestore_emulator_url()?;
    Ok(FirestoreDump::all_documents(&emulator, E2E_FIRESTORE_PROJECT).await?)
}

/// XC-01: no canary, corpus address or unsubscribe URL appears in any service
/// log line.
#[tokio::test]
#[ignore = "run by scripts/e2e.sh's scan phase"]
async fn xc_01_e2e_logs_hold_no_canary() -> Result<(), Box<dyn Error>> {
    if !leak_scan_enabled() {
        return Ok(());
    }
    let canaries = Canaries::load()?;
    let mut offenders = Vec::new();
    for (file, line) in service_log_lines()? {
        let found = canaries.find_in(&line);
        if !found.is_empty() {
            offenders.push(format!("{file}: {found:?}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "service logs hold corpus values: {offenders:?}"
    );
    Ok(())
}

/// LOG-1: no metric or request event - the two events the services write about a
/// user action - carries a value from the fixture mail corpus. The run must have
/// written some, or there is nothing to have scanned.
#[tokio::test]
#[ignore = "run by scripts/e2e.sh's scan phase"]
async fn log_1_e2e_logs_hold_no_corpus_value() -> Result<(), Box<dyn Error>> {
    if !leak_scan_enabled() {
        return Ok(());
    }
    let canaries = Canaries::load()?;
    let mut scanned = 0_usize;
    let mut offenders = Vec::new();
    for (file, line) in service_log_lines()? {
        let Some(event) = log_event(&line) else {
            continue;
        };
        if event != "metric" && event != "request" {
            continue;
        }
        scanned += 1;
        let found = canaries.find_in(&line);
        if !found.is_empty() {
            offenders.push(format!("{file}: {event}: {found:?}"));
        }
    }
    assert!(
        scanned > 0,
        "the run wrote no metric or request event to scan"
    );
    assert!(
        offenders.is_empty(),
        "metric and request events hold corpus values: {offenders:?}"
    );
    Ok(())
}

/// FD-01 AC3: no message content reaches the server database while building
/// cards - no document of the collections a card-building request writes holds a
/// canary, an address or an unsubscribe URL.
#[tokio::test]
#[ignore = "run by scripts/e2e.sh's scan phase"]
async fn fd_01_ac3_e2e_firestore_holds_no_card_content() -> Result<(), Box<dyn Error>> {
    if !leak_scan_enabled() {
        return Ok(());
    }
    let canaries = Canaries::load()?;
    let documents = firestore_dump().await?;
    let mut scanned = 0_usize;
    let mut offenders = Vec::new();
    for collection in CARD_PATH_COLLECTIONS {
        for (path, json) in documents_in(&documents, collection) {
            scanned += 1;
            let found = canaries.find_in(json);
            if !found.is_empty() {
                offenders.push(format!("{path}: {found:?}"));
            }
        }
    }
    assert!(
        scanned > 0,
        "the run wrote no document in the card path's collections: {CARD_PATH_COLLECTIONS:?}"
    );
    assert!(
        offenders.is_empty(),
        "Firestore holds message content: {offenders:?}"
    );
    Ok(())
}

/// INV-1: no Firestore document holds a body, subject or snippet. Only the
/// plaintext canaries are scanned; an encrypted field holds ciphertext by
/// construction, so a plaintext match anywhere is a failure (S3 INV-1, S5
/// DEL-2).
#[tokio::test]
#[ignore = "run by scripts/e2e.sh's scan phase"]
async fn inv_1_e2e_no_firestore_document_holds_mail_content() -> Result<(), Box<dyn Error>> {
    if !leak_scan_enabled() {
        return Ok(());
    }
    let canaries = Canaries::load()?;
    let documents = firestore_dump().await?;
    assert!(
        !documents.is_empty(),
        "the run wrote no document in any of {FIRESTORE_COLLECTIONS:?}"
    );
    let mut offenders = Vec::new();
    for (path, json) in &documents {
        let found = canaries.find_in(json);
        if !found.is_empty() {
            offenders.push(format!("{path}: {found:?}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "Firestore holds corpus values: {offenders:?}"
    );
    Ok(())
}

/// ASVS V14.3.3: browser storage holds no card data after every journey. Every
/// journey wrote its dump in `finish_journey`, and a journey without one fails
/// here rather than being skipped.
#[tokio::test]
#[ignore = "run by scripts/e2e.sh's scan phase"]
async fn asvs_v14_3_3_e2e_browser_storage_holds_no_card_data() -> Result<(), Box<dyn Error>> {
    if !leak_scan_enabled() {
        return Ok(());
    }
    let canaries = Canaries::load()?;
    let dumps = finished_journey_dumps()?;
    let mut offenders = Vec::new();
    for (journey, dump) in &dumps {
        let found = canaries.find_in(dump);
        if !found.is_empty() {
            offenders.push(format!("{journey}: a dump value holds {found:?}"));
        }
        let value: Value = serde_json::from_str(dump)
            .map_err(|e| format!("{journey}: the dump is not JSON: {e}"))?;
        for store in ["localStorage", "sessionStorage"] {
            for key in value
                .get(store)
                .and_then(Value::as_object)
                .map(|entries| entries.keys().map(String::as_str).collect::<Vec<_>>())
                .unwrap_or_default()
            {
                if !ALLOWED_BROWSER_KEYS.contains(&key) {
                    offenders.push(format!("{journey}: {store} holds the key {key:?}"));
                }
            }
        }
        for database in value
            .get("indexedDB")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !ALLOWED_BROWSER_KEYS.contains(&database) {
                offenders.push(format!("{journey}: IndexedDB holds {database:?}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "browser storage holds card data: {offenders:?}"
    );
    Ok(())
}

/// ASVS V3.4.3: the app completes a swipe under the shipped CSP with Trusted
/// Types and no violation.
///
/// Every journey checks that live: `finish_journey` reads Chrome's log after the
/// journey's last step and fails on any CSP or Trusted Types message, and its
/// marker is what this scan requires of every journey. On top of that, the host
/// must actually serve what `firebase.json` declares, so the policy the journeys
/// ran under is the shipped one and not a weaker copy (T-006).
#[tokio::test]
#[ignore = "run by scripts/e2e.sh's scan phase"]
async fn asvs_v3_4_3_e2e_swipe_completes_under_shipped_csp() -> Result<(), Box<dyn Error>> {
    if !leak_scan_enabled() {
        return Ok(());
    }
    // Every journey finished its swipe under the policy (each checks it live)
    // and left its marker plus its browser dump.
    let dumps = finished_journey_dumps()?;
    assert!(
        !dumps.is_empty(),
        "no journey finished, so nothing ran under the shipped CSP"
    );

    let declared = declared_csp()?;
    let stack = Stack::from_env()?;
    let served = served_csp(stack.app_url.join("index.html")?.as_ref()).await?;
    assert_eq!(
        served.trim(),
        declared.trim(),
        "the host serves a CSP the shipped firebase.json does not declare"
    );
    assert!(
        declared.contains("require-trusted-types-for 'script'"),
        "the shipped CSP does not require Trusted Types: {declared}"
    );
    assert!(
        declared.contains("trusted-types"),
        "the shipped CSP declares no Trusted Types policies: {declared}"
    );
    Ok(())
}

/// The `Content-Security-Policy` value `firebase.json` declares for every path
/// (S10 section 7.2, row "CSP compatibility").
fn declared_csp() -> Result<String, Box<dyn Error>> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../firebase.json");
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let value: Value = serde_json::from_str(&text)?;
    let headers = value
        .get("hosting")
        .and_then(|hosting| hosting.get("headers"))
        .and_then(Value::as_array)
        .ok_or("firebase.json declares no hosting headers")?;
    for block in headers {
        for header in block
            .get("headers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if header.get("key").and_then(Value::as_str) == Some(CSP_HEADER) {
                if let Some(csp) = header.get("value").and_then(Value::as_str) {
                    return Ok(csp.to_owned());
                }
            }
        }
    }
    Err(format!("firebase.json declares no {CSP_HEADER} header").into())
}

/// The `Content-Security-Policy` the host answers `url` with.
async fn served_csp(url: &str) -> Result<String, Box<dyn Error>> {
    let response = reqwest::get(url).await?;
    let status = response.status();
    let csp = response
        .headers()
        .get(CSP_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    csp.ok_or_else(|| {
        format!("{url} answered {status} with no {CSP_HEADER} header, so the app is not served as shipped")
            .into()
    })
}
