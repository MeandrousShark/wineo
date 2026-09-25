# CLAUDE.md

AGENTS.md is the primary instruction file for every agent. Put shared rules
there, and keep only Claude-specific instructions here.

@AGENTS.md

## Claude-specific

- The primary session writes documentation itself. Don't delegate docs to
  the `docs-writer` agent, since the primary already has the context.
