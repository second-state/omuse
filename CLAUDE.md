# OMuse: notes for Claude Code

Read `AGENTS.md` first: it has the release steps, the test commands and the repository rules.

## Fix in code, not in instructions

The workflow is as deterministic as possible. The model is used to understand what the user wants, NOT to decide what
to do. When you fix a problem, always prefer a fix in code over a change to a skill, a bot file or a prompt:

- Anything that data or state decides is decided in code: defaults (an arrival card's departure date, its Yes / No
  answers), dates and deadlines, amounts, links, fees, which page is handed over and when, what an answer must
  contain (a reference number, a link, a fee), input validation and argument fixes.
- Use the mechanisms already there: argument fixes where tool calls are parsed, Sentinel policy, a runtime step done
  in code (the automatic hand-over), a final-answer guard that adds what is missing, a one-time note or hint, and
  structured data (the card catalog's `fx_fee_pct`). See KNOWLEDGE.md, "Guarantees in code".
- Only the hand-over (the final pay / submit / confirm click is the user's) and secrets (Vault values, passwords) are
  hard stops; other checks fix the input or add a note and let the action run.
- Change instruction text only for what genuinely needs judgement, and when you do, check that code does not already
  decide it differently (skills must not contradict code).
- Every fix gets a unit test that reproduces the failure, and the real-model suite that found it is rerun.

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
- **Reading a page: HTML elements first, then the model.** Data on a page is read by deterministic rules on its HTML
  elements first — a selector, an element id, a table's cells, an attribute (the browser service reads them).
  When no element rule can find it (an unknown site, data only in running text), the model extracts it from the page or
  text into JSON. Use as little regex as possible: regex is for checking a value that is already structured (an ISO
  date, an email's shape, a reference's format), not for finding data in a page's text.

The design that implements this rule (the workflow tools, the protocol in the run loop, `must_say`, how to add a
machine) is `docs/json-tools.md`: read it before changing a skill, a code machine or the run loop.

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
the model's words to decide the next step marks a step that belongs in code (or in a JSON choice). The final
natural-language answer is itself reviewed once by the model, as the user would read it (`Runtime::review_answer`).

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
