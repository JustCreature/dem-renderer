# Phase 2 — Eager deallocation on tier reload

**Issue:** [#33 — vRAM OOM Error, no shared RAM utilisation](https://github.com/JustCreature/dem_renderer/issues/33)
**Full plan:** `~/.claude/plans/curious-booping-rabbit.md`
**Prerequisite:** Phase 1 instrumentation (counts what we allocate).

## The bug we're killing

The old code (and the old `set_hm5m_inactive` / `set_hm1m_inactive`) had a "grow-only" tier policy that looked cheap on paper but was lying about what wgpu actually does. The pattern was:

```rust
if size_changed {
    let new_tex = device.create_texture(&desc);   // (1) new alloc lands on GPU
    self._hm5m_texture = new_tex;                  // (2) Rust drops old handle
}
// ...
self.rebuild_bind_group();                        // (3) bind group releases its Arc
```

A reader sees "we overwrite the field, so the old is gone" — but the BindGroup at step (1) is still bound and still holds an internal `Arc<TextureInner>` on the old texture. wgpu won't actually free it until **two things both happen**:

1. The BindGroup releases its Arc (step 3, via `rebuild_bind_group`).
2. The next `queue.submit(...)` retires *and* `device.poll(...)` advances the destroy-after-submission queue.

So peak GPU memory across the reload is `sizeof(old) + sizeof(new)`. On a 4 GB GPU with the Tirol demo (base 1.3 GB + close 800 MB + fine 600 MB ≈ 2.7 GB steady state), a single tier reload doubles its contribution and the OOM lands on whichever allocation crosses the budget.

`set_hm5m_inactive` was even worse: it set `extent_x = 0.0` so the shader skipped the tier, but the texture / normal / shadow buffer for it stayed resident *forever*. After the camera blasted through a region at boost speed, ~800 MB of close-tier memory just sat on the GPU.

## The fix shape

A drop-first cycle, applied symmetrically in three call sites:

1. **Replace the field with a 1×1 placeholder** so the BindGroup's next rebuild can drop the Arc on the real resource.
2. **`rebuild_bind_group`** — releases the BindGroup's reference; wgpu schedules destroy after the current submission.
3. **`device.poll(wgpu::PollType::Wait { ... })`** — blocks until the prior submission retires and pumps the destroy queue. The old resource is now actually freed.
4. **Allocate the real new resource.** With the old gone, peak ≈ `sizeof(new)`.
5. **`rebuild_bind_group`** again — the second rebuild plugs the real new resource into the shader's binding.

Two BindGroup rebuilds per reload is cheap (microseconds; bind groups are descriptor tables). The `device.poll(Wait)` is the load-bearing line. Without it, wgpu defers cleanup to "sometime after the next vsync" and we're back to peak doubling.

## Why `poll(Wait)` is safe here

`device.poll(Wait)` blocks the calling thread until the next pending submission retires. Risks:

- **Steady-state frame loop:** would stall vsync hard (≥ 16 ms hitches). We never call it from there.
- **Reload paths:** already produce a perceptual hitch (CPU-side window extract is 200–500 ms, normals 50 ms, shadow 50 ms). Adding ~1 ms of `poll(Wait)` is invisible in that envelope.

Empirically on the Iris Plus the `write_texture` timings around a base reload are 150 ms (hm), 47 ms (mips), 261 ms (normals), 75 ms (ao) — the poll is rounding error.

## Files & changes

### `crates/render_gpu/src/scene/mod.rs`

- Added `make_tier_size_placeholders(device, queue, label) -> (Texture, View, Texture, View, Buffer)` — builds the three size-tied placeholder resources for close/fine tiers (1×1 R16Float, 1×1 Rgba8Snorm, 1-element f32 storage). Samplers are left untouched (they live as long as the `GpuScene` and have no per-allocation cost worth churning).
- `update_heightmap` now runs an inline drop-first cycle for all four base-tier resources: hm texture (R16Float with 8 mips), AO texture (R8Unorm), normals_packed (u32 storage buffer), shadow (f32 storage buffer). Placeholders are 1×1 with 1 mip (the bind group layout doesn't pin a mip count) and 4-byte buffers (storage layouts allow any size).

### `crates/render_gpu/src/scene/tiers.rs`

- `upload_hm5m` / `upload_hm1m`: collapsed the two independent `if size_changed` / `if buf_too_small` branches into one combined branch that runs the drop-first cycle. The `buf_too_small` flag is now subsumed (when size changes, the buffer is always recreated to match). This drops the per-call grow-only buffer optimization but it never mattered in practice — `buf_too_small` is only true when `size_changed` is also true (they're both derived from `cols * rows`).
- `set_hm5m_inactive` / `set_hm1m_inactive`: no longer cosmetic. Now `track_drop` the three resources, swap to placeholders, `rebuild_bind_group`, `poll(Wait)`. Early-return when the tier is already at placeholder state (avoids redundant polls on repeat calls — the base-reload flow calls these every time, and a wasted `poll(Wait)` after a clean state would block on whatever's in the queue for no benefit).
- Shader safety: `self.hm5m_extent_x = 0.0` is set *before* the first `rebuild_bind_group` so any compute dispatch that lands during the swap window skips the tier entirely. The shader checks `extent_x > 0.0` before sampling close / fine tier bindings. Restored to the real value at the end of `upload_hm*m`.

## Observed effect (Intel Iris Plus, Tirol demo)

Steady state, all three tiers loaded:

| Counter | Before base reload | After base reload (Phase 2) | After base reload + set_inactive |
|---|---|---|---|
| Texture | 1269.7 MB | 1028.6 MB | 166.7 MB |
| Buffer | 1346.9 MB | 820.8 MB | 389.8 MB |

The base reload itself (10800×10800 → 8192×5821 window) used to allocate 121 + 45 + 181 + 181 = ~530 MB *on top of* the 297 + 111 + 445 + 445 = ~1300 MB old base, then eventually drop the 1300 MB once the bind group rebuild flushed. Phase 2 inverts the order — the 1300 MB drops first, then 530 MB allocs into an empty heap.

But the bigger win is the `set_hm5m_inactive` + `set_hm1m_inactive` calls fired from the base-reload completion path in `viewer/mod.rs:426-427`. Those used to be no-ops; now they reclaim:

```
[vram]  drop tex hm5m_tex        - 244.02 MB  (tex 784 → 540)
[vram]  drop tex hm5m_normal_tex - 244.02 MB  (tex 540 → 540)
[vram]  drop buf hm5m_shadow     - 244.02 MB  (buf 820 → 576)
[vram]  drop tex hm1m_tex        - 186.92 MB  (tex 353 → 166)
[vram]  drop tex hm1m_normal_tex - 186.92 MB
[vram]  drop buf hm1m_shadow     - 186.92 MB  (buf 576 → 389)
```

~860 MB of texture + ~430 MB of buffer reclaimed every time the base shifts. On a 4 GB card that's the difference between "headroom" and "the next allocation OOMs".

## Subtleties worth knowing

- **The first `upload_hm5m` after startup also runs the drop-first cycle.** At that point the fields are already 1×1 placeholders from `GpuScene::new`, so the drops are 0 MB and the `poll(Wait)` is near-instant (nothing in the destroy queue). Cost: one extra `rebuild_bind_group` and one `poll(Poll)`-equivalent (the wait completes immediately). Not worth the special case.
- **Two log lines per resource per reload now.** The first `drop` is for the active resource being swapped to a placeholder. The second `drop` is for the placeholder being swapped to the real new resource. The pattern is `drop active → alloc placeholder (0 MB) → drop placeholder (0 MB) → alloc real`. Visually noisy but the counter behaviour is correct.
- **Why I didn't use the existing `create_tier_placeholder`.** It also creates samplers. We only want to churn the size-tied resources. A new `make_tier_size_placeholders` keeps the swap focused; the samplers stay alive as long as the GpuScene does.
- **Why the base-tier placeholder is `R16Float` with `mip_level_count: 1` instead of 8.** The bind group layout (`scene/mod.rs:385-550`) doesn't constrain mip count — it only requires `sample_type: Float { filterable: true }`. The placeholder isn't sampled by any shader dispatch in the swap window anyway (`update_heightmap` runs between frames, no compute pass is concurrent).

## Verification on the M4 Max

Pressing the debug `R` hotkey now shows two clear "dip then climb" patterns in the log (one for hm5m, one for hm1m), instead of the old "climb then steady" pattern that hid the peak. Real GPU residency (Activity Monitor → GPU → "Memory Used") follows the counter much more closely.

The peak no longer doubles on reload — confirmed both for the close/fine tiers and the base tier. The fix is also visible *without* a reload: when the camera moves fast enough to hit speed-gate suppression and `set_hm*_inactive` fires, GPU memory drops by ~800 MB within a few frames (used to drop only on process exit).

## Not done in this phase

- The placeholder swap creates a tiny placeholder (~2-4 bytes per resource) that is then immediately dropped. Net waste: one alloc + one immediate drop per tier resource per reload. Could be eliminated by destructuring the placeholder right at the swap (re-using the same `Texture` handle after `rebuild_bind_group`), but it isn't worth the extra plumbing for ~10 bytes.
- `set_hm5m_inactive` and `set_hm1m_inactive` both run `poll(Wait)`. The base-reload path calls both back-to-back, so that's two polls. They're both near-instant since the queue is empty after the first one, but it could be batched into a single poll. Not worth a separate code path for ~1 ms saved.
- The drop-first cycle is correct for memory safety but the counter ordering in the log is "drop active → alloc placeholder (0 MB) → drop placeholder (0 MB) → alloc real" — verbose. A cleaner log would silently merge the placeholder steps. Left as-is because the verbosity is useful while validating the fix on every machine class.
