// Test cases for Semgrep project rules in .semgrep/mailtinder.yml

fn test_mailtinder_rules() {
    // ruleid: mailtinder-no-permanent-delete
    client.delete(format!("{BASE}/users/me/messages/{id}"));

    // ruleid: mailtinder-no-permanent-delete
    let action = "batchDelete";

    // ok: mailtinder-no-permanent-delete
    client.delete(format!("{BASE}/users/me/labels/{id}"));

    // ruleid: mailtinder-no-wall-clock-or-thread-rng
    let now = SystemTime::now();

    // ok: mailtinder-no-wall-clock-or-thread-rng
    let now = clock.now();
}
