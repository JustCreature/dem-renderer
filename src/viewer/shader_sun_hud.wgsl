// TAU = 2π = one full turn in radians.  Used everywhere instead of "2.0 * PI"
// so formulas read as "fraction of a full circle * TAU" rather than "* 2 * PI".
const TAU: f32 = 6.28318530718;

// Uniform block uploaded from Rust every frame (48 bytes).
// cx1/cy1 = centre of the season circle (top one).
// cx2/cy2 = centre of the time circle   (bottom one).
// day_angle  = needle angle for season circle: 0 = top = Jun 21, increases clockwise.
// hour_angle = needle angle for time circle:   0 = top = 12:00,  increases clockwise.
struct SunHud {
    screen_w: f32,
    screen_h: f32,
    cx1: f32,
    cy1: f32,   // season circle centre (pixel coords, Y-down)
    cx2: f32,
    cy2: f32,   // time circle centre
    radius: f32,
    day_angle: f32,
    hour_angle: f32,
    _pad1: f32,  // padding to reach 48-byte std140 alignment
    _pad2: f32,
    _pad3: f32,
}

@group(0) @binding(0) var<uniform> u: SunHud;

// ── Vertex shader ─────────────────────────────────────────────────────────────
// Passes NDC positions straight through.  The CPU sends a full-screen quad
// (two triangles covering −1..+1 in both axes) so every screen pixel gets a
// fragment invocation and we can do all the circle math per-pixel.
@vertex
fn vs_main(@location(0) pos: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(pos.x, pos.y, 0.0, 1.0);
}

// ── Helper: SDF of line segment a→b ──────────────────────────────────────────
// Returns the shortest distance from pixel p to the segment [a, b].
// Used to draw the needle and tick marks as anti-aliased lines.
//
// How it works:
//   1. Project p onto the infinite line through a and b.
//      t = dot(p-a, b-a) / |b-a|²  gives a 0..1 parameter along the segment.
//   2. Clamp t to [0,1] so the closest point stays on the segment (not beyond).
//   3. Distance = length from p to that closest point.
fn seg_sdf(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ba = b - a;                              // direction vector of the segment
    let pa = p - a;                              // vector from segment start to pixel
    let t = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);  // clamped projection
    return length(pa - ba * t);                  // distance to nearest point on segment
}

// ── Helper: clockwise angle from top ─────────────────────────────────────────
// Returns the angle of pixel p around `center`, measured clockwise from 12 o'clock.
//   0        = straight up    (12 o'clock)
//   TAU / 4  = right          ( 3 o'clock)
//   TAU / 2  = straight down  ( 6 o'clock)
//   TAU*3/4  = left           ( 9 o'clock)
//
// Standard atan2(y, x) gives counter-clockwise angle from the positive X axis.
// We want clockwise from the positive Y axis (= up on screen).
// Two adjustments:
//   • Swap x and y arguments  → rotates reference to the vertical axis.
//   • Negate dp.y             → flips Y because screen Y grows downward.
// The result can be negative for the left half; adding TAU wraps it to [0, TAU).
fn frag_angle(p: vec2<f32>, center: vec2<f32>) -> f32 {
    let dp = p - center;
    var a = atan2(dp.x, -dp.y);   // clockwise from top; may be in (−π, 0) for left half
    if a < 0.0 { a += TAU; }      // remap to [0, TAU)
    return a;
}

// ── Helper: season colour from angular position ───────────────────────────────
// The season circle's ring is coloured by which season each arc position falls in.
// angle=0 is the summer solstice (top); the year runs clockwise.
// Season boundaries are the number of days after Jun 21, converted to radians.
fn season_col(angle: f32) -> vec3<f32> {
    let fa = (93.0 / 365.0) * TAU;  // fall start   (~Sep 22, 93 days after Jun 21)
    let wi = (183.0 / 365.0) * TAU;  // winter start (~Dec 21, 183 days after Jun 21)
    let sp = (273.0 / 365.0) * TAU;  // spring start (~Mar 20, 273 days after Jun 21)
    if angle < fa { return vec3<f32>(0.95, 0.82, 0.08); }  // summer → yellow
    if angle < wi { return vec3<f32>(0.90, 0.45, 0.10); }  // fall   → orange
    if angle < sp { return vec3<f32>(0.45, 0.65, 0.95); }  // winter → blue
    return vec3<f32>(0.25, 0.80, 0.30);             // spring → green
}

// ── Helper: draw one tick mark ────────────────────────────────────────────────
// A tick is a short radial line segment at a given angle on the ring.
// `inner` is the fraction of the radius where the tick starts (0.72 = 72% out).
// Returns the SDF distance so the caller can blend it into the colour.
fn tick(p: vec2<f32>, center: vec2<f32>, r: f32, angle: f32, inner: f32) -> f32 {
    // Direction vector pointing outward at `angle` (clockwise-from-top convention).
    // sin/−cos converts the clock angle to a screen-space 2D unit vector.
    let dir = vec2<f32>(sin(angle), -cos(angle));
    let a = center + dir * (r * inner);  // inner end of tick (72% of radius)
    let b = center + dir * r;            // outer end of tick (at the ring)
    return seg_sdf(p, a, b);
}

