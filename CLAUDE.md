# CLAUDE.md

All project guidance lives in AGENTS.md so every agent follows the same rules.

@AGENTS.md

Coding subagents may stage and commit their own logical changes. The primary
session handles all release work, including pushes, merges, tags, releases,
publishing, and deployments. Do not delegate release work to the `release`
subagent or any other subagent.

Patch fixes and hotfixes need no plan, and the primary session may implement
them itself instead of delegating.
