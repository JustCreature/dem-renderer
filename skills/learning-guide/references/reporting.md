# Report Generation Requirements

Reports are optional but strongly recommended. They are the long-term learning artefact —
the session log is a diary, the reports are the textbook and reference card.

---

## `--R` Behaviour

1. Read `docs/sessions/phase-N/main-session.md` (full session history for this phase).
2. Read existing `long-report.md` and `short-report.md` if present (update rather than replace).
3. Read CLAUDE.md for the phase's key numbers and lessons.
4. Write both files. Do not ask for confirmation — generate immediately.

---

## long-report.md — Comprehensive Student Textbook

**Audience**: A student who missed every session and needs to learn everything from this document alone.

**Length**: As long as needed. No artificial limit.

### Required structure

```
# Phase N — [Topic]: Comprehensive Student Textbook

## Part 1: [Concept Name]
### 1.1 The Motivation
### 1.2 [Subtopic]
...

## Part 2: [Next Concept]
...

## Part N: Benchmark Results

[All measured numbers, conditions, hardware, full tables]

## Part N+1: Common Errors Encountered

[Every bug, wrong hypothesis, and gotcha, with cause and fix]

## Summary

[3–6 bullet points: the key lessons proved by measurement]
```

### Content requirements

- Every term defined on first use
- Every hardware concept explained from first principles (cache line size, ROB depth, etc.)
- All code patterns shown with inline explanation (not just "here is the code")
- All benchmark results with full context: hardware, buffer sizes, cold/warm cache, resolution
- Every wrong hypothesis documented: "We expected X, measured Y, because Z"
- Every error encountered: cause and fix
- Hardware reasoning at full depth — do not say "use SIMD"; explain port pressure, lane width, gather overhead

---

## short-report.md — Comprehensive Reference

**Audience**: A student who did every session and wants to refresh their full mental model in
10–15 minutes.

**Length**: Thorough but skimmable. Each section 2–6 sentences plus code/table.

### Required structure

```
# Phase N — [Topic]: Reference Document

## 1. [Concept]

[2–4 sentences. Self-contained. Everything needed to reconstruct the mental model.]

---

## 2. [Next Concept]

...

## N. Full Benchmark Table

[All measured numbers in one place. Hardware, date, conditions noted at top.]

## N+1. Common Errors

| Error | Cause | Fix |
```

### Content requirements

- Every concept covered — nothing omitted because "they already know it"
- All code patterns included (concise but complete)
- All benchmark tables
- All errors in error table format
- Numbers linked to hardware reasoning (why did we get 55 GB/s and not 400 GB/s?)

---

## What Makes a Good Report vs a Bad One

| Good | Bad |
|---|---|
| "NEON single-thread = scalar because the serial running-max dependency chain prevents ILP; NEON vectorises across 4 *rows*, not within a row" | "NEON was the same speed as scalar" |
| Includes the wrong hypotheses and why they were wrong | Only documents what worked |
| Every number has its hardware context (M4 Max, cold cache, 3601×3601) | Bare numbers with no context |
| Binary search fix explained with the arc artifact it solved | "Added binary search refinement" |
| Full error table including the banded-stripes RgbImage vs RgbaImage mistake | Only the important bugs |
