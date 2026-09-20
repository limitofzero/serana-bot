# Notes from the Python reference (`~/hermes-agent`)

Findings from reading the upstream implementation, and what each one means for serana.
Data, not doctrine: upstream solved these under constraints we do not all share. Where we
diverge, the reason is recorded here.

Scale for calibration: ~6.6k Python files, ~790k lines. `agent/` alone is ~250 modules.

## 1. A turn is a pipeline, not a loop body

`agent/AGENTS.md` states the loop in ~10 lines, then lists ~20 sibling modules that each own one
phase: preflight, iteration prep, request assembly, api call, api error, response intake, response
check, empty response, tool round, tool validation, overflow, truncation, context compaction,
recovery, retry state, stop gates, liveness, usage, final response, finalizer, summary.

That list is a map of everything that goes wrong in production. The naive `while` loop is correct
and useless.

**For serana:** `AgentService::run_turn` is a sequence of named, individually testable steps, not
one function. Start with request assembly → api call → response intake → tool round → finalize, and
add phases as we hit the problems. Do not flatten them into one `loop {}` body "for now" — the
seams are the whole point, and the layering rule already forbids it.

## 2. The prompt-caching invariant

Upstream's hardest rule: the system prompt is **byte-stable for the life of a conversation**. Never
alter past context, swap toolsets, reload memory, or rebuild the system prompt mid-conversation.
Anything that must be injected mid-conversation rides a user message or a tool result instead.
Compression is the single sanctioned cache break.

This is easy to violate by accident — a "helpful" re-render of the system prompt with the current
time in it silently doubles the bill on every turn.

**For serana:** the system prompt is built once, at conversation construction, and stored as an
immutable `Arc<str>` on the conversation. `ConversationRepository` persists it. No API takes a
`&mut` to it. Injection points are explicit and typed.

## 3. Strict role alternation

Never two same-role messages in a row; never a synthetic user message mid-loop. The one legal
three-step is `assistant(tool_calls) → tool → user`. Abort paths must *close an open tool tail*
before returning, or the next turn starts `tool → user` and the provider rejects it.

**For serana:** enforce this in the type system, not in review. A `Conversation` that only accepts
appends through methods encoding the legal transitions, with `#[must_use]` on the tool-tail closer.
A unit test asserting alternation over a fuzzed append sequence.

## 4. Tool-call validation is where models misbehave

From `agent/turn_tool_validation.py`, the failure modes upstream had to handle:

- **Duplicate `tool_call_id`s** in one batch — must be uniquified before anything downstream reads
  them, because the pre-API sanitizer keeps only the first call/result per id.
- **Hallucinated tool names** — fuzzy-repaired against known names first, then 3 strikes and a
  partial exit. Strikes only advance when a turn has *no* valid call, so a degenerate model still
  halts.
- **Mixed batches** (some valid, some unknown) — error-result only the invalid calls and run the
  valid ones. Voiding the whole turn discards real work.
- **Truncated arguments** — routers rewrite `length` → `tool_calls`, so args cut off mid-stream are
  refused outright rather than retried.
- **Every `tool_call` must get exactly one matching `tool` result**, including on every error path.

**For serana:** this is the first thing after the loop itself, and it is unit-test territory
before it is code. `ToolRound` returns a verdict enum (`Dispatch | Reissue | PartialExit`) rather
than booleans. Schema validation happens before dispatch; a validation failure becomes a tool-role
error result fed back to the model, never a hard error up the stack.

## 5. Registries, never `if name == ...`

Upstream is emphatic in both `agent/AGENTS.md` and `tools/AGENTS.md`: tools register into a table at
import time; agent-level tools are intercepted through an `INLINE_TOOL_EXECUTORS` table; adding a
backend means a new entry, "never an `elif` on a backend name".

**For serana:** `ToolRegistry` is a `HashMap<String, Arc<dyn Tool>>` built at the composition root.
Rust has no import-time side effects, and that is a feature — the binaries wire the registry
explicitly, so the tool set of any agent is readable in one place.

## 6. Compression is two-stage, and the cheap stage comes first

`ContextCompressor` prunes old tool results first — **no LLM call** — then picks boundaries, then
generates a structured summary with a separate, cheaper auxiliary model. Thresholds are layered:
~50% at the agent, ~85% at the gateway. There is a failure cooldown after a provider-proven
overflow, and a deterministic fallback summary when the summarising stream stalls twice.

**For serana:** when compaction lands, the free pruning pass is its own step and ships first. The
summarising model is a separate `LlmProvider` instance, configured independently — do not assume
one provider per process.

## 7. Capability is a property of the session, not the process

Toolsets are enabled per platform/surface (`cli`, `telegram`, ...), so the same binary exposes
different tools depending on where the turn came from. Availability probes are cached with a TTL
and must never make network calls.

**For serana:** `ToolRegistry` is built per session from a surface descriptor, not once globally.
This matters immediately, because we have two frontends from day one and the Telegram surface
should not get the same tools as the local CLI.

## 8. Smaller things worth stealing

- All tool handlers return a JSON string. Uniform, greppable, trivially serialisable into a tool
  result. We will do the same: `Tool::invoke -> Result<String, ToolError>`.
- Tool schema descriptions must not name tools from other toolsets — the referenced tool may be
  absent for this session and the model will hallucinate a call to it. Cross-references get injected
  dynamically at definition time instead.
- No `offset`/`limit` on instructional tools (skills, prompts): models read page one and skip the
  rest.
- Interrupt checks live inside the loop, and the iteration budget is separate from the iteration
  count (upstream default: 500 iterations, shared with subagents).

## Where we deliberately diverge

- **Module granularity.** Upstream's ~250 flat `agent/*.py` modules exist because Python has no
  compile-time layering. We get the same separation from crate boundaries plus modules, and our
  dependency rule is enforced by cargo. We do not need `turn_*.py` × 20 to achieve it.
- **Mixin-assembled god object.** `AIAgent` takes ~60 constructor parameters. We take ports and a
  config struct; anything approaching that parameter count is a design failure, not a milestone.
- **Process globals.** `_last_resolved_tool_names` is a process-global that subagent execution
  saves and restores around child runs, and upstream documents that readers may see it stale. We
  pass state explicitly or put it behind a port.
