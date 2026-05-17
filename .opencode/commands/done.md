---
description: Commit, push, and close the current issue after implementation is complete
---
Finish work on GitHub issue #$ARGUMENTS in the tjpeden/rgb-emulator repo.

Current git status:
!`git status`

Current branch:
!`git branch --show-current`

Follow these steps:
1. Run `cargo build --workspace` — fix any errors before proceeding
2. Run `cargo clippy --workspace` — fix any warnings before proceeding
3. Stage all changed files: `git add -A`
4. Commit with the message: `feat: <short description of what was implemented> (closes #$ARGUMENTS)`
   - Write the description in present tense, lowercase, no period
   - Keep it under 72 characters total
5. Push to origin: `git push origin main`
6. Close the issue: `gh issue close $ARGUMENTS --comment "Implemented in $(git rev-parse --short HEAD)"`

Report the commit hash and the GitHub issue URL when done.
