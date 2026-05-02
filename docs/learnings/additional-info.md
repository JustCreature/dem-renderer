lerp = linear interpolation. Given two values a and b and a weight t ∈ [0, 1]:                                                                                 
                                                                                                                                                               
  lerp(a, b, t) = a + (b - a) * t                                                                                                                                
               = a * (1 - t) + b * t                                                                                                                             
                                                                                                                                                                 
  At t=0 you get a, at t=1 you get b, at t=0.5 you get the midpoint.                                                                                             
                                                                                                                                                               
  mix in WGSL is exactly lerp. mix(a, b, t) works element-wise on vec3, so:                                                                                      
  mix(n00, n10, fx)                                                                                                                                            
  blends each of x, y, z components separately by the same weight fx.                                                                                            
                                                                                                                                                                 
  ---                                                                                                                                                            
  Why two mix calls?                                                                                                                                             
                                                                                                                                                                 
  The bilinear blend is lerp applied twice — once along each axis:                                                                                             
                                                                                                                                                                 
         fx=0.3
          │                                                                                                                                                      
  n00 ───●─── n10      ← mix(n00, n10, 0.3) = top edge blend                                                                                                   
          │                                                                                                                                                      
          │  fy=0.7                                                                                                                                            
          │                                                                                                                                                      
  n01 ───●─── n11      ← mix(n01, n11, 0.3) = bottom edge blend                                                                                                
                                                                                                                                                                 
  Then blend those two results vertically by fy:                                                                                                                 
                                                                                                                                                                 
  mix( top_blend, bottom_blend, fy )                                                                                                                             
                                                                                                                                                                 
  That gives the weighted average of all 4 corners. The corner weights multiply out to:                                                                          
  - n00: (1-fx)*(1-fy) = 0.7 × 0.3 = 0.21                                                                                                                        
  - n10: fx*(1-fy) = 0.3 × 0.3 = 0.09                                                                                                                            
  - n01: (1-fx)*fy = 0.7 × 0.7 = 0.49                                                                                                                          
  - n11: fx*fy = 0.3 × 0.7 = 0.21                                                                                                                                
                                                                                                                                                                 
  Sum = 1.0 — a proper weighted average.          


The GPU draws rectangles as two triangles. Each triangle needs 3 vertices (x, y). Here's the fps box as a diagram:

  (4,4) ─────────── (180,4)
    │  ╲  triangle 1  │
    │    ╲             │
    │      ╲           │
    │  tri2  ╲         │
    │          ╲       │
  (4,36) ──────── (180,36)

  Triangle 1 (top-right half): (4,4) → (180,4) → (180,36) — top-left, top-right, bottom-right
  Triangle 2 (bottom-left half): (4,4) → (180,36) → (4,36) — top-left, bottom-right, bottom-left

  Together they tile the rectangle without overlap or gap.

  ---
  Now the settings box sits at the top-right, so:
  - x0 = width - 296 (left edge, 296px from right)
  - x1 = width - 4   (right edge, 4px margin)
  - y0 = 4            (top, same as fps)
  - y1 = 36           (bottom, same as fps)

  (w-296, 4) ─────────── (w-4, 4)
       │  ╲  triangle 1    │
       │    ╲               │
       │  tri2 ╲            │
       │          ╲         │
  (w-296,36) ─────── (w-4, 36)

  Triangle 1: (w-296,4) → (w-4,4) → (w-4,36)
  Triangle 2: (w-296,4) → (w-4,36) → (w-296,36)

  The rule is always the same: top-left → top-right → bottom-right for T1, then top-left → bottom-right → bottom-left for T2. The diagonal goes from top-left to
  bottom-right.

