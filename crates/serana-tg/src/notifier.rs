//! Delivering what the schedulers push, over Telegram.

use async_trait::async_trait;
use serana_domain::NotifyError;
use serana_domain::prices::{DigestNotifier, Snapshot};
use serana_domain::reminder::{Notifier, Reminder, UserId};
use teloxide::prelude::*;
use teloxide::{ApiError, RequestError};

/// Sends what a scheduler pushed to its owner's private chat.
///
/// In a private chat Telegram's chat id and user id are the same number, so the owner's
/// [`UserId`] is enough to reach them without storing a separate chat id.
#[derive(Clone)]
pub struct TelegramNotifier {
    bot: Bot,
}

impl TelegramNotifier {
    pub fn new(bot: Bot) -> Self {
        Self { bot }
    }
}

#[async_trait]
impl Notifier for TelegramNotifier {
    async fn notify(
        &self,
        owner: UserId,
        reminder: &Reminder,
        now: jiff::Timestamp,
    ) -> Result<(), NotifyError> {
        // Presented, not passed through: a delivery arrives unprompted and has to read as a
        // reminder rather than as the assistant saying something. The wording lives in
        // `serana-app` with every other line the user sees.
        self.bot
            .send_message(
                ChatId(owner.get()),
                serana_app::text::reminders::fired(reminder, now),
            )
            .await
            .map(|_| ())
            .map_err(classify)
    }
}

#[async_trait]
impl DigestNotifier for TelegramNotifier {
    async fn notify(&self, owner: UserId, snapshot: &Snapshot) -> Result<(), NotifyError> {
        // The digest is read at the moment it is sent, so "how old is this?" is answered
        // against the same instant it was taken — it reads as "just now", which is true.
        self.bot
            .send_message(
                ChatId(owner.get()),
                serana_app::text::prices::pushed(snapshot),
            )
            .await
            .map(|_| ())
            .map_err(classify)
    }
}

/// Split Telegram's failures into "stop trying" and "try again later".
///
/// Getting this wrong in the permanent direction loses reminders; getting it wrong in the
/// transient direction means every tick pays for a delivery that can never land.
fn classify(error: RequestError) -> NotifyError {
    match &error {
        RequestError::Api(
            ApiError::BotBlocked
            | ApiError::UserDeactivated
            | ApiError::ChatNotFound
            | ApiError::BotKicked
            | ApiError::BotKickedFromSupergroup,
        ) => NotifyError::Unreachable(error.to_string()),
        _ => NotifyError::Transport(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blocked_or_gone_recipient_is_permanent() {
        for api_error in [
            ApiError::BotBlocked,
            ApiError::UserDeactivated,
            ApiError::ChatNotFound,
            ApiError::BotKicked,
        ] {
            let classified = classify(RequestError::Api(api_error.clone()));
            assert!(
                matches!(classified, NotifyError::Unreachable(_)),
                "{api_error:?} should be permanent, got {classified:?}"
            );
        }
    }

    #[test]
    fn rate_limits_and_outages_are_transient() {
        let retry = classify(RequestError::RetryAfter(
            teloxide::types::Seconds::from_seconds(5),
        ));
        assert!(matches!(retry, NotifyError::Transport(_)), "{retry:?}");

        // An unrecognised API error must not be treated as permanent: deactivating a
        // reminder is irreversible from the user's point of view.
        let unknown = classify(RequestError::Api(ApiError::Unknown("teapot".into())));
        assert!(matches!(unknown, NotifyError::Transport(_)), "{unknown:?}");
    }
}
