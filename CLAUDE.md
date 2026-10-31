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
