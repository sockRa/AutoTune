# Issue tracker: GitHub

Use GitHub Issues in `sockRa/AutoTune` through the `gh` CLI.

ROADMAP.md remains the canonical product status, priority and handoff
overview. Link tickets to its E0–E8 story IDs and relevant detailed
contracts. Ticket closure requires the roadmap's evidence criteria.

Before creating issues, verify the target:
`gh repo view --json nameWithOwner`.

- Read: `gh issue view <number> --json number,title,body,labels,comments`
- List: `gh issue list --state open --json number,title,body,labels`
- Create: `gh issue create --repo sockRa/AutoTune --title "..." --body-file <file>`
- Label: `gh issue edit <number> --add-label "..." --remove-label "..."`
- Comment: `gh issue comment <number> --body-file <file>`
- Close: post the explanation, then `gh issue close <number>`.

Use temporary UTF-8 files for multiline bodies and comments.
Use native sub-issues and issue dependencies when supported; otherwise
record `Part of #<parent>` and `Blocked by: #<number>` in issue bodies.
A ticket is unblocked when every blocker is closed.

When a skill says "publish to the issue tracker", create a GitHub issue.
When it says "fetch the relevant ticket", read the GitHub issue.
Follow the session's authorization requirements for external writes.

## Pull requests as a triage surface

PRs as a request surface: no.
