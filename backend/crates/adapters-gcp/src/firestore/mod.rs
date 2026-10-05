//! The production `ServerStore` over the Firestore REST API.

pub mod repos;
pub mod rest;
pub mod value;

use std::sync::Arc;

use ports::store::{
    BakeoffSnapshotRepo, ClassifierEvalRepo, ConfigRepo, InviteRepo, InviteRequestRepo, JobRepo,
    MailboxRepo, NeedsAttentionRepo, RateLimitRepo, ServerStore, SessionRepo, UserRepo,
};

use crate::gcp_http::GcpHttp;

use self::repos::{
    BakeoffSnapshotsImpl, ClassifierEvalImpl, ConfigImpl, InviteRequestsImpl, InvitesImpl,
    JobsImpl, MailboxesImpl, NeedsAttentionImpl, RateLimitsImpl, SessionsImpl, UsersImpl,
};

/// Configuration for the Firestore store.
#[derive(Clone, Debug)]
pub struct FirestoreConfig {
    pub project_id: String,
    pub database: String,
}

impl FirestoreConfig {
    /// The default database is `(default)`.
    pub fn new(project_id: impl Into<String>) -> Self {
        Self {
            project_id: project_id.into(),
            database: "(default)".to_owned(),
        }
    }
}

/// The production `ServerStore` over Firestore.
pub struct FirestoreStore {
    users: UsersImpl,
    mailboxes: MailboxesImpl,
    invites: InvitesImpl,
    invite_requests: InviteRequestsImpl,
    jobs: JobsImpl,
    needs_attention: NeedsAttentionImpl,
    sessions: SessionsImpl,
    classifier_eval: ClassifierEvalImpl,
    bakeoff_snapshots: BakeoffSnapshotsImpl,
    config: ConfigImpl,
    rate_limits: RateLimitsImpl,
}

impl FirestoreStore {
    /// Builds a store bound to the given project and database.
    pub fn new(http: Arc<GcpHttp>, cfg: FirestoreConfig) -> Self {
        let fs = Arc::new(rest::Firestore::new(http, &cfg.project_id, &cfg.database));
        Self {
            users: UsersImpl::new(Arc::clone(&fs)),
            mailboxes: MailboxesImpl::new(Arc::clone(&fs)),
            invites: InvitesImpl::new(Arc::clone(&fs)),
            invite_requests: InviteRequestsImpl::new(Arc::clone(&fs)),
            jobs: JobsImpl::new(Arc::clone(&fs)),
            needs_attention: NeedsAttentionImpl::new(Arc::clone(&fs)),
            sessions: SessionsImpl::new(Arc::clone(&fs)),
            classifier_eval: ClassifierEvalImpl::new(Arc::clone(&fs)),
            bakeoff_snapshots: BakeoffSnapshotsImpl::new(Arc::clone(&fs)),
            config: ConfigImpl::new(Arc::clone(&fs)),
            rate_limits: RateLimitsImpl::new(Arc::clone(&fs)),
        }
    }
}

impl ServerStore for FirestoreStore {
    fn users(&self) -> &dyn UserRepo {
        &self.users
    }
    fn mailboxes(&self) -> &dyn MailboxRepo {
        &self.mailboxes
    }
    fn invites(&self) -> &dyn InviteRepo {
        &self.invites
    }
    fn invite_requests(&self) -> &dyn InviteRequestRepo {
        &self.invite_requests
    }
    fn jobs(&self) -> &dyn JobRepo {
        &self.jobs
    }
    fn needs_attention(&self) -> &dyn NeedsAttentionRepo {
        &self.needs_attention
    }
    fn sessions(&self) -> &dyn SessionRepo {
        &self.sessions
    }
    fn classifier_eval(&self) -> &dyn ClassifierEvalRepo {
        &self.classifier_eval
    }
    fn bakeoff_snapshots(&self) -> &dyn BakeoffSnapshotRepo {
        &self.bakeoff_snapshots
    }
    fn config(&self) -> &dyn ConfigRepo {
        &self.config
    }
    fn rate_limits(&self) -> &dyn RateLimitRepo {
        &self.rate_limits
    }
}
