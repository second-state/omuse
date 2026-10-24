# OMuse: notes for Claude Code

Read `AGENTS.md` first: it has the release steps, the test commands and the repository rules.

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
