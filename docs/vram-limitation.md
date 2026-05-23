# vRAM limitations — why "Shared GPU Memory" can't save us

## The question

On a GTX 1050 3 GB (Windows), the High preset triggers an OOM and Phase 5 disables the fine tier. The same machine reports ~10 GB of "Shared GPU Memory" available (system RAM the GPU driver can reach). Why can't we use it?

## Short answer

The "Shared GPU Memory" Windows shows is **not an allocation target for applications**. It's an overflow region the driver uses to evict idle resources from VRAM under pressure. Your code can't request "please put this texture there primarily" — at least not through wgpu, and not in any way that would keep frame rates above single digits.

## Why the OOM is unavoidable for that texture

When the fine tier loads at High (3.5 km radius @ 1 m/px), the GPU allocations are roughly:

| Resource | Format | Size |
|---|---|---|
| `hm1m_tex` | R32Float, 7000×7000 | 196 MB |
| `hm1m_normal_tex` | Rg16Snorm, 7000×7000 | 196 MB |
| `hm1m_shadow_buf` | f32 storage, 49 M elements | 196 MB |
| **Fine tier total** | | **~600 MB** |

wgpu asks D3D12 (or Vulkan) for a `DEVICE_LOCAL` heap allocation big enough for each texture. If the GTX 1050's 3 GB of dedicated VRAM doesn't have a contiguous-enough free region for the 196 MB texture at that moment, the allocation fails immediately. The driver **cannot** transparently split a single texture across "dedicated + shared" heaps. wgpu reports OOM; Phase 5 catches it and degrades to base + close only.

## What "Shared GPU Memory" actually is on Windows

The Windows WDDM driver model has two memory pools per GPU:

- **Dedicated Video Memory** — the GDDR soldered to the card (3 GB on GTX 1050).
- **Shared System Memory** — a portion of system RAM the GPU driver can reach across PCIe.

The shared pool is used by:

1. **The driver, transparently**, to evict whole resources from VRAM when newer resources need the space. This is automatic and the application doesn't control it. It only kicks in for resources that are already allocated and currently idle — it doesn't help a brand-new allocation that doesn't fit.

2. **CPU-visible heaps explicitly requested by the application** (`D3D12_HEAP_TYPE_UPLOAD` / `READBACK` in DX12; `HOST_VISIBLE | HOST_COHERENT` in Vulkan). These are designed for staging buffers — small allocations the CPU writes and the GPU reads once.

Neither of these helps a `DEVICE_LOCAL` texture that GPU shaders need to sample at full rate.

## What if we did use a CPU-visible heap for the fine tier?

Technically possible (write a custom DX12 backend, allocate the texture from `D3D12_HEAP_TYPE_UPLOAD`, use BAR memory or write-combined mapping). Empirically not worth it:

- **PCIe 3.0 ×16 bandwidth: ~16 GB/s.** GDDR5 on a GTX 1050: ~112 GB/s. 7× slower.
- The raymarcher samples the fine tier dozens of times per pixel for the binary-search refinement pass. Per-frame fine-tier reads at 1920×1080 are ~3 GB if every pixel touches it (overestimate, but order of magnitude).
- Net effect: frame time on the fine tier alone goes from ~5 ms to ~50–100 ms. The viewer is unusable.
- Plus: NVIDIA's WDDM runtime will probably page the texture back to dedicated VRAM as soon as it sees it being sampled heavily, defeating the entire point and re-introducing the OOM.

## What might genuinely help (and the tradeoffs)

In rough order of effort, none are zero-cost:

### 1. Switch the GTX 1050 to the Low preset

The auto-detector tags 3 GB-class cards (GTX 1050 / 1650 / 1660) as `Mid` because Mid fits inside 3 GB with margin under normal conditions. The OOM here means the user either picked `High` manually (90/20/3.5 km radii — way too big for 3 GB) or the driver / OS is holding more than usual at the moment of the reload. Switching the launcher dropdown to `Low` loads:

