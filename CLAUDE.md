# OMuse: notes for Claude Code

Read `AGENTS.md` first: it has the release steps, the test commands and the repository rules.

## How every task works: converse → one JSON → one code tool → the user's link

The user's rule (2026-11-03), for EVERY task — a purchase, a claim, a refund, an application, a booking:

1. **Converse.** The model collects what the task needs: from the user's words, their email, their profile and the
   files of the chat. It asks once, in one message, for everything still missing — required inputs (a policy number,
   the documents the site itself requires) and optional ones (documents the user may skip and add later). Every
   decision the conversation settles (the entry mode, the plan, the claim type) becomes a JSON field.
2. **One JSON, one code tool.** The model calls ONE code tool with all of it as structured JSON. After the one
   question, the tool is always called with whatever the user gave — a reply to the tool's question owes the call.
3. **The code tool does the whole process.** It calls the site's API or walks its pages with form functions mapped
   from the real site (walk first, then code), in the user's place, to the end: it uploads the documents, presses the
   site's own buttons and submits where the submission is what creates the reference — no takeover of the server's
   browser. Where the user must approve what is sent (a payment, an insurance claim), the app asks once
   (`purchase_confirm` / `submit_confirm`), then code submits.
4. **JSON back.** The tool returns structured JSON: the status, the reference number, the link that opens the
   session on any device (a saved quote, a saved application, the claim), and what is still missing for the user to
   add there.
5. **The model answers.** It words the reply from that JSON; code appends the facts (`show`): the link, the reference,
   what is still the user's.

Tests on real sites with clearly fake data stop before a submission that would reach a real business (an insurer,
an airline): the product submits; the test declines the approval or stops at the last step.

## Fix in code, not in instructions

The workflow is as deterministic as possible. The model is used to understand what the user wants, NOT to decide what
to do. When you fix a problem, always prefer a fix in code over a change to a skill, a bot file or a prompt:

- Anything that data or state decides is decided in code: defaults (an arrival card's departure date, its Yes / No
  answers), dates and deadlines, amounts, links, fees, which page is handed over and when, what an answer must
  contain (a reference number, a link, a fee), input validation and argument fixes.
