//! Build a `Ports` struct entirely from deterministic fakes.

use std::sync::Arc;

use domain::Classification;
use ports::{ClassifierError, ClassifierId, Ports, ServerStore};
use time::OffsetDateTime;

use crate::app_folder::InMemoryAppFolder;
use crate::classifier::FakeClassifier;
use crate::clock::{VirtualClock, T0};
use crate::egress::FakeHttpEgress;
use crate::identity::FakeIdentityProvider;
use crate::keys::{FakeKeyService, FakeSystemKeyService};
use crate::null_mail::NullMailProvider;
use crate::rng::SeededRng;
use crate::scheduler::FakeJobScheduler;
use crate::secrets::FakeSecrets;
use crate::store::InMemoryServerStore;

/// Every fake handle, so tests can script and inspect them.
pub struct Fakes {
    pub clock: Arc<VirtualClock>,
    pub rng: Arc<SeededRng>,
    pub store: Arc<InMemoryServerStore>,
    pub keys: Arc<FakeKeyService>,
    pub system_keys: Arc<FakeSystemKeyService>,
    pub scheduler: Arc<FakeJobScheduler>,
    pub egress: Arc<FakeHttpEgress>,
    pub identity: Arc<FakeIdentityProvider>,
    pub app_folder: Arc<InMemoryAppFolder>,
    pub secrets: Arc<FakeSecrets>,
    pub gemini: Arc<FakeClassifier>,
    pub jev: Arc<FakeClassifier>,
}

/// Build a `Ports` struct and its `Fakes`. Seed 42, clock at `T0`, gmail is
/// `NullMailProvider` (T-203 switches it to `FakeMailbox`).
pub fn fake_ports() -> (Ports, Fakes) {
    let clock = Arc::new(VirtualClock::new(T0));
    let rng = Arc::new(SeededRng::new(42));
    let store = Arc::new(InMemoryServerStore::new());
    let keys = Arc::new(FakeKeyService::new(Arc::clone(&rng) as Arc<dyn ports::Rng>));
    let system_keys = Arc::new(FakeSystemKeyService::new(
        Arc::clone(&rng) as Arc<dyn ports::Rng>
    ));
    let scheduler = Arc::new(FakeJobScheduler::new());
    let egress = Arc::new(FakeHttpEgress::new());
    let identity = Arc::new(FakeIdentityProvider::new());
    let app_folder = Arc::new(InMemoryAppFolder::new());
    let secrets = Arc::new(FakeSecrets::with_defaults());
    let gemini = Arc::new(FakeClassifier::new(
        "gemini@flash-lite",
        Err(ClassifierError::Disabled),
    ));
    let jev = Arc::new(FakeClassifier::new(
        "jev@1.13.0",
        Err(ClassifierError::Disabled),
    ));

    let ports = Ports {
        clock: Arc::clone(&clock) as Arc<dyn ports::Clock>,
        rng: Arc::clone(&rng) as Arc<dyn ports::Rng>,
        gmail: Arc::new(NullMailProvider),
        app_folder: Arc::clone(&app_folder) as Arc<dyn ports::AppFolderStore>,
        store: Arc::clone(&store) as Arc<dyn ServerStore>,
        keys: Arc::clone(&keys) as Arc<dyn ports::KeyService>,
        system_keys: Arc::clone(&system_keys) as Arc<dyn ports::SystemKeyService>,
        scheduler: Arc::clone(&scheduler) as Arc<dyn ports::JobScheduler>,
        egress: Arc::clone(&egress) as Arc<dyn ports::HttpEgress>,
        identity: Arc::clone(&identity) as Arc<dyn ports::IdentityProvider>,
        models: vec![
            Arc::clone(&gemini) as Arc<dyn ports::Classifier>,
            Arc::clone(&jev) as Arc<dyn ports::Classifier>,
        ],
        secrets: Arc::clone(&secrets) as Arc<dyn ports::Secrets>,
    };

    let fakes = Fakes {
        clock,
        rng,
        store,
        keys,
        system_keys,
        scheduler,
        egress,
        identity,
        app_folder,
        secrets,
        gemini,
        jev,
    };

    (ports, fakes)
}

/// A helper to silence unused-import warnings for types used only in tests.
#[allow(dead_code)]
fn _unused(_: Classification, _: ClassifierId, _: OffsetDateTime) {}
