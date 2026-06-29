# quarto-yaml

## **WORK TRACKING**

We use **braid** for issue tracking instead of Markdown TODOs or external tools.
braid stores all issues for the project in a **skein** (a single
[automerge](https://automerge.org) CRDT document); a single issue is a
**strand**. The skein — synced through a sync server — is the **source of
truth**. There is no git involvement and no JSONL to commit: edits converge
through the CRDT, not through merge tooling.

The skein is configured in `.braid.toml` (gitignored — the `doc_id` is a
bearer token, do not commit it).

**`braid` is non-invasive and never executes git commands.** There is
**nothing to commit** after issue work — the skein syncs itself. (A
`.braid/snapshot.jsonl` backup *may* be committed periodically, but it is
**backup-only and one-directional** — see the snapshot policy below. Never
`braid import` it back.)

For the authoritative, version-matched command guide, run `braid agents-info`
(or invoke the `/braid` skill). The quick reference below is a convenience
summary, not the contract.

We use plans for additional context and bookkeeping. Write plans to
`claude-notes/plans/YYYY-MM-DD-<description>.md`, and reference the plan file
in the strands.

### Plan Files

Plan files should include:

1. **Overview**: Brief description of the plan's goals and context
2. **Checklist**: A markdown checklist of all work items using `- [ ]` syntax
3. **Details**: Additional context, design decisions, or implementation notes as needed

As you work through a plan:

1. **Update the plan file** after completing each work item
2. **Check off items** by changing `- [ ]` to `- [x]`
3. **Keep the plan file current** - it serves as both a roadmap and progress tracker
4. **Add new items** if you discover additional work during implementation

Create plan files for:

- Multi-step features spanning multiple crates
- Complex refactoring that requires coordination
- Tasks where tracking progress helps ensure nothing is missed

Complex plans can have phases, and work items are then split into multiple
lists, one for each phase. For simple tasks (single file changes, bug fixes),
the TodoWrite tool is sufficient.

### Braid Quick Reference

```bash
# Find ready work (active + unblocked)
braid ready --json

# Create new strand (prints its id; braid assigns a bd-<random> id)
braid create "Strand title" -t bug|feature|task -p 0-4 -d "Description" --json

# Create with labels
braid create "Strand title" -t bug -p 1 -l bug -l critical --json

# Create and link discovered work in one shot
braid create "Found bug in auth" -t bug -p 1 --deps discovered-from:<current-id> --json

# Update status
braid update <id> --status in_progress --json

# Link existing strands (id depends on target)
braid dep add <discovered-id> <parent-id> --type discovered-from

# Complete work
braid close <id> --reason "Done"

# Show an epic's descendant tree / one strand's details
braid dep tree <id>
braid show <id> --json

# Backup snapshot (one-directional — see snapshot policy; NEVER import it back)
braid export > .braid/snapshot.jsonl
```

Notes:
- **No explicit `--id`.** braid assigns collision-free ids; with a CRDT,
  parallel workers never need to pre-agree on ids.
- **No bulk create from a file flag.** Use `braid import <jsonl>` for bulk.
- **No sync-and-commit step.** The skein is the source of truth; there is
  nothing to commit after issue work.

### Workflow

1. **Check for ready work**: `braid ready` to see what's unblocked
2. **Claim your task**: `braid update <id> --status in_progress`
3. **Work on it**: implement, test, document; leave a trail with
   `braid comment <id> "..."`
4. **Discover new work**: file it and link it in one shot:
   `braid create "Found bug in auth" -t bug -p 1 --deps discovered-from:<current-id> --json`
5. **Complete**: `braid close <id> --reason "Implemented"`

That's the whole loop — **no sync-and-commit step.** braid syncs the skein to
the server on every command. (`braid sync` forces a round trip if you want to
confirm convergence.)

### Issue Types

- `bug` - Something broken that needs fixing
- `feature` - New functionality
- `task` - Work item (tests, docs, refactoring)
- `epic` - Large feature composed of multiple strands
- `chore` - Maintenance work (dependencies, tooling)
- `docs` - Documentation work
- `question` - Open question to resolve

### Priorities

- `0` - Critical (security, data loss, broken builds)
- `1` - High (major features, important bugs)
- `2` - Medium (nice-to-have features, minor bugs)
- `3` - Low (polish, optimization)
- `4` - Backlog (future ideas)

### Dependency Types

- `blocks` - Hard dependency (X depends on / is blocked by Y)
- `parent-child` - Epic/subtask relationship
- `related` - Soft relationship (strands are connected)
- `discovered-from` - Track strands discovered during work
- braid also accepts `conditional-blocks`, `waits-for`, `replies-to`,
  `duplicates`, `supersedes`, `caused-by`.

**What gates `ready`:** `blocks`, `conditional-blocks`, and `waits-for` make a
strand unready while their target is active. `parent-child` does **not** block
the child (children stay workable); instead an open child blocks the *parent's*
close. `related`/`discovered-from` and the rest are informational.

### Snapshot backup policy (READ THIS)

The skein (automerge CRDT) is the **single source of truth**. You may
additionally commit a `.braid/snapshot.jsonl` (`braid export`) to the repo so
issues stay greppable in PRs, diffable in git history, and recoverable. This
snapshot is **backup-only and strictly one-directional**:

- It flows **automerge → file only** (`braid export > .braid/snapshot.jsonl`).
  It is **never** an import or sync source back into the skein. **Never run
  `braid import .braid/snapshot.jsonl`.**
- On any git **conflict** in `.braid/snapshot.jsonl`, do **not** hand-merge:
  resolve by regenerating from the live skein (`braid export`). The CRDT is
  authoritative; the file is a photograph. (Cross-branch contamination — the
  snapshot on one branch showing strand state created on another — is expected
  and fine, because the snapshot is not the truth.)
- The snapshot lives on whatever work branch you're on; it is not special.