- Fine: 1 km × 1 m → 2000×2000 → ~48 MB total fine.
- Close: 8 km × 5 m → 3200×3200 → ~130 MB total close.

Comfortably under 0.7 GB total tracked; ~0.5 GB if the runtime OOM handler also dropped the fine tier on the first launch attempt. The launcher persists the choice, so this only needs to happen once.

### 2. R32Float → R16Float for the fine tier (Low preset only)

Halves the heightmap texture from 196 MB to 98 MB at the High-preset fine size. The cost: f16 has ~2 m quantization at 4096 m elevation, ~4 m at 8192 m (see `docs/sessions/base-tier-r16float-stairs.md`). At 1 m source spacing the stairs are far more visible than at the 30 m base tier where R16Float is currently used. Probably acceptable for Low-VRAM users who already accept reduced quality, but the artefact is real.

Implementation: a flag in `upload_hm1m` plus a `hm_to_f16_bytes` call before the `write_texture`.

### 3. Upgrade wgpu to 0.30+ and pass `MemoryHints::MemoryUsage`

The 0.30 release added `wgpu::MemoryHints` to `DeviceDescriptor`:

```rust
wgpu::DeviceDescriptor {
    required_features: ...,
    required_limits: ...,
    memory_hints: wgpu::MemoryHints::MemoryUsage,  // ← new in 0.30
    ..Default::default()
}
```

This hints to the internal `gpu-allocator` (Vulkan) or D3D12 heap policy to prefer smaller block sizes and tighter packing. It doesn't move textures to system RAM, but it can reduce fragmentation inside the allocator's pools — sometimes enough to make a borderline allocation succeed where it would otherwise fail.

Cost: 0.29 → 0.30 has breaking changes (`Maintain` → `PollType` renames, Surface lifetime model, `Required` features). It's a real piece of work, on the order of half a day of careful upgrades.

### 4. Per-view custom radii (the future plan)

The right fine-tier radius for a 3 GB card might be 1.5 km, not 1 km (today's Low default) or 3.5 km (High). The custom-view constructor lets the user dial it in until it fits. This is the production answer: the user knows their hardware better than any heuristic.

### 5. Force the DX12 backend on Windows + NVIDIA

`wgpu::Backends::DX12` instead of the current `Backends::all()`. DX12's residency manager (`MakeResident` / `Evict` calls) does somewhat better than Vulkan's under pressure on NVIDIA Windows drivers. Empirical effect on a 3 GB card is uncertain — could be a few hundred MB of headroom, could be nothing.

### 6. Split the fine texture into tiles (major rework)

Manually chunk the fine tier into many small textures (e.g. 1024×1024) and sample the right chunk per pixel in the shader. The sum of chunks doesn't change — so the same OOM risk — but allocation patterns become smaller, which reduces fragmentation. Not worth the shader / bind group complexity for that.

## What you can't easily do

- **No "spill this texture into shared memory" API in wgpu 0.29.**
- **No transparent tiling of a single wgpu texture across heaps.**
- **No way to ask the driver to pre-evict idle textures from other applications.** That's an OS-level call and any well-behaved app respects it transparently anyway.
- **No way to use OpenCL / CUDA / DirectStorage-style direct VRAM access.** Out of scope of a wgpu compute renderer.

## Bottom line

Phase 5 doing exactly what the user observed — disabling the fine tier and continuing — is the right product answer for this hardware. The GTX 1050 3 GB just isn't a fine-tier-at-High card; that's a 6 GB+ class workload. The "Shared GPU Memory" Windows reports is a driver-internal eviction pool, not an application-visible allocation target, and forcing wgpu to use it would tank performance to the point of unplayable.

The honest fix is **per-view radii** in the future custom-view plan: a user with a 3 GB card builds a custom view with `fine_radius_m = 1500.0` and gets sharp detail under the camera without OOM. Today's Low preset is a coarse approximation of that.