- Use the mechanisms already there: argument fixes where tool calls are parsed, Sentinel policy, a runtime step done
  in code (the automatic hand-over), the result's `show` items that code appends to the answer, a one-time note or hint, and
  structured data (the card catalog's `fx_fee_pct`). See KNOWLEDGE.md, "Guarantees in code".
- Only the hand-over (the final pay / submit / confirm click is the user's) and secrets (Vault values, passwords) are
  hard stops; other checks fix the input or add a note and let the action run.
- One exception to the hand-over, decided by the user (2026-11-03): a purchase through Shopify's UCP checkout API may be
  completed by code when the user's card is in the Vault, the API accepts it in a supported way, and the user approved
  the purchase (item, store, total) in the app's purchase approval. Without a usable card the user gets the checkout's
  continue_url to pay in their own browser.
- A second exception, decided by the user (2026-11-03): FWD Singapore's online travel claim is filled and submitted
  by code once the user approved it in the app (`submit_confirm`: the facts sent and FWD's declaration; never
  auto-approved). Every other Pay / Submit stays the user's click.
- Change instruction text only for what genuinely needs judgement, and when you do, check that code does not already
  decide it differently (skills must not contradict code).
- Every fix gets a unit test that reproduces the failure, and the real-model suite that found it is rerun.
- When a test fails, first decide whether the test is needed at all (see AGENTS.md, "Tests"): a check on the model's
  optional wording, on pages or ids we invented, or a duplicate is removed, not fixed for.

## Natural language is the model's; code tools work only on JSON

**Hard rule.** All natural language is processed by the model into structured JSON data, and code tools use only that
JSON data:

- **Input:** the model reads the user's words (and any other natural language: mails, pages, replies) and turns them
  into the code tool's JSON arguments. Code never parses natural language: no regexes or keyword lists over the user's
  wording to decide a step or read a value. Code may validate the JSON it receives (a date parses, a city is in the
  table, an address is well-formed) and fix or drop a bad field.
- **The code tool:** works only on its JSON input and returns structured JSON output. A code tool never calls the
  model.
- **Output:** the model interprets the code tool's JSON output and writes what the user reads.
- **Language vs. pages:** natural language (the user's words, mail text, a page's prose, the model's own replies) is
  processed only by the model. A known website and its HTML is processed by code: form functions with fixed input ids,
  the fixed parts of a known result or confirmation page, structured API and tool results. Regex cannot understand
  natural language; every new phrasing breaks it.
- **Walk the whole process before coding it.** Before writing or changing a form function or any code tool that drives
  a real site, go through the ENTIRE real process yourself, screen by screen, with clearly fake data, up to the step
  that is the user's (Submit, Pay, sign-in, a photo, a code you cannot read). Record every screen: its URL, every
  input's id/name, required or not, its option list, its validation, and what enables Next. Code the tool only from
  that complete map, and keep the map with the code. Never discover fields one failed run at a time.
- **Reading a page: HTML elements first, then the model.** Data on a page is read by deterministic rules on its HTML
  elements first — a selector, an element id, a table's cells, an attribute (the browser service reads them).
  When no element rule can find it (an unknown site, data only in running text), the model extracts it from the page or
  text into JSON. Use as little regex as possible: regex is for checking a value that is already structured (an ISO
  date, an email's shape, a reference's format), not for finding data in a page's text.

The design that implements this rule (the workflow tools, the protocol in the run loop, `show` — the facts the user
must read, appended to the answer by code — and the optional `hints`, how to add a machine) is `docs/json-tools.md`:
read it before changing a skill, a code machine or the run loop.

## Skills: code decides, the model chooses from JSON, prose only for the user

In each skill, use code tools as much as possible — a web form is filled by a form function per URL, a mail search is
a fixed query, a default, a deadline, a window, a file name, a schedule and a delivery are computed. A skill should
need **no model call at all** to decide what happens next (see `agent/driver.rs` for the arrival cards: the model only
understands the request and loads the skill; code runs every step).

When a decision genuinely needs the model, do not ask it in prose and read its prose back. Send it **JSON with the
possible choices** and have it return one of them (a decision API: `{"choice": "A"}` out of the listed options). This
holds even when the input is a picture or a page: the possible decisions are listed in JSON for the model to choose
from (what field, which option, which button — never "what should I do next?").

**Hard rule: no natural language from the model anywhere in the decision flow** — not as an instruction to it (no "(System)" hints), not as an input read back to decide a step. **Form filling is code with fixed input ids, always.** Every value typed, selected, ticked or uploaded into a web
page comes from a form function with hard-coded selectors (`crates/omuse-runtime/src/forms.rs`), run by a code machine
(`agent/driver.rs`, `agent/claims.rs`, …). The model never fills an input, and code never guesses a field from its
label. A site without a form function is the one exception: code opens the page and passes control to the `browser-use`
skill, the only place the model may use the browser input tools (they are withdrawn from the model everywhere else,
in code). That path is not e2e-tested, since its results vary with every try; the final Submit / Pay is still the
user's. Any skill that fills or submits a known site's form must be driven by a code machine, whatever its front
matter says.

A judgement the model must make goes through `decide(question, choices)`: the question and a JSON array of choices go
in, exactly one choice must come out — anything else gets one retry, then the default (the most common or safe
choice); code acts on the choice. A form workflow starts only when every input value is known (collected from the
user's data, computed as a default, or asked once *before* the form opens); nothing is asked in the middle depending
on the form's response — the only pauses are the user's own steps (sign-in, emailed code, CAPTCHA, photo, Submit/Pay).

Natural language the model writes is used for ONE thing: the reply the user reads. Anything in a test that checks
the model's words to decide the next step marks a step that belongs in code (or in a JSON choice). Every fact the user
must read (a reference, a link, an amount, a deadline) is a `show` item that code appends after the reply — never
something the model must copy, and no review enforces facts in the prose; tests check the appended block, never the
model's wording. The final natural-language answer is reviewed once by the model, as the user would read it, only for
coherence and the language rule (`Runtime::review_answer`).

## Repository: second-state/omuse only

This repository is `second-state/omuse`, a fork of `Drlucaslu/locius`. All future commits, pushes, tags, issues and
pull requests go to `second-state/omuse`. Never send anything upstream: no push, pull request, issue or comment on
`Drlucaslu/locius`.

- Push only to `origin` (`https://github.com/second-state/omuse`).
- `gh` targets the parent repository of a fork by default, so always pass the repository explicitly:
  `gh pr create --repo second-state/omuse --base main …`, and `--repo second-state/omuse` for every other `gh pr`,
  `gh issue` and `gh release` command.
- Check the target before pushing or opening a PR (`git remote -v`).
- Commit and push only when the user asks; merge a PR only when the user says to merge it.