// ── Main circle drawing function ──────────────────────────────────────────────
// Draws one complete circle (background disc + coloured ring + tick marks +
// needle + centre dot) and returns the RGBA colour for the current pixel.
// kind=0 → season circle (coloured ring, season tints)
// kind=1 → time circle   (white ring, dark background)
// needle → angle of the yellow needle (day_angle or hour_angle from the uniform)
fn draw_circle(p: vec2<f32>, center: vec2<f32>, r: f32, needle: f32, kind: i32) -> vec4<f32> {
    let d = length(p - center);           // pixel's distance from circle centre
    let fa = frag_angle(p, center);        // pixel's clockwise angle from top

    // Early exit: pixel is outside the circle (with 1.5px anti-alias margin).
    // This is a secondary guard; fs_main already discards most far pixels.
    if d > r + 1.5 { return vec4<f32>(0.0); }

    var col = vec4<f32>(0.0);  // start fully transparent

    // ── Background disc ───────────────────────────────────────────────────────
    // Fill the interior with a semi-transparent tint so the terrain shows through.
    // Season circle: dim version of the season colour at that angle (22% brightness).
    // Time circle:   uniform dark grey.
    if d < r {
        if kind == 0 {
            let sc = season_col(fa);
            col = vec4<f32>(sc * 0.22, 0.60);      // dim season tint, 60% opaque
        } else {
            col = vec4<f32>(0.0, 0.0, 0.0, 0.55);  // dark, 55% opaque
        }
    }

    // ── Outer ring ────────────────────────────────────────────────────────────
    // A thin band around d ≈ r, anti-aliased by blending over 1.8 pixels.
    // ring_d = distance from the ring edge (0 = exactly on the ring).
    // Colour: season ring uses the season colour; time ring is white.
    // mix(existing, new, weight) blends smoothly rather than hard-replacing.
    let ring_d = abs(d - r);
    if ring_d < 1.8 {
        var rc = vec3<f32>(1.0);                       // default white for time circle
        if kind == 0 { rc = season_col(fa); }          // season colour for season circle
        col = mix(col, vec4<f32>(rc, 1.0), 1.0 - ring_d / 1.8);
        // weight = 1.0 at ring_d=0 (dead centre of ring), 0.0 at ring_d=1.8 (edge)
    }

    // ── Tick marks ────────────────────────────────────────────────────────────
    // Four white radial lines at the cardinal positions of each circle.
    // The tick SDF returns a distance; if < 1.5px we paint it white with
    // the same linear anti-alias blend as the ring.
    if kind == 0 {
        // Season circle: solstices and equinoxes at 90° intervals.
        // Angles match the season_col boundaries (same convention: 0 = top = summer).
        let t0 = 0.0;                           // Jun 21 summer solstice  → top
        let t1 = (93.0 / 365.0) * TAU;        // Sep 22 fall equinox     → right
        let t2 = (183.0 / 365.0) * TAU;        // Dec 21 winter solstice  → bottom
        let t3 = (273.0 / 365.0) * TAU;        // Mar 20 spring equinox   → left
        if tick(p, center, r, t0, 0.72) < 1.5 { col = mix(col, vec4<f32>(1.0, 1.0, 1.0, 1.0), 1.0 - tick(p, center, r, t0, 0.72) / 1.5); }
        if tick(p, center, r, t1, 0.72) < 1.5 { col = mix(col, vec4<f32>(1.0, 1.0, 1.0, 1.0), 1.0 - tick(p, center, r, t1, 0.72) / 1.5); }
        if tick(p, center, r, t2, 0.72) < 1.5 { col = mix(col, vec4<f32>(1.0, 1.0, 1.0, 1.0), 1.0 - tick(p, center, r, t2, 0.72) / 1.5); }
        if tick(p, center, r, t3, 0.72) < 1.5 { col = mix(col, vec4<f32>(1.0, 1.0, 1.0, 1.0), 1.0 - tick(p, center, r, t3, 0.72) / 1.5); }
    } else {
        // Time circle: 12-hour clock face, evenly spaced at TAU/4 intervals.
        let h12 = 0.0;               // 12:00 → top     (0   radians)
        let h15 = TAU / 4.0;        // 15:00 → right   (π/2 radians)
        let h18 = TAU / 2.0;        // 18:00 → bottom  (π   radians)
        let h21 = TAU * 3.0 / 4.0;  // 21:00 → left    (3π/2 radians)
        if tick(p, center, r, h12, 0.72) < 1.5 { col = mix(col, vec4<f32>(1.0, 1.0, 1.0, 0.9), 1.0 - tick(p, center, r, h12, 0.72) / 1.5); }
        if tick(p, center, r, h15, 0.72) < 1.5 { col = mix(col, vec4<f32>(1.0, 1.0, 1.0, 0.9), 1.0 - tick(p, center, r, h15, 0.72) / 1.5); }
        if tick(p, center, r, h18, 0.72) < 1.5 { col = mix(col, vec4<f32>(1.0, 1.0, 1.0, 0.9), 1.0 - tick(p, center, r, h18, 0.72) / 1.5); }
        if tick(p, center, r, h21, 0.72) < 1.5 { col = mix(col, vec4<f32>(1.0, 1.0, 1.0, 0.9), 1.0 - tick(p, center, r, h21, 0.72) / 1.5); }
    }

    // ── Yellow needle ─────────────────────────────────────────────────────────
    // A line from the centre to (r − 9) pixels out in the needle direction.
    // Only drawn inside the disc (d < r − 3) so it doesn't bleed onto the ring.
    // The needle direction uses the same sin/−cos clock-angle conversion as tick().
    if d < r - 3.0 {
        let tip = center + vec2<f32>(sin(needle), -cos(needle)) * (r - 9.0);
        let nd = seg_sdf(p, center, tip);
        if nd < 2.0 {
            // Blend yellow over whatever colour is already there.
            // Weight = 1 at the centreline, 0 at 2px away → smooth anti-aliased line.
            col = mix(col, vec4<f32>(1.0, 0.85, 0.15, 1.0), 1.0 - nd / 2.0);
        }
    }

    // ── White centre dot ──────────────────────────────────────────────────────
    // A solid white disc of radius 3.5px hides the messy needle base and
    // gives the clock a clean pivot point.
    if d < 3.5 {
        col = vec4<f32>(1.0, 1.0, 1.0, 1.0);
    }

    return col;
}

