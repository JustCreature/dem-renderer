# Phase 5 — OOM safety net + debug hotkey

**Issue:** [#33 — vRAM OOM Error, no shared RAM utilisation](https://github.com/JustCreature/dem_renderer/issues/33)
**Full plan:** `~/.claude/plans/curious-booping-rabbit.md`
**Prerequisites:** Phase 2 (eager dealloc), Phase 3 (`TierRadii` mutability), Phase 4 (override plumbing).

## Why a safety net even after Phases 1–4

Phases 2 and 3 cut steady-state memory and reload peak roughly in half on the Tirol demo. With Phase 4 the user can dial the budget even lower manually. But three failure modes survive:

1. **Mis-classified GPU.** Phase 3's substring table can't possibly stay current with every new SKU. A new "GTX 1700-something 4 GB" we haven't seen will default to `Mid` and still OOM at the demo's peak. The user shouldn't have to file a bug to discover this — the app should react.
2. **Custom DEM bigger than the demo.** A user loading a single 10 GB BigTIFF in single-file mode can drive the base tier alone past 1.5 GB even at the Low preset, depending on the source resolution and IFD overview availability. There's no static way to detect this without parsing the file's IFD tree, which `prepare_scene_with_ctx` does — but by then we've already committed to a tier preset.
3. **External pressure.** Another process opens a huge framebuffer mid-flight (browser tab with hardware-accelerated video, GPU-bound game). Suddenly there's 1 GB less budget than when we started. Phase 3's startup detection has no way to know.

Default wgpu behaviour on a failed allocation is `panic!` from an internal thread, killing the process. The user sees nothing useful — just the same crash they had before any of the phases shipped.

Phase 5 catches the OOM, logs it, drops a tier, and keeps running. Worst case the user sees a quality regression instead of a crash; best case they never notice because Phase 3 already prevented the OOM.

## How wgpu's error handler works

`device.on_uncaptured_error(Arc<dyn UncapturedErrorHandler>)` registers a callback that wgpu invokes when a non-scoped error happens. The handler trait is `Fn(wgpu::Error) + Send + Sync + 'static`. wgpu fires it on its internal worker thread (the same one that drives the destroy queue), so:

- **No blocking in the handler.** A `std::sync::Mutex::lock()` here could deadlock against the main thread.
- **No allocation in the handler.** It runs on the OOM path; allocating could re-fire.
- **No state mutation visible to the main thread except via atomics.** Anything else needs explicit synchronisation that doesn't allocate.

So the handler is the minimum possible: log the error, and if it's an `OutOfMemory` variant, set a `static AtomicBool` and bump a `static AtomicU32` count. The main thread reads the atomic at the top of each frame and degrades; that's where all the heavy work happens, on the main thread where mutation is free.

```rust
device.on_uncaptured_error(Arc::new(|err: wgpu::Error| {
    eprintln!("[GPU ERROR] {err:?}");
    if matches!(err, wgpu::Error::OutOfMemory { .. }) {
        OOM_OBSERVED.store(true, Ordering::SeqCst);
        OOM_COUNT.fetch_add(1, Ordering::SeqCst);
    }
}));
```

The handler signature changed between wgpu 0.20 and 0.29: the older code shape used `Box<dyn Fn(Error) + Send + Sync>`; 0.29's `on_uncaptured_error` takes `Arc<dyn UncapturedErrorHandler>` (where `UncapturedErrorHandler` is a marker trait blanket-impl'd on `Fn(Error) + Send + Sync + 'static`). Same ergonomics, different ownership wrapper.

## Degradation policy

Three-step ladder, applied in `Viewer::poll_and_handle_oom` at the top of every redraw:

1. **Disable the fine tier.** Most callers stream a 7000×7000 R32Float window for fine — ~600 MB of textures + buffers. Killing it gives the most VRAM back per step, and the user's view at altitude usually doesn't depend on it (the close tier covers 8–20 km from the camera).
   - `scene.set_hm1m_inactive()` — Phase 2's eager-dealloc path; swaps the three fine resources (hm, normal, shadow) to 1×1 placeholders, rebuilds the bind group, calls `device.poll(Wait)` to drain wgpu's destroy queue.
   - `bev_base.fine = None` — drops the `StreamingTier`, which drops the `mpsc::SyncSender` to the worker. The worker thread exits cleanly when it sees the channel closed (the next `recv()` returns `Err`).
   - `self.tier_radii.fine_radius_m = 0.0` — informational; the reload loop already checks `bev_base.fine.is_some()`.

