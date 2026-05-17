---
description: Start work on a GitHub issue — creates branch, assigns issue, and begins implementation
---
Start work on GitHub issue #$ARGUMENTS in the tjpeden/rgb-emulator repo.

Here are the issue details:
!`gh issue view $ARGUMENTS --json number,title,body,labels,milestone`

Follow these steps:
1. Display a brief summary of what needs to be done
2. Create and switch to a branch named `issue-$ARGUMENTS`
3. Assign the issue to @me with: `gh issue edit $ARGUMENTS --add-assignee "@me"`
4. Implement the issue following the architecture described in AGENTS.md and docs/PRD.md
5. When implementation is complete, run `cargo build --workspace` to verify everything compiles
6. Fix any compiler errors before considering the work done

Do not commit — leave that for the /done command.
