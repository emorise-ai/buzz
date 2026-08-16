# Handoff — test the sandbox system before it goes to review

**Written 2026-08-16.** The feature is built and committed on
`feat/agent-sandboxes` (one commit, `b85d64fda`). It is **not** pushed, and no
PR exists. That is deliberate: this needs real end-to-end tests first.

Read `HANDOFF.md` for the operational state (what runs where, the gotchas) and
`SANDBOX-PLAN.md` for the design. This file is only about what to test and how.

---

## The job

Two things, in this order:

1. **Exercise the whole system by hand once**, end to end, and record what
   actually happens. Not "the code looks right" — drive it and watch.
2. **Turn that into automated E2E tests** that live in the repo and run against
   a real relay and a real Docker host.

Only then push and request review.

The reason for the order: manual first tells you what the tests should assert.
Writing the tests first tends to encode what you *hoped* happens.

---

## What has and has not been proven

**Already verified live** (during the build, ad hoc, not automated):

- A sandbox is created via provider → broker → container, with CPU, memory,
  PID, and capability limits enforced *inside* the container.
- The TTL reaper destroys an expired sandbox and publishes an event.
- The desktop viewer loads from a laptop with a working input client; the agent
  drives the same Chromium a human sees.
- The six browser tools work through the real MCP protocol against a live site.
- Claude Code authenticates in a sandbox from a transferred session and returns
  a real inference result.
- Both lifecycle events (48200 / 48201) are accepted by the relay.

**Never proven, and the reason this is not review-ready:**

- **An agent has never replied to a message.** It connects, goes online, and
  sits there — it belongs to no channel, so it has nothing to answer. The whole
  point of the system is untested.
- No test asserts any of the above. Every check was a one-off shell command in
  a session that is now gone.
- The provider→broker path has not been exercised since the auth change. The
  provider now signs with the agent's key (it previously sent a bearer token);
  that code path compiles and is unit-tested but has not run against the live
  broker.

That last one is the highest-risk gap. **Test it first.**

---

## Manual pass — do this before writing any test

Infrastructure is already running on Matterhorn (`ssh -p 2222
root@matterhorn.emorise.com`); see `HANDOFF.md` for what is where.

1. **Provider → broker with the new signing.** Build
   `cargo build --release -p buzz-backend-docker`, tunnel the broker
   (`ssh -p 2222 -L 19310:127.0.0.1:9310 …`), and feed the provider a deploy
   payload on stdin with `provider_config.broker_url` pointing at the tunnel.
   Expect `{"ok":true,"agent_id":"…"}`. If this fails, the signing change is
   broken and nothing downstream matters.
2. **Agent joins a channel and replies.** Create a channel on the staging relay,
   add the agent's pubkey as a member, post a message mentioning it, and wait.
   `buzz-cli` has `channels` and `messages` subcommands;
   `crates/buzz-cli/TESTING.md` is the runbook. This is the missing proof.
3. **The browser handoff, as a human would do it.** Deploy with image
   `buzz-sprig-claude`, tunnel port 6080, open the viewer, and confirm you can
   click and type in the agent's browser while it is running.
4. **Failure paths.** Kill the relay and confirm the broker refuses to create
   (fail-closed). Ask for an unlisted image. Ask for 64 CPUs and confirm it is
   clamped rather than refused.

Write down what actually happened, including anything surprising. That list is
the test plan.

---

## The E2E tests to build

Put relay-facing tests in `crates/buzz-test-client/tests/`, following the
existing convention exactly — read `e2e_managed_agent.rs` first, it is the
closest analogue:

- Every test `#[ignore]`d so `cargo test` stays green without infrastructure.
- Relay URL from `RELAY_URL`, defaulting to `ws://localhost:3000`.
- Run with `cargo test --test <name> -- --ignored`.
- A module doc comment listing what the file asserts and how to run it.

### `e2e_sandbox_events.rs` — relay-only, no Docker needed

Cheapest to write and the easiest to run in CI later.

- A kind:48200 event with the full tag set is accepted and queryable by its
  `d` tag (the sandbox id).
- A kind:48201 event is accepted and carries its `reason`.
- The `p` tag makes a sandbox findable by the agent it belongs to — this is what
  a future "this agent has a computer" card would filter on.
- **A guard against the trap that cost real time:** assert both kinds are
  accepted, so if someone adds a kind to `buzz-core` without the write-scope
  mapping in `crates/buzz-relay/src/handlers/ingest.rs`, this test fails with a
  clear message rather than the feature silently not working.

### `e2e_sandbox_broker.rs` — needs a Docker host

Gate on a `BUZZ_SANDBOX_BROKER_URL` env var; skip cleanly when unset.

- Unsigned request → 401. Forged signature → 401. Both matter: they are the
  whole security boundary.
- A validly signed request from a non-member → 403 (only if the relay requires
  membership; on an open relay it succeeds — assert whichever the relay under
  test is configured for, and say so in the doc comment).
- Create → the container exists with the limits actually applied. Read them back
  from Docker rather than trusting the response body.
- Delete → gone, and a 48201 event lands on the relay.
- A short TTL → the reaper removes it and publishes. Keep the TTL at the 60s
  floor so the test is bounded.
- Ask for an unlisted image → 400 before anything is created.
- **Always clean up in a teardown**, including on failure. A leaked sandbox on a
  shared host is a real cost — Matterhorn runs production sites.

### `e2e_sandbox_agent.rs` — the one that matters

This is the test that proves the product works, and the hardest to write. Do it
last, once the manual pass has shown the shape.

- Deploy an agent into a sandbox through the provider.
- It appears online on the relay (kind:20001 presence).
- Add it to a channel, mention it, and **assert it replies**.
- Shut it down and confirm presence goes offline and the sandbox is reaped.

Expect this to be slow (a minute or more) and to need generous timeouts. That is
acceptable for an `--ignored` test; do not try to make it fast.

### Unit-level gaps worth closing while you are there

- `buzz-sandbox-broker` has 29 tests against ~1,800 lines; the Kubernetes
  binding runs ~154 against ~6,300. The thin areas are the reaper's expiry
  decision and `summarize`'s label parsing.
- Nothing tests that the provider's signed request is one the broker's verifier
  accepts. A test constructing a request with the provider's `sign_request` and
  verifying it with the broker's `Verifier` would have caught the bearer-token
  mismatch found during cleanup, and is worth adding even though both sides are
  now correct.

---

## Definition of done

- The manual pass is complete and its findings recorded.
- An agent has demonstrably replied to a message from inside a sandbox.
- The three E2E files exist, pass against Matterhorn, and are `#[ignore]`d.
- `just ci` is green.
- No sandboxes left running on the host afterwards.
- `HANDOFF.md` updated so its "verified" list points at tests rather than
  memories of shell commands.

Then push `feat/agent-sandboxes` and open the PR. Two things a maintainer will
reasonably want to agree before merge:

- **The kind numbers.** 48200/48201 claim slots in Buzz's shared registry.
- **The duplicated `env.rs`/`wire.rs`** between the Kubernetes and Docker
  providers — deliberate, documented, but a maintainer may prefer extraction
  into a shared crate first.

---

## Working notes

- Run `. ./bin/activate-hermit` before any git command, or the pre-commit hooks
  fail with `just: command not found`.
- `just ci` is the full gate. `just test` needs Postgres and Redis running.
- Commits need `-s` (DCO), and the branch targets **`main`** — `block/buzz` has
  no `dev` branch, unlike Ron's other repos.
- Ron does not read code. Report what a user can now do, what to click to see
  it, and what broke — not implementation detail. Always close with one concrete
  next step.