// ── Helper: HUD panel background SDF ─────────────────────────────────────────
// Returns the signed distance for a single rounded rectangle that covers the
// entire HUD widget: both circles + all surrounding cardinal labels + the
// current-value labels to the left.
// Convention: negative = inside, 0 = boundary, positive = outside.
//
// Layout (all relative to the shared circle centre cx = cx1 = cx2):
//   left  : cx − radius − 112  (past the current-value label, ~10 px padding)
//   right : cx + radius + 72   (past the Fall/15:00 label, ~8 px padding)
//   top   : cy1 − radius − 28  (above the Summer/12:00 label)
//   bottom: cy2 + radius + 28  (below the Winter/18:00 label)
fn panel_rect_sdf(p: vec2<f32>) -> f32 {
    let cx = u.cx1;   // cx1 == cx2: both circles share the same X
    let r  = u.radius;
    let x0 = cx - r - 112.0;
    let x1 = cx + r + 72.0;
    let y0 = u.cy1 - r - 28.0;
    let y1 = u.cy2 + r + 28.0;
    // Rounded-rect SDF (corner radius 8 px for a smooth panel feel).
    let center   = vec2<f32>((x0 + x1) * 0.5, (y0 + y1) * 0.5);
    let half_ext = vec2<f32>((x1 - x0) * 0.5, (y1 - y0) * 0.5);
    let cr = 8.0;
    let q = abs(p - center) - half_ext + vec2<f32>(cr);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - cr;
}

// ── Fragment shader (entry point) ─────────────────────────────────────────────
// Runs once per pixel of the full-screen quad.
// Rendering order (back to front):
//   1. Panel background — dark semi-transparent rounded rect behind everything.
//   2. Circles          — drawn on top of the panel wherever they overlap.
@fragment
fn fs_main(@builtin(position) frag_pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p  = frag_pos.xy;
    let c1 = vec2<f32>(u.cx1, u.cy1);
    let c2 = vec2<f32>(u.cx2, u.cy2);
    let r  = u.radius;

    let d1 = length(p - c1);   // distance to season circle centre
    let d2 = length(p - c2);   // distance to time circle centre
    let pd = panel_rect_sdf(p); // signed distance to the panel background rect

    // Discard pixels that are outside both circles and outside the panel.
    if d1 > r + 1.5 && d2 > r + 1.5 && pd > 1.0 { discard; }

    // ── Circles (drawn first so they appear above the panel background) ────────
    // Each pixel belongs to at most one circle; check season circle first.
    if d1 <= r + 1.5 {
        return draw_circle(p, c1, r, u.day_angle, 0);   // season circle
    }
    if d2 <= r + 1.5 {
        return draw_circle(p, c2, r, u.hour_angle, 1);  // time circle
    }

    // ── Panel background (pixels inside the panel but outside both circles) ────
    // Alpha fades from 0.60 deep inside to 0 at 1 px outside the rounded edge.
    let alpha = 0.60 * clamp(1.0 - pd, 0.0, 1.0);
    return vec4<f32>(0.05, 0.05, 0.05, alpha);
}
