//! Apply enabled sort rules before Feed cards are built (T-609).
use domain::user_state::{HistoryAction, HistoryEntry, HistoryOutcome, UserState};
use domain::{first_match, LabelSet, MessageMeta, RuleAction};
use uuid::Uuid;

use crate::error::ApiError;
use crate::services::categories::ensure_label_for;
use crate::services::feed::RuleApplication;
use crate::services::swipe::provider_of;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

pub const NS_RULE_ACTION: Uuid = Uuid::from_u128(0x6d74_7261_0000_4000_8000_0000_0000_0001);
/// `[DEFAULT]` bounds provider writes per load.
pub const MAX_RULE_ACTIONS_PER_LOAD: u32 = 200;

/// Apply rules outside the retryable state update; failed matches stay hidden.
///
/// # Errors
/// Returns storage errors while discovering the provider or creating a label.
pub async fn apply_rules(
    app: &AppState,
    session: &AuthedSession,
    state: &UserState,
    metas: &[MessageMeta],
) -> Result<RuleApplication, ApiError> {
    let rules: Vec<_> = state.rules.iter().map(|r| r.rule.clone()).collect();
    let mut result = RuleApplication::default();
    let mut attempted = 0;
    for meta in metas {
        let Some(rule) = first_match(&rules, meta) else {
            continue;
        };
        let Some(action) = rule.action() else {
            continue;
        };
        result
            .acted_on
            .insert((meta.mailbox.0, meta.id.as_str().to_owned()));
        if attempted >= MAX_RULE_ACTIONS_PER_LOAD {
            continue;
        }
        attempted += 1;
        let Ok(ctx) = app
            .tokens
            .mailbox_ctx(app, &session.user, &meta.mailbox)
            .await
        else {
            continue;
        };
        let provider = provider_of(app, &meta.mailbox).await?;
        let history_action = match action {
            RuleAction::Trash { .. } => {
                if app
                    .ports
                    .mail(provider)
                    .trash(&ctx, &meta.id)
                    .await
                    .is_err()
                {
                    continue;
                }
                HistoryAction::TrashedByRule
            }
            RuleAction::File { category, .. } => {
                let Some(category) = state.category(&category) else {
                    continue;
                };
                let label = match ensure_label_for(app, &session.user, &ctx, category).await {
                    Ok(label) => label,
                    Err(
                        ApiError::ProviderError { .. }
                        | ApiError::ProviderUnavailable { .. }
                        | ApiError::MailboxNeedsSignIn { .. },
                    ) => continue,
                    Err(e) => return Err(e),
                };
                let add = LabelSet::from_ids([label]);
                let remove = LabelSet::from_ids(["INBOX".to_owned()]);
                if app
                    .ports
                    .mail(provider)
                    .set_labels(&ctx, &meta.id, &add, &remove)
                    .await
                    .is_err()
                {
                    continue;
                }
                HistoryAction::FiledByRule
            }
        };
        // Fixed-width UUIDs delimit the provider message ID without ambiguity.
        let mut name = Vec::new();
        name.extend_from_slice(session.user.0.as_bytes());
        name.extend_from_slice(meta.mailbox.0.as_bytes());
        name.extend_from_slice(meta.id.as_str().as_bytes());
        name.extend_from_slice(rule.rule_id.0.as_bytes());
        result.entries.push(HistoryEntry {
            entry_id: Uuid::new_v5(&NS_RULE_ACTION, &name),
            at: app.ports.clock.now(),
            mailbox_id: meta.mailbox,
            sender_display: crate::text::plain_text(&meta.from_display, 256),
            action: history_action,
            outcome: HistoryOutcome::Done,
            rule_id: Some(rule.rule_id),
        });
        result.applied += 1;
    }
    Ok(result)
}
