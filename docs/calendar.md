# The calendar, and the shape that made room for it

`/calendar` is the second thing the assistant can be asked about, and adding it is what
turned "the reminder conversation" into "a conversation, about something".

## One loop, two subjects

Everything that is hard about a turn is the same whatever the turn is about: loading the
history, keeping the roles alternating, making sure every tool call gets exactly one
result, compacting when the prompt grows, never letting the system prompt move. Those
invariants are cheap to get right once and expensive to get wrong twice, so there is one
loop and the subjects plug into it.

```
serana-services/
├── chat/
│   ├── capability.rs      trait Capability: instructions, tools, run, summarise
│   ├── session.rs         Chat<K, L, V>: load → ask → dispatch → record → save
│   └── config.rs          ChatConfig: model, zone, compaction threshold
├── reminders/
│   ├── capability.rs      Reminding<R, C, I>: impl Capability
│   └── turn.rs            ChatService — the loop with reminders fitted
└── calendar/
    ├── capability.rs      Planning<P, C>: impl Capability
    ├── service.rs         CalendarService — the lifecycle, no model
    └── mod.rs             CalendarChat — the loop with the calendar fitted
```

A `Capability` supplies the five things that differ:

| | reminders | calendar |
| --- | --- | --- |
| `topic()` | `reminders` | `calendar` |
| `instructions()` | how to read a schedule | how to read an appointment |
| `tools()` | create / update / delete / acknowledge / complete | list / create / cancel / delete |
| `context()` | the local time + every reminder | the local time + the week ahead |
| `run()` | → `ReminderOutcome` | → `CalendarOutcome` |

`topic()` scopes the stored conversation: `tg:42` becomes `tg:42:reminders` and
`tg:42:calendar`. They cannot share one, because a conversation stores the system prompt it
was opened with and never changes it — sharing an id would mean whichever spoke first
deciding what the other one is, with a toolset that no longer matches its instructions.

## Cancel is not delete

The distinction the whole calendar design turns on:

| | what it does | when |
| --- | --- | --- |
| `strike_off` | the occurrence disappears from the day; a series keeps its other occurrences | the event is ours |
| `decline` | it stays on the calendar, marked as not attending | it is somebody else's invitation |
| `delete` | gone, irreversibly | only after the person has confirmed, in a previous message |

Which of the first two happens is decided from `CalendarEvent::mine`, in the domain — not by
the model, which only ever says "cancel this one". Removing another person's event from
their own invitation is not ours to do, and a model that guessed would sometimes do it.

Deleting is the one irreversible action, and it is gated by the conversation rather than by
a flag: the model is told to say what would be deleted and ask, and to call the tool only
once the answer has come back. That works because the answer is the next message and the
exchange is still in front of it.

## Travel

An appointment someone has to physically get to blocks more than itself. `NewEvent` carries
the appointment's own hours and a `travel` duration; `blocked()` widens the span at both
ends, and that is what reaches Google. So "am I free at 14:45?" answers correctly while the
person is still on the road — which is the whole reason it is reserved rather than merely
noted in the description.

The model sets `in_person`; the duration comes from `SERANA_TRAVEL_MINUTES`. The
confirmation shows both spans, because a block that silently starts half an hour early looks
like a bug the next time they open their calendar.

## Where a bare message goes

The assistant now keeps two conversations, so "yes" has to be aimed at one of them.

- A slash command says where it is going.
- A plain message continues whatever that person was last talking about (`Topics`, in
  memory — losing it on a restart costs one message).
- A **reply** is aimed at the conversation the quoted message came from, which need not be
  the last one used. Every message that names something carries its subject's id marker
  (`🆔` for a reminder, `📆` for an event), and `text::topic_of` reads it. That is what
  keeps a reply to a reminder the scheduler pushed with reminders, even mid-way through
  arranging something.

## Untrusted by construction

Titles, locations and descriptions on a calendar were written by whoever created the event,
which is usually not the person using this. They reach the model as data — the system prompt
says so explicitly — and nothing in `serana-app/src/text/calendar.rs` does anything with
them but echo them back exactly as they came.
