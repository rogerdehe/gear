# Field notes: the user's BMAD experience, and bloat under never-stop

Input for the overall retrospective (the user's experience plus this project's). **Not a conclusion and not a rule** — the user asked to record it now and decide together later.

## The user's experience (2026-10-23, paraphrased faithfully)

- BMAD suits mature products that need high rigour: very detailed, blind, deliberately fault-finding reviews.
- For ordinary development, is that rigour worth it early on? The big direction must be held — a wrong direction turns small problems into big ones — but not every detail deserves focus.
- A project cut into 100+ stories moved very slowly; around story 20–30 the stories started "fighting": one story said do it this way, another that way. Not a lack of ability — the frontier agents think so deeply that each, looking only at its own part, finds many problems; one step in, they reason about the next 1000 steps. Reviews turned into an ever denser mutual fault-finding; problems and the project inflated without bound.
- The way out was removing the review chain and finishing all 100+ stories — but then nobody knew how many technical problems were inside, and a full retrospective of 100+ stories was too big to attempt.
- The agents love writing code: a not-very-complex project reached ~400k lines of code plus ~800–900k lines of docs and comments. The agents lost focus and expanded wildly, ignoring "skeleton first, then flesh".
- This does not prove BMAD is wrong — many people use it and it has proven value; it may be how it was used.
- **The risk for GEAR:** never-stop makes parallel agents work continuously — they may keep "dumping" output into the project: always busy, little value; hundreds of thousands of lines of which only a few tens of thousands are useful. Never-stop could then do harm. This has to be considered while building GEAR.
- Open for the user: change the approach, or keep it and tighten the review's scope when assigning work?

## Matching signals in this project so far (observations only)

| Signal | Here |
|---|---|
| Reviewers reason far ahead | T-5 round 2: three new P1s, each a stack of coincidences (crash at an exact point + agent exit while the engine is down; cancel during start; quota lowered mid-tick). Partly invited by the captain's prompt ("a restart between any two steps"). Captain passed T-5 and logged them. |
| Review rounds that do not converge | T-5 went fail → fail with new findings each round; risk of an endless loop that blocks the lane's flow. |
| Size versus budget | 2026-10-23 on dev: `crates/*/src` 12,135 lines (incl. inline unit tests and comments), `crates/*/tests` 6,178, Markdown (docs, skills, ops) 11,371. MVP product budget was ≤3,000; the user had already stopped growth once at ~7,800. |
| Docs grow with every incident | lessons L-1…L-45, field notes, SOPs — the captain adds text after each event (the user already asked for subtraction). |
| Value per line is not measured | never-stop is measured by idle time and rework rate; nothing yet measures whether the output is worth its size. |

## Questions for the retrospective (no answers yet)

- How does review stay on direction and real defects, not on the next 1000 steps? (scope of review per task; severity calibration; a cap on rounds; who decides "good enough for the skeleton")
- How do we keep never-stop from becoming never-stop *growth*? (a size/value signal per task or per lane; skeleton-first gate; deletion as valued work)
- How do many parallel tasks avoid contradicting each other? (one owner of each decision; contracts from the captain, not from each story)
- How do we keep a retrospective possible at all as the project grows?
