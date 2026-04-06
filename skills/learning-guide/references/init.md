# Init Flow — New Project Setup

Run this when `docs/planning/global-plan.md` is absent (fresh directory).

---

## Step 1: Create Folder Structure

```
<project-root>/
├── CLAUDE.md               ← create from template below
├── docs/
│   ├── planning/
│   │   └── global-plan.md  ← create empty, user will fill in
│   ├── sessions/           ← session logs written here per phase
│   └── lessons/            ← learning reports written here per phase
└── src/                    ← (or wherever code lives; don't create unless needed)
```

Create all directories. Create the two files listed. Do not create anything else.

---

## Step 2: Write CLAUDE.md from Template

```markdown
# CLAUDE.md

## Learning Guide

This project uses the `learning-guide` skill. It is **always active**.

### Commands

| Command | What it does |
|---|---|
| `--R` | Generate / update docs/lessons/phase-N reports |
| `--\|` | Save session to docs/sessions/phase-N/ and update CLAUDE.md |
| `--\|--` | Restore from most recent session in docs/sessions/ |
| `--\|--path` | Restore from a specific session file |
| `--s` | Show current phase, status, open items, last session |
| `--v` | Finalise current phase (if all items complete) |
| `--v--FORCE` | Finalise unconditionally; carry open items forward |

---

## Interaction Mode

- **Guide, don't implement.** Explain *why* at the hardware level. Do not write code unless
  explicitly asked (code-exception mode).
- **Assume strong technical curiosity.** Full-depth: cache-line math, TLB reach, ROB/store-buffer
  reasoning, TAGE branch predictor, port pressure.
- **Measurement over intuition.** Every optimisation claim requires measured numbers.
- **Layered mental models.** Hardware constraint → software implication → experiment to validate.

---

## Project Purpose

[USER FILLS IN: what is being built and what hardware concepts will be explored]

---

## Status

**Current phase: Phase 1** (Phase 0 complete)

[Phase 0 artifacts will be added here by `--|` after first session]

---

## Implementation Phases

See `docs/planning/global-plan.md` for full details.

[User or guide fills in phase list here]
```

---

## Step 3: Write Empty global-plan.md

```markdown
# Project Plan

## Goal

[What is being built and why — user fills in]

## Phase 0 — Foundations & Tooling

[What tooling, profiling harness, baseline numbers are needed]

## Phase 1 — [First real topic]

[...]
```

---

## Step 4: Orient the User

Say:

> "Workspace initialised. I've created the folder structure, `CLAUDE.md`, and an empty
> `docs/planning/global-plan.md`.
>
> Before we start: what are you building, and what hardware or systems concepts do you want to
> understand deeply by the end? I'll help you design the phase plan around your learning goals."

Then help the user fill in `global-plan.md` — ask about:
1. What they are building (language, domain, target data size)
2. What hardware concepts they care about (cache, SIMD, GPU, network, disk, etc.)
3. How many phases feel right (typically 5–8 for a thorough project)
4. Whether they have target hardware to profile on

Do not proceed to Phase 0 until `global-plan.md` has at least a Goal and Phase 0–2 sketched out.
