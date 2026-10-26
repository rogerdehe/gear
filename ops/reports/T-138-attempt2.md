# T-138 — `read_quota`, the quota of the providers, attempt 2

Scope: T-138 attempt 2 — the review of attempt 1: F1, the HTTP status of the
answer.

Branch: `deck/t138` (the same branch as attempt 1, per the conventions; code @
the hand-in commit; `origin/dev` merged before verification)

## F1 — the HTTP status of the answer was thrown away

Curl is happy with a 401, a 429 and a 500: it writes the body and exits 0,
so the reader parsed the body of an error as if it were the answer of the
provider — the reason of the error was lost (`the response of minimax has no
model_remains` instead of "401 unauthorized").

The call now asks curl for the status as well:

```text
curl -sS --max-time 15 -w '\n%{http_code}' -H @- <url>
```

The status is the last line of the answer (`-w` writes it there, after a
newline and without one of its own). The reader splits it off:
- a status of **400 or more** is an error before the body is parsed:
  `Err("<reader>: HTTP <status>: <the first 200 characters of the body>")` —
  the reader's name, the status and what the provider said;
- a **missing or non-numeric status** is an error too (the answer is not one
  of ours);
- a status below 400 gives the body to the parser, as before.

The key still goes in through standard input (`-H @-`), never on the command
line.

## Tests

The fake curl now prints the answer **as real curl does**: the body and the
status on its last line. On top of that:
- `an_http_error_gives_the_status_and_the_body` — a 401, a 429 and a 500 with
  the body of the provider: each ends in an `Err` that names the status, the
  reader and the body;
- `an_answer_without_a_status_is_an_error` — a fake that prints the body and
  no status: the reader says so;
- `a_call_that_fails_gives_the_reason` — the call that fails itself (curl's
  own error, a timeout) keeps its reason;
- the 200 answers parse as before.

The shape of the answer was checked against the real curl as well: a local
server that answers 401 gives `{"error":"unauthorized"}\n401` on the standard
output — the body, a newline and the status — which is what the reader
expects.

## Verification

- `cargo test -p gear-usage` → **31 passed, 0 failed**.
- `cargo build --workspace --all-targets` → clean, **no warning**.
- `scripts/test.sh` → **1000 tests run, 1000 passed, 0 skipped**.
- The tests over 10 s that the suite warns about are the machine (the load
  average was 62–74 while the suite ran): the git-heavy engine tests, which
  this branch does not touch.

## Open questions

None.

## Out of scope

- The policy of §9.2 (the quota stop, the park, the five-minute wait): the
  engine's `quota.rs` (T-148). The crate only reads.
- The configuration of the sources (`[[quota.sources]]`, the key file and the
  key path): gear-app's config (T-147).
- The readings in the store (`meta quota.reading.<provider>`) and their age:
  the engine (T-148).
