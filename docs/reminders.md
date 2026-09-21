# How a reminder works

Reminders are the one feature that is finished, and they exercise the whole spine: a
transport, a model call, validation of what the model said, storage, and a background loop
that delivers. This is how the crates fit together around them.

Two paths, and they only meet at the database:

- **Create / list / delete** — synchronous, user-driven, needs the model.
- **Deliver** — a background tick, needs no model at all.

## The crates

```
          ┌───────────────┐              ┌───────────────┐
          │  serana-cli   │              │   serana-tg   │   composition roots
          │  (REPL)       │              │  (bot)        │   — transport only
          └───────┬───────┘              └───────┬───────┘
                  │        both call into        │
                  └──────────────┬───────────────┘
                                 ▼
                      ┌──────────────────────┐
                      │      serana-app      │   Command · respond · AppConfig
                      │                      │   wiring · every user-facing string
                      └──────────┬───────────┘
                     ┌───────────┴───────────┐
                     ▼                       ▼
          ┌────────────────────┐  ┌────────────────────┐
          │  serana-services   │  │  serana-adapters   │
          │  ReminderService   │  │  OpenAiProvider    │
          │  SchedulerService  │  │  SqliteReminder…   │
          │                    │  │  SystemClock       │
          └─────────┬──────────┘  └─────────┬──────────┘
                    │      never each other │
                    └───────────┬───────────┘
                                ▼
                     ┌────────────────────┐
                     │   serana-domain    │   Reminder · Recurrence · UserId
                     │  types + ports     │   ReminderRepository · Notifier
                     │  pure, no I/O      │   LlmProvider · Clock · IdGenerator
                     └────────────────────┘
```

`services` and `adapters` both point at `domain` and never at each other. That is what lets
every service be built out of `serana-testkit` fakes — the service has no idea whether its
`ReminderRepository` is SQLite or a `HashMap`.

## Creating one

`/reminder каждый месяц 20 число - писать мне что надо оформить invoice`

```
  Telegram                                CLI
     │                                     │
     ▼                                     ▼
  TelegramCommand                     input::parse(line)
  (teloxide derive)                   "/" optional, aliases
  serana-tg/command.rs                serana-cli/input.rs
     │                                     │
     │  From<TelegramCommand>              │  Input::Command(..)
     └──────────────┬──────────────────────┘
                    ▼
            Command::Reminder(String)            ← serana-app/command.rs
                    │                              the user's words, verbatim
                    ▼
            respond(&service, user, &command)    ← serana-app/respond.rs
                    │                              the only place a reply is written
                    ▼
            ReminderService::create(owner, request)   ← services/reminders/mod.rs
                    │
     ┌──────────────┼───────────────────────────────────────────┐
     │  1. Clock::now()            → SystemClock                │
     │  2. build the system prompt with the local date/time,    │
     │     because "next Friday" is meaningless without it      │
     │  3. LlmProvider::complete(                               │
     │        model, prompt, [user message],                    │
     │        tools: [ToolSpec::typed::<ParsedReminder>],       │  schema from schemars,
     │        temperature: 0.0 )   → OpenAiProvider             │  never hand-written
     └──────────────┬───────────────────────────────────────────┘
                    │  response.tool_calls
                    ▼
            ParsedReminder                        ← services/reminders/parse.rs
            {kind, time, weekday?, day_of_month?, date?, text}
                    │
                    │  into_recurrence() — the model did the language,
                    │  this does the validation: HH:MM, day 1-31,
                    │  weekday present when kind is weekly, …
                    ▼
            Recurrence::Monthly { day: 20, at: 10:00 }   ← domain/reminder.rs
                    │
                    │  Reminder { id: IdGenerator::generate(), owner, text,
                    │             recurrence, timezone, … }
                    │  reschedule(now) → next_fire_at
                    │  None ⇒ ReminderError::NeverFires, refused before storing
                    ▼
            ReminderRepository::put()             → SqliteReminderRepository
                    │
                    ▼
            text::created(&reminder)              ← serana-app/text.rs
            "Set: … / Schedule: … /                read back in words,
             Next: … / id: …"                       never as a cron expression
```

Everything the user reads comes out of `serana-app/text.rs`, including the errors:
`ReminderError` is mapped in `respond.rs` so a provider outage and a request the model
could not understand do not produce the same sentence.

## Delivering one

No model, no frontend command — just the bot's background task.

```
  serana-tg/main.rs
     │  tokio::spawn(scheduler.run(tick_interval))      every 60s by default
     ▼
  SchedulerService::tick()                    ← services/reminders/scheduler.rs
     │
     │  Clock::now()
     ▼
  ReminderRepository::due_at(now)             → SELECT … WHERE next_fire_at <= ?
     │                                          one indexed lookup; `next_fire_at` is
     │                                          denormalised onto the row so the tick
     │                                          never evaluates every recurrence
     │  for each due reminder:
     ▼
  Notifier::notify(owner, text)               → TelegramNotifier    ← serana-tg/notifier.rs
     │
     ├── Ok               → mark_fired(now), recompute next_fire_at, put()
     ├── Unreachable      → next_fire_at = None, put()    blocked the bot: stop paying
     │                                                    for a delivery that can't land
     └── Transport        → leave it untouched            still due on the next tick
```

Two decisions worth knowing:

- **Downtime collapses.** A reminder that came due five times while the process was down is
  delivered *once* and rescheduled from the present, not replayed. `next_occurrence_after`
  is computed from `now`, never from the moment that was missed.
- **The REPL does not tick.** `serana-cli` wires `build_reminders` but never
  `build_scheduler`. Two schedulers over one database would deliver everything twice, so
  the CLI manages reminders and the bot delivers them — which is why `SERANA_USER_ID` has
  no default: a reminder created in the terminal is owned by a Telegram id, or it is owned
  by nobody and never arrives.

## Where the ports get their real implementations

One place, `serana-app/wiring.rs`:

| Port (domain) | Production (adapters) | Tests (testkit) |
| --- | --- | --- |
| `LlmProvider` | `OpenAiProvider` | `ScriptedLlm` |
| `ReminderRepository` | `SqliteReminderRepository` | `InMemoryReminderRepository` |
| `Notifier` | `TelegramNotifier` (in `serana-tg`) | `RecordingNotifier` |
| `Clock` | `SystemClock` | `FixedClock` |
| `IdGenerator` | `RandomIds` | `SequentialIds` |

The right-hand column is why `cargo test` needs no API key, no database and no network, and
why no test sleeps: the scheduler's tests move a `FixedClock` instead of waiting.
