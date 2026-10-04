//! The `Ports` struct: every I/O boundary wired together.

use std::sync::Arc;

use crate::app_folder::AppFolderStore;
use crate::classifier::Classifier;
use crate::clock::Clock;
use crate::egress::HttpEgress;
use crate::identity::IdentityProvider;
use crate::keys::{KeyService, SystemKeyService};
use crate::mail::MailProvider;
use crate::rng::Rng;
use crate::scheduler::JobScheduler;
use crate::secrets::Secrets;
use crate::store::ServerStore;
use domain::Provider;

/// Every I/O boundary a service needs, wired together. `Clone` (all `Arc`).
#[derive(Clone)]
pub struct Ports {
    pub clock: Arc<dyn Clock>,
    pub rng: Arc<dyn Rng>,
    pub gmail: Arc<dyn MailProvider>,
    pub app_folder: Arc<dyn AppFolderStore>,
    pub store: Arc<dyn ServerStore>,
    pub keys: Arc<dyn KeyService>,
    pub system_keys: Arc<dyn SystemKeyService>,
    pub scheduler: Arc<dyn JobScheduler>,
    pub egress: Arc<dyn HttpEgress>,
    pub identity: Arc<dyn IdentityProvider>,
    /// Bake-off models only; header rules is in-process (T-102).
    pub models: Vec<Arc<dyn Classifier>>,
    pub secrets: Arc<dyn Secrets>,
}

impl Ports {
    /// Exhaustive match: adding `Provider::Graph` in v2 must fail to compile here.
    pub fn mail(&self, provider: Provider) -> &Arc<dyn MailProvider> {
        match provider {
            Provider::Gmail => &self.gmail,
        }
    }
}
