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

