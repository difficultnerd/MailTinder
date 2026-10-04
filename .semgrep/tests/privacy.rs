// Test cases for Semgrep privacy rules in .semgrep/privacy.yml

fn test_log_cases() {
    // ruleid: privacy-log-sensitive-identifier
    tracing::info!(subject = %m.subject);

    // ruleid: privacy-log-sensitive-identifier
    info!(from_address = "test@example.com");

    // ruleid: privacy-log-sensitive-identifier
    warn!("bad url {}", u);

    // ruleid: privacy-log-sensitive-identifier
    info!(message_id = "msg123");

    // ok: privacy-log-sensitive-identifier
    info!(route = "feed.next", status = 200);

    // ok: privacy-log-sensitive-identifier
    info!(latency_ms = 12);
}

// ruleid: privacy-rust-derive-debug-on-sensitive-struct
#[derive(Debug)]
struct SensitiveUser {
    subject: String,
}

// ok: privacy-rust-derive-debug-on-sensitive-struct
struct SafeUser {
    subject: String,
}
