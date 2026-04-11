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
