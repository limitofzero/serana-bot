//! The Telegram bot: read the environment, wire the graph, dispatch.

use std::sync::Arc;

use serana_app::{
    AppConfig, Calendar, Command, Digest, Reminders, Topics, build_digest_scheduler,
    build_reminders, build_scheduler, respond, text,
};
use serana_domain::conversation::ConversationId;
use serana_domain::reminder::UserId;
use serana_tg::TelegramCommand;
use serana_tg::admission::{Admission, admit};
use serana_tg::config::TelegramConfig;
use serana_tg::notifier::TelegramNotifier;
use teloxide::prelude::*;
use teloxide::utils::command::BotCommands;

struct AppState {
    reminders: Reminders,
    /// `None` when no calendar is configured, which is a supported way to run.
    calendar: Option<Calendar>,
    prices: Digest,
    /// What each person was last talking about, so a bare message continues it.
    topics: Topics,
    telegram: TelegramConfig,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // A missing .env is normal: in Docker the variables come from the environment.
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = AppConfig::from_env()?;
    let telegram = TelegramConfig::from_env()?;
    if telegram.allowed_users.is_empty() {
        tracing::warn!(
            "SERANA_ALLOWED_USER_IDS is empty, so the bot will answer nobody. \
             Set it to your Telegram user id."
        );
    }

    let wiring = build_reminders(&config).await?;
    let bot = Bot::new(&telegram.token);

    // One scheduler ticking beside the dispatcher. Both share the database; SQLite in WAL
    // mode lets the tick read while a command writes.
    let tick_interval = config.tick_interval;
    let digests = build_digest_scheduler(&wiring, TelegramNotifier::new(bot.clone()));
    tokio::spawn(async move { digests.run(tick_interval).await });

    let scheduler = build_scheduler(wiring.repository, TelegramNotifier::new(bot.clone()));
    tokio::spawn(async move { scheduler.run(tick_interval).await });

    // Telegram keeps the command menu server-side, per bot token. Without this it keeps
    // showing whatever was registered last — including commands from an entirely different
    // program that once held this token. Re-registering on every start also means adding a
    // variant to `TelegramCommand` is all it takes for the menu to follow.
    if let Err(error) = bot.set_my_commands(TelegramCommand::bot_commands()).await {
        // Not fatal: the bot answers its commands whether or not the menu lists them, and
        // refusing to start over a cosmetic call would be worse than a stale menu.
        tracing::warn!(%error, "could not register the command menu with Telegram");
    }

    tracing::info!(
        allowed = telegram.allowed_users.len(),
        model = %config.model,
        timezone = %config.timezone,
        calendar = wiring.calendar.is_some(),
        "serana is up"
    );

    let state = Arc::new(AppState {
        reminders: wiring.reminders,
        calendar: wiring.calendar,
        prices: wiring.prices,
        topics: Topics::new(),
        telegram,
    });
    // Two branches over the same messages: a slash command, or anything else. Without the
    // second, an answer to a question the assistant just asked ("tomorrow at 9") would be
    // silently dropped, which is the one thing a conversation cannot afford.
    let handler = Update::filter_message()
        .branch(
            dptree::entry()
                .filter_command::<TelegramCommand>()
                .endpoint(dispatch),
        )
        .branch(dptree::endpoint(dispatch_text));
    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

/// Who sent this. Channel posts and similar have none, and there is nobody to answer.
fn sender(message: &Message) -> Option<UserId> {
    message
        .from
        .as_ref()
        .map(|user| UserId::new(user.id.0 as i64))
}

/// Send `body` back, logging rather than failing the update if it cannot be delivered.
async fn reply(bot: &Bot, message: &Message, body: String) {
    if let Err(error) = bot.send_message(message.chat.id, body).await {
        tracing::warn!(%error, "could not send a reply");
    }
}

/// Decide admission and act on everything except `Admit`: log a refusal, or reply and stop
/// for a group chat. Returns the sender only when the caller may proceed.
///
/// Shared by `dispatch` and `dispatch_text` so the decision, the log line and the
/// private-chat reply exist in exactly one place each.
async fn admitted(bot: &Bot, message: &Message, state: &AppState) -> Option<UserId> {
    match admit(sender(message), message.chat.is_private(), |user| {
        state.telegram.allows(user)
    }) {
        Admission::Ignore => None,
        Admission::Refused(sender) => {
            // Logged at warn so an owner who mistyped their own id can see why nothing
            // works. No reply: an unauthenticated reply path is the vulnerability.
            tracing::warn!(user = %sender, "refused a command from a user outside the allowlist");
            None
        }
        Admission::PrivateChatOnly => {
            reply(bot, message, text::PRIVATE_CHAT_ONLY.to_owned()).await;
            None
        }
        Admission::Admit(sender) => Some(sender),
    }
}

/// A message that is not a command: treated as a continuation of the conversation.
async fn dispatch_text(bot: Bot, message: Message, state: Arc<AppState>) -> anyhow::Result<()> {
    let Some(text) = message
        .text()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
    else {
        // Stickers, photos, joins — nothing to read.
        return Ok(());
    };

    // Admission is decided before anything else runs, exactly as in `dispatch` — a
    // slash-prefixed word that reached here is still a message from whoever sent it, and a
    // stranger's guess at a command must not get a reply any more than their plain text
    // would.
    let Some(sender) = admitted(&bot, &message, &state).await else {
        return Ok(());
    };

    // A slash-prefixed word that reached here is a command we do not have; answering it as
    // a reminder request would be worse than saying so.
    if text.starts_with('/') {
        reply(&bot, &message, text::UNKNOWN_COMMAND.to_owned()).await;
        return Ok(());
    }

    // A bare "yes" answers the last question asked, and the assistant keeps one
    // conversation per subject.
    let last = state.topics.last(sender);
    // A reply is how a person points at something. The message being replied to is very
    // often a delivery the scheduler pushed, which never entered the conversation, so this
    // is the only way the assistant can know what "this one" refers to — and it is aimed at
    // the conversation that message came from, which need not be the last one they used.
    let command = match message
        .reply_to_message()
        .and_then(|replied| replied.text())
    {
        Some(quoted) => Command::replying(text::topic_of(quoted).unwrap_or(last), quoted, &text),
        None => Command::bare(last, &text),
    };
    run(bot, message, sender, command, state).await
}

async fn dispatch(
    bot: Bot,
    message: Message,
    command: TelegramCommand,
    state: Arc<AppState>,
) -> anyhow::Result<()> {
    // Admission is decided once, before the command that teloxide already parsed runs.
    let Some(sender) = admitted(&bot, &message, &state).await else {
        return Ok(());
    };
    run(bot, message, sender, Command::from(command), state).await
}

/// Run an already-admitted command and send the reply.
///
/// Everything that can affect state or produce a reply happens here, and only here —
/// `dispatch` and `dispatch_text` both gate on [`admit`] before reaching it, so `sender` is
/// known to be on the allowlist and `message.chat` is known to be a private chat.
async fn run(
    bot: Bot,
    message: Message,
    sender: UserId,
    command: Command,
    state: Arc<AppState>,
) -> anyhow::Result<()> {
    // Noted before the turn, not after: it is where the next bare message goes, and a turn
    // that fails is still the subject they are on.
    state.topics.note(sender, &command);
    let reply = respond(
        &state.reminders,
        state.calendar.as_ref(),
        &state.prices,
        sender,
        &ConversationId::new(format!("tg:{sender}")),
        &command,
    )
    .await;

    bot.send_message(message.chat.id, reply).await?;
    Ok(())
}
