//! Whether a replied-to message is safe to quote back to the model.
//!
//! A reply is only quotable when the bot itself sent the message being replied to. Telegram
//! lets anyone reply to anyone, and in a private chat the replied-to message can be the
//! user's own earlier text, or a message forwarded in from a third party. Quoting the
//! latter would hand an outsider's words the "[replying to this message of yours: …]"
//! framing that tells the model it wrote them itself — including a planted reminder id
//! marker or instructions the model is told to treat as trusted. Checking the author, not
//! `is_bot`, is what rules that out: `is_bot` says the sender is *some* bot, not this one.

use serana_domain::reminder::UserId;

/// The text to quote, if any: `Some` only when `replied_author` is the bot itself.
pub fn quotable(
    replied_author: Option<UserId>,
    replied_text: Option<&str>,
    me: UserId,
) -> Option<&str> {
    if replied_author? != me {
        return None;
    }
    replied_text
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: UserId = UserId::new(1);
    const SOMEONE_ELSE: UserId = UserId::new(2);

    #[test]
    fn authored_by_the_bot_is_quotable() {
        assert_eq!(quotable(Some(ME), Some("hello"), ME), Some("hello"));
    }

    #[test]
    fn authored_by_the_user_is_not_quotable() {
        // Otherwise a reply to the person's own earlier message would be framed to the
        // model as the model's own words.
        assert_eq!(quotable(Some(SOMEONE_ELSE), Some("hello"), ME), None);
    }

    #[test]
    fn no_author_is_not_quotable() {
        // An anonymous admin post or an automatic forward carries no `from`; treating that
        // as the bot's own text is exactly the laundering this function exists to block.
        assert_eq!(quotable(None, Some("hello"), ME), None);
    }

    #[test]
    fn bot_authored_but_no_text_is_not_quotable() {
        // A sticker or photo the bot sent has no text to quote.
        assert_eq!(quotable(Some(ME), None, ME), None);
    }
}
