# AGENTS.md

Workflow rules for AI coding agents working in this repository.
These cover process and preferences that can't be inferred from the code.

## Branches

- Never work directly on `main`; always create a branch first.
- Naming: `<type>/<short-description>`, e.g. `feat/login-page`, `fix/null-user-crash`.
  - Types: `feat`, `fix`, `refactor`, `docs`, `chore`, `test`.
- One branch per task. Don't mix unrelated changes.
- Branch from an up-to-date `main`.

## Commits

- Use [Conventional Commits](https://www.conventionalcommits.org/): `<type>(<scope>): <summary>`
  - Scope is required: the area of the code affected (e.g. `auth`, `api`, `ui`).
  - e.g. `feat(auth): add password reset flow`
- Summary: imperative mood, lowercase, no period, max ~72 characters.
- Body (when needed): explain *why*, not *what*.
- Atomic commits: one logical change per commit (a feature step, a fix, or a refactor), never mixed.
  - Split by purpose, not by file. Unrelated fixes found along the way get their own commit.
  - Don't commit half-finished or knowingly broken code, but don't run checks per commit (see Checks).
  - Aim for under ~200 changed lines per commit so it can be reviewed in one sitting.
  - Keep formatting, renames, and generated files in separate commits from logic changes.
- Reference issues in the footer: `Closes #123`.
- Never commit secrets, `.env` files, or debug leftovers.
- Only commit when asked. Never amend or rewrite pushed commits.
- Don't reference AI agents or tools: no `Co-Authored-By` trailers for agents, no "Generated with ..." lines.

## Pull requests

- Title follows the same Conventional Commits format.
- Description must include:
  - **What**: what changed
  - **Why**: the problem or motivation
  - **How to test**: steps to verify
  - **Notes**: risks, follow-ups, screenshots for UI changes
- Keep PRs small; split if it's over ~400 lines of real changes.
- Open as **draft** until checks pass and it's ready for review.
- Link the related issue.
- Don't reference AI agents or tools in the title or description.
- Prefer squash merge. Never merge without approval.

## Checks (tests, lint, typecheck)

- Run checks once, at the end of a task, not after every small edit.
- While iterating, run only the tests related to the change, not the full suite.
- If I say I'm still working (or "wip"), don't run checks until I say I'm done.
- If a check is slow or long-running, say so before starting it.
- Never say a task is done without running checks; if you skipped them, say so.

## Before saying "done"

- Checks pass (see above).
- No leftover TODOs, console logs, or commented-out code.
- Docs/README updated if behavior or setup changed.

## Ask first before

- Pushing, opening a PR, or merging.
- Adding or upgrading dependencies.
- Changing database schema, public APIs, or CI config.
- Deleting files or running destructive commands (`reset --hard`, `push --force`, `rm -rf`).

## Never

- Force-push to shared branches.
- Skip hooks (`--no-verify`) or bypass checks.
- Commit directly to `main`.

## Communication

- If requirements are unclear, ask instead of guessing.
- When finishing, summarize what changed and anything not verified.