2. **Disable the close tier.** ~800 MB at High, ~130 MB at Low. Triggered on the second OOM (or the first if a hand-edited config / future preset set fine_radius = 0 at startup, since the launcher presets all keep fine on).
   - `scene.set_hm5m_inactive()` — same Phase 2 path for the close tier.
   - `self.close_tier_disabled = true` — runtime kill switch. The close worker can't be cleanly dropped without restructuring `BevBaseState` (it's `StreamingTier`, not `Option<StreamingTier>`), so the worker keeps running but the main thread stops sending reload requests (`try_trigger` gated on `!self.close_tier_disabled`). The worker parks on `recv()` indefinitely, which is fine — it's a background thread, no CPU spent.
   - The close delivery `try_recv` is also gated: if the worker has a TierData in flight when close gets disabled, we drop it instead of uploading. Otherwise we'd re-allocate the texture we just freed.

3. **No-op.** Both detail tiers gone, base tier is the floor. If we still OOM, there's nothing safe to drop — the base hm/ao/normals/shadow are part of the bind group layout and can't be omitted without re-compiling the shader. Log it and let the user decide whether to relaunch with a smaller dataset.

Each step clears `OOM_OBSERVED` so the next event triggers the next step. The `OOM_COUNT` keeps incrementing — useful for debugging "I OOMed 7 times before the close tier finally died." The handler doesn't need to know what step we're on; the main thread does.

## Why not drop the close-tier worker outright?

`BevBaseState.close` is typed `StreamingTier`, not `Option<StreamingTier>`. Making it optional would require ~30 lines of `if let Some(...)` plumbing across `viewer/mod.rs` (close delivery, close trigger, close threshold update, debug hotkey). The runtime flag is two booleans of state and three call-site gates — strictly less code, same observable behaviour.

The worker thread doesn't cost anything when parked: `mpsc::Receiver::recv()` is a blocking syscall that the OS scheduler doesn't wake. Zero CPU, ~8 KB of stack. The only "leak" is the worker holding `Arc<TileIndex>` and `Arc<Heightmap>` references, which keep that data alive until the process exits — measured in dozens of MB, well worth the simplicity.

If the user manually re-enables the close tier (Phase 4 dropdown toggled mid-flight, future change), we'd need to rebuild the worker anyway because the captured `close_radius_m` was the old value. Same applies to any "auto-recovery after low pressure" path. Filed mentally; not in scope here.

## Why no `ReloadGate` after all

The original Phase 5 plan included a `ReloadGate { Idle, BaseInFlight, CloseInFlight, FineInFlight }` state machine to stagger reloads so close and fine couldn't fire near-simultaneously. After implementing Phase 2 (eager dealloc + `device.poll(Wait)` per upload), the stacking problem evaporated:

- The workers themselves run in parallel — but their `TierData` bundles land in `mpsc::Receiver`s that the main thread drains sequentially in a single frame.
- Each `upload_hm5m` / `upload_hm1m` call does drop-first internally: `set old → placeholder`, `rebuild_bind_group`, `poll(Wait)`, `alloc new`, `rebuild_bind_group`. The `poll(Wait)` is the critical line — by the time it returns, wgpu has actually freed the old resource on the GPU.
- So even if close and fine both deliver in the same frame, the upload sequence is: free close → alloc close → upload close data → free fine → alloc fine → upload fine data. Peak across the two uploads ≈ `new_close + new_fine` (steady state), not `(old_close + new_close) + (old_fine + new_fine)`.

Adding a state machine would have *delayed* one tier's reload by an extra frame for no memory benefit. Skipped.

## The debug hotkey

`O` in the viewer fires `render_gpu::signal_oom_for_testing()`, which is the same thing the real OOM handler does: sets `OOM_OBSERVED = true`, increments `OOM_COUNT`. On the next frame's `poll_and_handle_oom` call, the degradation runs.

