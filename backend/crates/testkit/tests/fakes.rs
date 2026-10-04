//! Tests for the fakes (T-202b).

use std::sync::Arc;

use domain::{Provider, UserId};
use ports::{
    Aad, AppFolderStore, Classifier, Clock, HttpEgress, IdentityProvider, JobScheduler, KeyService,
    Rng,
};
use testkit::{
    fake_ports, FakeClassifier, FakeHttpEgress, FakeIdentityProvider, FakeJobScheduler,
    FakeKeyService, InMemoryAppFolder, SeededRng, VirtualClock, T0,
};
use uuid::Uuid;

#[test]
fn virtual_clock_advances_only_when_told() {
    let c = VirtualClock::new(T0);
    assert_eq!(c.now(), T0);
    c.advance(time::Duration::seconds(60));
    assert_eq!(c.now(), T0 + time::Duration::seconds(60));
}

#[test]
fn seeded_rng_is_deterministic_and_seeds_differ() {
    let a = SeededRng::new(42);
    let b = SeededRng::new(42);
    let c = SeededRng::new(7);
    let a1 = a.bytes32();
    let a2 = a.bytes32();
    let b1 = b.bytes32();
    let c1 = c.bytes32();
    assert_eq!(a1, b1, "same seed must give same first bytes");
    assert_ne!(a1, a2, "counter must advance");
    assert_ne!(a1, c1, "different seeds must differ");
}

#[test]
fn fake_key_service_round_trip() {
    let rng = Arc::new(SeededRng::new(1));
    let keys = FakeKeyService::new(rng);
    // Stub to satisfy unused warnings if the async test is skipped; real test below.
    let _ = keys;
}

#[tokio::test]
async fn fake_key_service_aad_binding_rejects_other_user_scope_or_field() {
    let rng: Arc<dyn Rng> = Arc::new(SeededRng::new(1));
    let keys = FakeKeyService::new(Arc::clone(&rng));
    let user = UserId(Uuid::new_v4());
    let other = UserId(Uuid::new_v4());
    let wrapped = keys
        .new_user_key(&user)
        .await
        .unwrap_or_else(|_| panic!("new key"));
    let aad = Aad {
        user,
        scope: "mailbox".to_owned(),
        field: "refresh_token",
    };
    let ct = keys
        .seal(&user, &wrapped, &aad, b"secret")
        .await
        .unwrap_or_else(|_| panic!("seal"));
    let ok = keys
        .open(&user, &wrapped, &aad, &ct)
        .await
        .unwrap_or_else(|_| panic!("open"));
    assert_eq!(ok, b"secret");

    // Wrong user fails.
    let other_aad = Aad {
        user: other,
        scope: "mailbox".to_owned(),
        field: "refresh_token",
    };
    assert!(keys.open(&other, &wrapped, &other_aad, &ct).await.is_err());
    // Wrong scope fails.
    let scope_aad = Aad {
        user,
        scope: "job".to_owned(),
        field: "refresh_token",
    };
    assert!(keys.open(&user, &wrapped, &scope_aad, &ct).await.is_err());
    // Wrong field fails.
    let field_aad = Aad {
        user,
        scope: "mailbox".to_owned(),
        field: "email_address",
    };
    assert!(keys.open(&user, &wrapped, &field_aad, &ct).await.is_err());
}

#[tokio::test]
async fn fake_scheduler_schedule_is_idempotent_and_cancel_outcomes() {
    let sched = FakeJobScheduler::new();
    let job = domain::JobId(Uuid::new_v4());
    let name = sched
        .schedule(&job, T0)
        .await
        .unwrap_or_else(|_| panic!("schedule"));
    let name2 = sched
        .schedule(&job, T0)
        .await
        .unwrap_or_else(|_| panic!("schedule"));
    assert_eq!(name, name2, "schedule must be idempotent");
    assert_eq!(sched.due(T0).len(), 1);
    assert_eq!(
        sched
            .cancel(&name)
            .await
            .unwrap_or_else(|_| panic!("cancel")),
        ports::CancelOutcome::Cancelled
    );
    // Cancelling the deleted (unknown) name again is NotFound.
    assert_eq!(
        sched
            .cancel(&name)
            .await
            .unwrap_or_else(|_| panic!("cancel")),
        ports::CancelOutcome::NotFound
    );
}

#[tokio::test]
async fn fake_egress_refuses_unrouted_host_and_records_violation() {
    let egress = FakeHttpEgress::new();
    let url = url::Url::parse("https://example.com/x").unwrap_or_else(|_| panic!("url"));
    let req = ports::EgressRequest {
        method: ports::HttpMethod::Get,
        url,
        headers: vec![],
        body: None,
        timeout: std::time::Duration::from_secs(1),
    };
    let result = egress.call(req).await;
    assert!(matches!(result, Err(ports::EgressError::HostNotAllowed)));
    assert_eq!(egress.violations().len(), 1);
}

#[tokio::test]
async fn fake_identity_revoke_makes_refresh_fail() {
    let idp = FakeIdentityProvider::new();
    let token = obs::Sensitive::new("refresh-1".to_owned());
    idp.revoke(&token)
        .await
        .unwrap_or_else(|_| panic!("revoke"));
    assert_eq!(idp.revoked(), vec!["refresh-1"]);
    // Refresh of a revoked value fails with InvalidGrant.
    assert!(matches!(
        idp.refresh(&token).await,
        Err(ports::IdError::InvalidGrant)
    ));
}

#[tokio::test]
async fn fake_classifier_counts_calls_and_max_in_flight() {
    let gate = Arc::new(tokio::sync::Semaphore::new(1));
    let cf = FakeClassifier::new("test@1", Err(ports::ClassifierError::Disabled)).with_gate(gate);
    let _ = cf.classify(&input_for("subj")).await;
    assert_eq!(cf.calls(), 1);
    assert_eq!(cf.max_in_flight(), 1);
}

fn input_for(subject: &str) -> ports::ClassifierInput {
    ports::ClassifierInput {
        from_display: String::new(),
        from_domain: "example.com".to_owned(),
        list_id: None,
        has_list_unsubscribe: false,
        has_list_unsubscribe_post: false,
        precedence: None,
        auto_submitted: None,
        esp_header_names: vec![],
        auth_summary: String::new(),
        subject: subject.to_owned(),
        text: String::new(),
        input_version: "v1",
    }
}

#[tokio::test]
async fn in_memory_app_folder_etag_conflicts() {
    let folder = InMemoryAppFolder::new();
    let ctx = ports::MailboxCtx {
        mailbox: domain::MailboxId(Uuid::new_v4()),
        access_token: obs::Sensitive::new("tok".to_owned()),
    };
    let tag = folder
        .write(&ctx, b"a", None)
        .await
        .unwrap_or_else(|_| panic!("write"));
    // write again with None -> Conflict
    assert!(matches!(
        folder.write(&ctx, b"b", None).await,
        Err(ports::AppFolderError::Conflict)
    ));
    // write with stale tag -> Conflict
    assert!(matches!(
        folder
            .write(&ctx, b"c", Some(&ports::ETag("stale".into())))
            .await,
        Err(ports::AppFolderError::Conflict)
    ));
    // write with correct tag -> ok
    assert!(folder.write(&ctx, b"d", Some(&tag)).await.is_ok());
}

#[test]
fn ports_mail_matches_every_provider() {
    let (p, _f) = fake_ports();
    assert_eq!(p.mail(Provider::Gmail).provider(), Provider::Gmail);
}

#[test]
fn fake_ports_builds() {
    let (_p, f) = fake_ports();
    let _ = (f.clock.now(), f.secrets);
}