The base heightmap is a large window extracted from the BEV GeoTIFF. Its top-left corner has an absolute EPSG:31287 coordinate — let's call it the base origin 
  (ox, oy).                                                                                                                                                      
                                                                                                                                                                 
  The fine tiers (5m and 1m) are smaller windows nested inside that base. The GPU shader doesn't know absolute CRS coordinates — it only knows tile-local pixel  
  offsets: how many metres from the base origin to the fine tier's top-left corner.                                                                              
                                                                                                                                                                 
  Base heightmap (90km × 90km)                                                                                                                                 
  ┌─────────────────────────────────────────┐                                                                                                                    
  │ origin = (ox=273000, oy=447000)         │
  │                                         │                                                                                                                    
  │         ┌───────────┐                   │                                                                                                                  
  │         │  5m tier  │ origin_x = 19000  │                                                                                                                    
  │         │           │ origin_y = 23000  │                                                                                                                    
  │         │   ┌─────┐ │                   │                                                                                                                    
  │         │   │ 1m  │ │                   │                                                                                                                    
  │         │   └─────┘ │                   │                                                                                                                  
  │         └───────────┘                   │                                                                                                                    
  └─────────────────────────────────────────┘                                                                                                                  
                                                                                                                                                                 
  Those offsets (19000, 23000) are computed as:                                                                                                                  
                                                                                                                                                               
  origin_x = fine_tile.crs_origin_x  -  base.crs_origin_x                                                                                                        
  origin_y = base.crs_origin_y       -  fine_tile.crs_origin_y                                                                                                   
                                                                                                                                                                 
  Now the camera drifts far enough that the base reloads — a new 90km window is extracted, shifted say 30km east. The base origin changes:                       
                                                                                                                                                                 
  Before: base origin = (273000, 447000)                                                                                                                         
  After:  base origin = (303000, 447000)   ← shifted 30km east                                                                                                   
                                                                                                                                                               
  The fine tiers still hold their old offset numbers (19000, 23000) in GPU memory. But those numbers were computed relative to the old origin. Relative to the   
  new origin they point somewhere completely wrong — potentially off the terrain entirely.                                                                     
                                                                                                                                                                 
  Old offsets pointed here (correct):                                                                                                                          
    absolute = (273000 + 19000) = 292000  ✓                                                                                                                      
                                                                                                                                                                 
  Same offsets after base shift (stale):                                                                                                                         
    absolute = (303000 + 19000) = 322000  ✗  (wrong location, 30km off)                                                                                          
                                                                                                                                                                 
  So set_hm5m_inactive() and set_hm1m_inactive() flip a flag in the GPU uniform that tells the shader "ignore these tiers entirely" — the shader falls back to   
  the base resolution only. The fine-tier workers then independently detect the drift (via invalidate() → needs_reload() → try_trigger()) and deliver new windows
   computed relative to the new base origin. When those arrive, the correct offsets are uploaded and the tiers become active again.                              
                                                                                                                                                               
  The alternative — not hiding them — would show fine-tier terrain detail floating 30km from where the camera actually is, which is visually broken.


  ---                                                                                                                                                            
  What this code does: bilinear sampling of the 1m tier                                                                                                          
                                                                                                                                                                 
  The ray has already hit the terrain at pos (a world-space XY position). The shader must decide: which normal and shadow value should this pixel use? There are 
  up to 3 data sources — base 30m, 5m close tier, 1m fine tier. The 1m block (L386–419) is the final override layer.                                             
                                                                                                                                                               
  ---                                                                                                                                                            
  Step 1 — Is the 1m tier even active? (L387)                                
                                                                                                                                                                 
  if cam.hm1m_extent_x > 0.0
                                                                                                                                                                 
  When set_hm1m_inactive() is called on the CPU side, it zeros out extent_x. This single float is the "is this tier live?" flag. If it's zero, the whole block is
   skipped with zero GPU cost.                                                                                                                                 
                                                                                                                                                                 
  ---                                                                        
  Step 2 — Convert world position to tile-local coordinates (L388–390)                                                                                         
                                                                                                                                                                 
  World space (metres, absolute)
  ────────────────────────────────────────────────────────────                                                                                                   
         cam.hm1m_origin_x                                                   
         │                                                                                                                                                       
         ▼                                                                   
         ┌───────────────────────────────┐                                                                                                                       
         │  1m heightmap buffer          │                                                                                                                       
         │                               │                                                                                                                     
         │  (0,0) ──────────────────►   │  extent_x                                                                                                              
         │    │                          │                                                                                                                       
         │    │                          │                                                                                                                       
         │    ▼  extent_y                │                                                                                                                       
         └───────────────────────────────┘                                                                                                                       
                                                                                                                                                                 
  lx1 = pos.x - cam.hm1m_origin_x   ← how far RIGHT of the tile's left edge                                                                                      
  ly1 = pos.y - cam.hm1m_origin_y   ← how far DOWN  of the tile's top edge                                                                                       
                                                                                                                                                                 
  pos is in world metres. The origin is the top-left corner of the 1m window in those same metres. Subtracting gives a tile-local offset (lx1, ly1) where (0,0) =
   top-left, (extent_x, extent_y) = bottom-right.                                                                                                                
                                                                                                                                                                 
  ---                                                                                                                                                          
  Step 3 — Compute blend weight t1 (L390–391)                                                                                                                  
                                                                                                                                                                 
  in_1m = lx1 and ly1 are both inside [0, extent]   → true/false
                                                                                                                                                                 
  fine_tier_edge_dist(lx1, ly1) = distance to the nearest edge                                                                                                   
                                   of the 1m tile rectangle                                                                                                      
                                                                                                                                                                 
  smoothstep(0.0, BLEND_MARGIN, that_distance)                                                                                                                   
                                                                                                                                                               
  t1 = 0.0 ──── blend ──── 1.0                                                                                                                                   
       │       margin │        │                                                                                                                                 
       edge           │      centre                                                                                                                              
       │←─BLEND_MARGIN─►│                                                                                                                                        
                                                                                                                                                                 
  At the very edge of the 1m tile t1 = 0 (use whatever 5m computed). At BLEND_MARGIN metres inward t1 = 1 (use 1m fully). In between it's a smooth cubic ramp —  
  no visible seam.                                                                                                                                             
                                                                                                                                                                 
  ---                                                                        
  Step 4 — Convert tile-local position to a pixel address (L393–404)                                                                                           
                                                                                                                                                                 
  This is the core of bilinear interpolation. Picture a 4-pixel patch in the data buffer:
                                                                                                                                                                 
  pixel space (floating-point)                                               
  ─────────────────────────────                                                                                                                                  
    c1       c1+1                                                            
    │         │                                                                                                                                                  
  r1 ── [00] ── [10] ──                                                      
    │         │                                                                                                                                                  
  r1+1── [01] ── [11] ──                                                     
    │         │                                                                                                                                                  
   fx1 = fractional column  (0..1)                                                                                                                               
   fy1 = fractional row     (0..1)                                                                                                                               
                                                                                                                                                                 
  dx1 = extent_x / cols    ← metres per pixel (≈1.0 for 1m tier)                                                                                                 
  c1_f = lx1 / dx1         ← floating-point column index                                                                                                         
  c1   = floor(c1_f)       ← integer column of left pixel                                                                                                        
  fx1  = c1_f - c1         ← how far right within that pixel (0..1)                                                                                              
                                                                                                                                                                 
  same for rows → r1, fy1                                                                                                                                        
                                                                                                                                                                 
  The four flat buffer indices:                                                                                                                                  
                                                                                                                                                               
  i1_00 = r1   * cols + c1       (top-left)                                                                                                                    
  i1_10 = r1   * cols + (c1+1)   (top-right)                                                                                                                     
  i1_01 = (r1+1) * cols + c1     (bottom-left)
  i1_11 = (r1+1) * cols + (c1+1) (bottom-right)                                                                                                                  
                                                                             
  ---                                                                                                                                                            
  Step 5 — Bilinear interpolation of the normal (L405–410)                   
                                                                                                                                                                 
  n1 = bilinear(hm1m_nx/ny/nz at the 4 corners)                              
                                                                                                                                                                 
                fx1
           ←────────►                                                                                                                                            
      [00]────────────[10]     ▲                                             
        │    mix(H, H, fx1)    │ fy1                                                                                                                             
      [01]────────────[11]     ▼                                                                                                                                 
                │                                                                                                                                                
                mix(top_row, bot_row, fy1)                                                                                                                       
                                                                                                                                                                 
  Two horizontal lerps (across fx1), then one vertical lerp (across fy1). Result: a smooth normal that slides continuously across pixel boundaries — no 1m grid  
  lines visible in lighting.                                                                                                                                     
                                                                                                                                                                 
  hm1m_nx/ny/nz are separate flat arrays (SoA layout) — the same pattern used for the base and 5m tiers.                                                         
                                                                                                                                                               
  ---                                                                                                                                                            
  Step 6 — Same bilinear for shadow (L411–412)                                                                                                                 
                                                                                                                                                                 
  Shadow is a single f32 per pixel (0 = lit, 1 = in shadow). Same 4-corner bilinear gives a smooth shadow boundary.
                                                                                                                                                                 
  ---                                                                                                                                                          
  Step 7 — Blend the 1m result over whatever 5m computed (L415–417)                                                                                              
                                                                                                                                                                 
  normal    = mix(normal_from_5m_block,    n1,  t1)                                                                                                            
  in_shadow = mix(shadow_from_5m_block,   sh1,  t1)                                                                                                              
  hit_uv    = mix(uv_from_5m_block, fine_uv,  t1)                                                                                                              
                                                                                                                                                                 
  t1 = 1.0 at the centre → 1m wins completely.                                                                                                                   
  t1 = 0.0 at the edge → 5m value passes through unchanged.                                                                                                      
  The edge blend zone means the 1m patch fades in smoothly rather than snapping.                                                                                 
                                                                                                                                                                 
  ---                                                                                                                                                          
  The full tier stack at one pixel                                                                                                                               
                                                                                                                                                                 
                      BASE 30m  (always)                                                                                                                       
                          │                                                                                                                                      
              ┌───────────┴───────────┐                                                                                                                          
              │  inside 5m window?    │                                                                                                                        
              │  t5 = smoothstep(…)   │                                                                                                                          
              │  mix(base, 5m, t5)    │                                                                                                                          
              └───────────┬───────────┘                                                                                                                          
                          │                                                                                                                                      
              ┌───────────┴───────────┐                                                                                                                        
              │  inside 1m window?    │                                                                                                                          
              │  t1 = smoothstep(…)   │
              │  mix(prev, 1m, t1)    │   ← L386–419 is this box                                                                                                 
              └───────────┬───────────┘                                                                                                                          
                          │                                                                                                                                    
                     final normal,                                                                                                                               
                     shadow, uv                                                                                                                                  
                                                                                                                                                               
  Each tier overrides the previous one with a smooth smoothstep blend at its boundary — so all three data sources contribute without hard seams.

  
  // ── close tier normals (texture sample) and shadow (buffer bilinear) ──
            let close_uv = vec2<f32>(lx_hit / cam.hm5m_extent_x, ly_hit / cam.hm5m_extent_y);
            let n5_rg = textureSampleLevel(hm5m_normal_tex, hm5m_normal_samp, close_uv, 0.0).rg;
            let n5 = normalize(vec3<f32>(n5_rg.x, n5_rg.y, sqrt(max(0.0, 1.0 - dot(n5_rg, n5_rg)))));
  
  This is the unit sphere constraint — the key invariant that makes storing only 2 components safe.                                                            
                                                                                                                                                                 
  The invariant: every normal coming from compute_normals is a unit vector, meaning its length is exactly 1:                                                     
                                                                                                                                                                 
  x² + y² + z² = 1                                                                                                                                               
                                                                                                                                                                 
  So if you know x and y, you can always recover z:                                                                                                              
                                                                                                                                                                 
  z² = 1 - x² - y²                                                                                                                                               
  z  = sqrt(1 - x² - y²)                                              
                                         
  Why the dot product? dot(v, v) is just a shorthand for squaring all components and summing them — exactly x² + y² for a 2D vector:                             
   
  dot(rg, rg) = rg.x*rg.x + rg.y*rg.y = x² + y²                                                                                                                  
                                                                                                                                                                 
  So 1.0 - dot(rg, rg) = 1 - x² - y² = z², and sqrt(...) gives z.                                                                                                
                                                                                                                                                                 
  Visual — the unit sphere:                                                                                                                                      
                                                                      
           z (up)                                                                                                                                                
           │                                                          
      ┌────┼────┐                        
      │    │  * │← some normal (x, y, z)                                                                                                                         
      │    │  /│                                                                                                                                                 
      │    │ / │  z = height above xy-plane                                                                                                                      
      │    │/  │                                                                                                                                                 
      └────┼────┘──── y                                               
          /                                                                                                                                                      
         x                                                                                                                                                       
                                         
  Top-down view (xy-plane):                                                                                                                                      
          y                                                           
          │                              
      ●───┼───●   The circle x²+y²=1 is the
      │   │   │   "maximum reach" — if x²+y²=1                                                                                                                   
      │   │   │   then z=0 (flat surface, 90° slope)                                                                                                             
      ●───┼───●                                                                                                                                                  
          │      x                                                                                                                                               
                                                                                                                                                                 
     x²+y² < 1 → the normal tilts upward → z > 0                                                                                                                 
     x²+y² = 0 → straight up → z = 1     
     x²+y² = 1 → horizontal → z = 0                                                                                                                              
                                                                                                                                                                 
  Why max(0.0, ...)? Floating-point rounding during encoding (* 127.0, round, cast to i8, then GPU decodes back) can produce values where x² + y² is slightly    
  greater than 1.0 by a tiny epsilon. Without the clamp, you'd be sqrt-ing a negative number → NaN → black pixel. The max(0, ...) makes this safe.               
                                                                                                                                                                 
  Why normalize after reconstruction? The Rgba8Snorm decode has ~0.8% precision loss (127 levels per unit vs exact float). After decoding and reconstructing z,  
  the vector might be length 0.998 instead of 1.0. normalize corrects it back to unit length so the dot product with the sun direction gives the right
  illumination value.                                                                      
  