# Session Management and Phase Finalisation

---

## Session File Format

Path: `docs/sessions/phase-N/main-session.md`

If appending (session continues across conversations), add a `---` separator and a date heading
before the new content.

### Template

```markdown
# Phase N Session Log

## Overview

[1–3 sentences: what this phase covered and its learning objective]

---

## Session 1 — [Topic]

### What was built / explored

[Artifacts created, experiments run, key decisions made]

### Errors and fixes

[Any bugs, misunderstandings, wrong hypotheses — document these: they are the best learning]

### Key discussion points

[Hardware concepts explained, mental models built]

---

## Session 2 — [Topic]   ← appended by --|

[...]

---

## Final Numbers

[Table of all measured results for this phase]
```

---

## `--|` Checklist (run every time)

1. Write or append session summary to `docs/sessions/phase-N/main-session.md`
2. Update CLAUDE.md:
   - `## Status` → current phase, mark any newly completed sub-items
   - Add any new artifacts (file paths + one-line descriptions) under the phase section
   - Add any new numbers to the phase's "key numbers" block
   - Add new "Known open items" if any were discovered
3. Confirm to user

---

## `--v` / `--v--FORCE` Finalisation Checklist

These must all be done before marking a phase complete:

- [ ] Run `--|` (save session + update CLAUDE.md)
- [ ] CLAUDE.md `## Status` bumped to next phase
- [ ] Completed phase section written in CLAUDE.md:
  - All artifact file paths listed with one-line descriptions
  - Key numbers table (benchmark results that prove the learning objective)
  - Lessons (2–4 sentences: what the numbers proved about hardware behaviour)
  - Known open items (anything intentionally skipped or deferred)
- [ ] User has been prompted to run `--R` for reports

### CLAUDE.md Phase Section Format

```markdown
Phase N artifacts:
- `path/to/file.rs` — brief description
- `path/to/other.rs` — brief description
- `docs/lessons/phase-N/long-report.md` — comprehensive textbook (if generated)
- `docs/lessons/phase-N/short-report.md` — reference doc (if generated)
- `docs/sessions/phase-N/main-session.md` — session log

Phase N key numbers (hardware, resolution, conditions):
- [metric]: [value] | [metric]: [value]
- Lesson: [what the numbers proved]

Known open items from Phase N:
- [item] — [brief context]
```

---

## Restore (`--|--`) Logic

1. If no path given: glob `docs/sessions/*/main-session.md`, sort by modification time, take most recent.
2. Read the file.
3. Extract: current phase, last session section heading, last "open items" or "next step" mention.
4. Summarise to user in 3–5 sentences. Ask if they want to continue from that point.
5. Do not modify any files during restore — read only.
