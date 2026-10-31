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

## Skills: code decides, the model chooses from JSON, prose only for the user

In each skill, use code tools as much as possible — a web form is filled by a form function per URL, a mail search is
a fixed query, a default, a deadline, a window, a file name, a schedule and a delivery are computed. A skill should
need **no model call at all** to decide what happens next (see `agent/driver.rs` for the arrival cards: the model only
understands the request and loads the skill; code runs every step).

When a decision genuinely needs the model, do not ask it in prose and read its prose back. Send it **JSON with the
possible choices** and have it return one of them (a decision API: `{"choice": "A"}` out of the listed options). This
holds even when the input is a picture or a page: the possible decisions are listed in JSON for the model to choose
from (what field, which option, which button — never "what should I do next?").

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
