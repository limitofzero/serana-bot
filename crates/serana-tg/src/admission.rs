//! Whether an incoming update gets to run a command or touch the conversation at all.
//!
//! Kept free of teloxide's `Message` so the decision is a plain function of a few facts —
//! `main.rs` reads those facts off the `Message` and hands them here.

use serana_domain::reminder::UserId;

/// What to do with an update, decided once before any side effect or reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// No sender at all (a channel post, an anonymous admin): nobody to answer, and
    /// nobody to log either.
    Ignore,
    /// A sender who exists but is not on the allowlist. Say nothing back to them — an
    /// unauthenticated reply path is itself the vulnerability — but the caller logs this
    /// one, because it is how an owner who mistyped their own id finds out why the bot
    /// has gone silent.
    Refused(UserId),
    /// An allowed sender, but the chat is not private. Tell them where to go, and stop —
    /// no command runs, no topic is noted.
    PrivateChatOnly,
    /// An allowed sender in a private chat: proceed as normal.
    Admit(UserId),
}

/// Decide admission from the few facts that matter, so the caller never has to reason
/// about ordering: this is the one place the order is decided.
///
/// `sender` is `None` for channel posts and anonymous admins, who have nobody to answer.
/// A stranger is refused before the chat kind is even looked at, so a stranger in a group
/// gets the same silence as a stranger in a DM — telling them "private chats only" would
/// confirm the bot exists and is worth harassing. Pure and untraced on purpose: the caller
/// decides what, if anything, to log.
pub fn admit(
    sender: Option<UserId>,
    is_private: bool,
    allows: impl Fn(UserId) -> bool,
) -> Admission {
    let Some(sender) = sender else {
        return Admission::Ignore;
    };
    if !allows(sender) {
        return Admission::Refused(sender);
    }
    if !is_private {
        return Admission::PrivateChatOnly;
    }
    Admission::Admit(sender)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(ids: &[i64]) -> impl Fn(UserId) -> bool + '_ {
        move |user| ids.contains(&user.get())
    }

    #[test]
    fn no_sender_is_ignored_regardless_of_chat_kind() {
        assert_eq!(admit(None, true, allow(&[42])), Admission::Ignore);
        assert_eq!(admit(None, false, allow(&[42])), Admission::Ignore);
    }

    #[test]
    fn a_stranger_in_a_private_chat_is_refused() {
        let stranger = UserId::new(7);
        assert_eq!(
            admit(Some(stranger), true, allow(&[42])),
            Admission::Refused(stranger)
        );
    }

    #[test]
    fn a_stranger_in_a_group_is_refused_the_same_way() {
        // Not `PrivateChatOnly`: that reply would confirm the bot is there and listening,
        // which is exactly what an unauthenticated reply path must not do.
        let stranger = UserId::new(7);
        assert_eq!(
            admit(Some(stranger), false, allow(&[42])),
            Admission::Refused(stranger)
        );
    }

    #[test]
    fn an_allowed_user_in_a_group_is_told_to_use_a_private_chat() {
        let owner = UserId::new(42);
        assert_eq!(
            admit(Some(owner), false, allow(&[42])),
            Admission::PrivateChatOnly
        );
    }

    #[test]
    fn an_allowed_user_in_a_private_chat_is_admitted() {
        let owner = UserId::new(42);
        assert_eq!(
            admit(Some(owner), true, allow(&[42])),
            Admission::Admit(owner)
        );
    }

    #[test]
    fn an_empty_allowlist_refuses_everyone() {
        let anyone = UserId::new(1);
        assert_eq!(
            admit(Some(anyone), true, allow(&[])),
            Admission::Refused(anyone)
        );
        assert_eq!(
            admit(Some(anyone), false, allow(&[])),
            Admission::Refused(anyone)
        );
    }
}
