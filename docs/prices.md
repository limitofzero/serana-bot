# The price digest

`/prices` is the third subject on the shared conversation loop (see `docs/calendar.md` for
the loop itself). It quotes a watchlist once a day, caches what it read, and lets the
schedule be changed by just saying so.

## Shape

```
serana-domain/prices/        AssetId · Quote · Snapshot · Digest
                             PriceSource · DigestRepository · DigestNotifier
serana-adapters/prices/      CoinGecko (coins/markets) · DefiLlama · Chain
serana-adapters/digests.rs   SqliteDigestRepository — one JSON row per owner
serana-services/prices/      PriceService · Watching (Capability) · DigestScheduler
serana-services/schedule.rs  ParsedSchedule — shared with reminders
serana-app/text/prices.rs    everything the person reads
```

## One standing order per person

A `Digest` is keyed by owner, so no tool takes an id — there is nothing to disambiguate. It
holds the watchlist, a `Recurrence`, a zone, `next_fire_at`, and the last `Snapshot` read.

The schedule is the same `Recurrence` a reminder uses, and the model fills it with the same
`ParsedSchedule` shape. That was extracted from the reminder parser rather than copied:
"every day at 9", "weekdays at 8" and "the 1st of the month" are one vocabulary, and two
copies of its validation would be two places for "the 31st" to behave differently.

It is created the first time the person uses `/prices`, never at startup. A daily message
nobody asked for is the one thing an unprompted push must not be. The prompt's context line
reads the stored digest *without* creating one, so merely asking a question does not start it.

## Cache

The scheduled run reads the market, sends, and stores the snapshot. `/prices` answers from
that snapshot — no request — unless:

- the person asks for something current ("refresh", "right now"), which the model passes as
  `fresh: true`; or
- the snapshot is older than a day, meaning a scheduled run was missed.

If a read fails and something is cached, the cached prices are shown with their real age
rather than an error. Only a digest with nothing at all to show fails.

The prices are deliberately **not** in the model's context line — only how many minutes old
they are. A number in the context goes stale the moment the next digest lands, and a model
quoting yesterday's price as today's is worse than one that says nothing. The system prompt
also forbids it from stating a price itself, or giving any buy/sell/hold view.

## Sources

| | key | ticker | 24h move | role |
| --- | --- | --- | --- | --- |
| CoinGecko `coins/markets` | optional | ✅ | ✅ | first choice |
| DefiLlama `coins.llama.fi` | none | ✅ | ✗ | fallback |

`Chain` is itself a `PriceSource`, so nothing above it knows there are two. It treats an
empty answer as a failure of that source, not an empty market, and falls through. A digest
served by DefiLlama simply has no ▲/▼ column — a missing move renders as nothing, never as
`0.00%`, because a flat market and an unknown one must not look the same.

The keyless CoinGecko route shares a rate limit with everyone on the same egress IP; its
failure mode is 429. `SERANA_COINGECKO_API_KEY` (a free Demo key) lifts it.

## Delivery

`DigestScheduler` is a second loop beside the reminder scheduler, on the same tick interval.
Not a generalisation of it: they share a shape and nothing else — different store, different
port, different recovery — and one abstraction over both is worth building at the third, not
the second.

- Market unreadable → nothing sent, left due, retried next tick. A one-minute rate limit
  must not cost the day's digest.
- Recipient blocked the bot → switched off; the read prices are kept.
- Transient send failure → left due; the read prices are kept so the retry does not pay
  for the market twice.
- A week of downtime → one message, then rescheduled from now.

`f64` for prices is deliberate: nothing here settles, sums or signs. The only operation is
rendering to ~6 significant figures ($84,753 · $2,690.38 · $0.1603).