Why not `Ctrl+Shift+O` like the plan suggested? Modifier tracking isn't already wired into the viewer's keyboard handler (the only modifier-sensitive bindings are `SuperLeft / AltLeft` for speed-boost, and they don't combine with other keys). Adding a modifier state machine just for one debug hotkey isn't worth it — `O` is the only binding on that key, so the chance of accidental presses is low.

The simulation is indistinguishable from a real OOM from the viewer's perspective: both go through the same atomic, both trigger the same degradation path. Testing on the M4 Max with `O` will exercise the code that a GTX 1650 user would exercise on a real OOM.

## Files & exact changes

### `crates/render_gpu/src/context.rs`
- New `pub static OOM_OBSERVED: AtomicBool` and `pub static OOM_COUNT: AtomicU32`.
- New `pub fn signal_oom_for_testing()` and `pub fn clear_oom_flag()`.
- `GpuContext::new()` calls `device.on_uncaptured_error(Arc::new(|err| ...))` immediately after device creation. The handler logs every error variant and sets the atomic on `Error::OutOfMemory`.

### `crates/render_gpu/src/lib.rs`
- Re-exported `OOM_OBSERVED`, `OOM_COUNT`, `clear_oom_flag`, `signal_oom_for_testing`.

### `src/viewer/mod.rs`
- `Viewer` gained `close_tier_disabled: bool` (default `false`).
- New `Viewer::poll_and_handle_oom` method runs at the top of `RedrawRequested`. Checks `OOM_OBSERVED`; if set, clears it and steps the ladder.
- The close-tier reload-trigger conditional now also checks `!self.close_tier_disabled`.
- The close-tier delivery branch now checks `self.close_tier_disabled` and discards in-flight bundles instead of uploading.
- New `O` key handler calls `render_gpu::signal_oom_for_testing()`.

## Verification on the M4 Max

1. Launch the demo. Confirm log shows `[GPU ERROR]` handler installed implicitly (no message until OOM).
2. Press `O`. Expected:
   ```
   [vram] debug: simulated OOM (O)
   [OOM #1] disabling fine tier — freeing ~hm1m_tex + normal + shadow memory
   [vram]  drop tex hm1m_tex               - 186.92 MB  (tex   ... MB, buf   ... MB)
   [vram]  drop tex hm1m_normal_tex        - 186.92 MB
   [vram]  drop buf hm1m_shadow            - 186.92 MB
   [vram] alloc tex hm1m_tex               +   0.00 MB
   [vram] alloc tex hm1m_normal_tex        +   0.00 MB
   [vram] alloc buf hm1m_shadow            +   0.00 MB  (tier placeholder swap)
   ```
   Visually: 1 m fine detail disappears from the area under the camera; only 5 m close + 30 m base remain.
3. Press `O` again. Expected:
   ```
   [OOM #2] disabling close tier — freeing ~hm5m_tex + normal + shadow memory
   [vram]  drop tex hm5m_tex               - 244.02 MB
   [vram]  drop tex hm5m_normal_tex        - 244.02 MB
   [vram]  drop buf hm5m_shadow            - 244.02 MB
   ```
   Visually: 5 m close detail disappears too; the whole view runs on the 30 m base tier (looks coarser, especially close to the camera).
4. Press `O` a third time. Expected:
   ```
   [OOM #3] all detail tiers already disabled — base tier is the floor
   ```
   No further memory change.
5. If a close-tier worker had a TierData in flight when step 3 fired:
   ```
   [OOM] discarding in-flight close reload (tier disabled)
   ```

## What this fixes for low-VRAM users

3 GB-class cards (GTX 1050 / 1650 / 1660 and friends) default to `Mid` after Phase 4 — the auto-detector no longer downgrades them, because Mid (~1.7 GB tracked + ~0.3 GB driver overhead = ~2 GB) fits inside the 3 GB budget with margin. The safety net here catches the edge cases:

- If a user accepts the `Mid` default on a 3 GB card and the driver / OS happens to be holding more than usual (compositor scrolling a 4 K monitor, background browser tab with hardware video decode), the reload spike can push over the budget. The OOM handler fires, step 1 drops the fine tier; the viewer keeps rendering. The user can switch the launcher dropdown to `Low` for the next launch.
- 2 GB-class cards (MX series, RX 550/560, Arc A310/A380) auto-detect as `Low` from the substring table — Low's ~0.7 GB total (or ~0.5 GB after the runtime fine-tier drop) fits comfortably. If even Low somehow OOMs, step 2 drops the close tier and the viewer renders on the 30 m base alone.
- 4 GB+ discrete and Apple Silicon never trip this path under normal operation; the safety net is dead code on that hardware.

## Not done in this phase

- **No auto-promotion.** Once a tier is killed for the session, it stays killed. A future change could probe the available memory after some idle time and re-enable the close tier if there's headroom. The launcher's `vram_budget` setting is the explicit way to recover today: quit, change the dropdown, relaunch.
- **No telemetry surface.** OOM events are stderr-only. A future quality-of-life pass could surface them in the HUD ("1 of 2 detail tiers disabled — OOM at HH:MM:SS") so users without a terminal know what happened.
- **No graceful base-tier fallback.** Step 3 of the ladder is a no-op because the base tier is structurally required by the shader's bind group layout (binding 1 is a non-optional Float texture). To drop the base too we'd need a different shader path, which is well out of scope for an OOM hotfix.
- **No re-enabling of the close worker after Phase 4 mid-flight changes.** The viewer reads `vram_budget` once at launch; a future hot-apply would need to restart the close worker because its captured `close_radius_m` would be stale.
