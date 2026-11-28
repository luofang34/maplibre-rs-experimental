struct HudFrame {
    viewport_and_panel: vec4<f32>,
    angles_and_interaction: vec4<f32>,
    state_flags: vec4<u32>,
    selection_rect: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> frame: HudFrame;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) logical_pos: vec2<f32>,
    @location(1) @interpolate(flat) quad_kind: u32,
};

fn sd_box(p: vec2<f32>, b: vec2<f32>) -> f32 {
    let d = abs(p) - b;
    return length(max(d, vec2<f32>(0.0, 0.0))) + min(max(d.x, d.y), 0.0);
}

fn sd_rounded_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r, r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0, 0.0))) - r;
}

fn sd_segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

fn aa_coverage(dist: f32) -> f32 {
    return 1.0 - smoothstep(-0.65, 0.65, dist);
}

// 3x5 bitmap font encoded as a 15-bit integer (row-major, 3 bits per row, top to bottom).
fn glyph_bits(ch: u32) -> u32 {
    switch ch {
        case 2u: { return 0x71747u; } // '2': 111 / 001 / 111 / 100 / 111 -> encoded 3-bit rows
        default: { return 0u; }
    }
}

fn sample_glyph_3x5(p: vec2<f32>, rows: array<u32, 5>) -> f32 {
    // p in [0..3, 0..5]
    if p.x < -0.4 || p.x > 3.4 || p.y < -0.4 || p.y > 5.4 {
        return 10.0;
    }
    var min_d = 10.0;
    for (var r: i32 = 0; r < 5; r = r + 1) {
        let bits = rows[r];
        for (var c: i32 = 0; c < 3; c = c + 1) {
            if ((bits >> u32(2 - c)) & 1u) != 0u {
                let cell_center = vec2<f32>(f32(c) + 0.5, f32(r) + 0.5);
                let d = sd_box(p - cell_center, vec2<f32>(0.52, 0.52));
                min_d = min(min_d, d);
            }
        }
    }
    return min_d;
}

fn sd_char(p: vec2<f32>, ch: u32) -> f32 {
    // ch codes:
    // 0='2', 1='3', 2='D', 3='A', 4='U', 5='T', 6='O', 7='M', 8='N', 9='P', 10='R', 11='S'
    switch ch {
        case 0u: { // '2'
            return sample_glyph_3x5(p, array<u32, 5>(7u, 1u, 7u, 4u, 7u));
        }
        case 1u: { // '3'
            return sample_glyph_3x5(p, array<u32, 5>(7u, 1u, 7u, 1u, 7u));
        }
        case 2u: { // 'D'
            return sample_glyph_3x5(p, array<u32, 5>(6u, 5u, 5u, 5u, 6u));
        }
        case 3u: { // 'A'
            return sample_glyph_3x5(p, array<u32, 5>(2u, 5u, 7u, 5u, 5u));
        }
        case 4u: { // 'U'
            return sample_glyph_3x5(p, array<u32, 5>(5u, 5u, 5u, 5u, 7u));
        }
        case 5u: { // 'T'
            return sample_glyph_3x5(p, array<u32, 5>(7u, 2u, 2u, 2u, 2u));
        }
        case 6u: { // 'O'
            return sample_glyph_3x5(p, array<u32, 5>(7u, 5u, 5u, 5u, 7u));
        }
        case 7u: { // 'M'
            return sample_glyph_3x5(p, array<u32, 5>(5u, 7u, 7u, 5u, 5u));
        }
        case 8u: { // 'N'
            return sample_glyph_3x5(p, array<u32, 5>(5u, 7u, 7u, 7u, 5u));
        }
        case 9u: { // 'P'
            return sample_glyph_3x5(p, array<u32, 5>(7u, 5u, 7u, 4u, 4u));
        }
        case 10u: { // 'R'
            return sample_glyph_3x5(p, array<u32, 5>(6u, 5u, 6u, 5u, 5u));
        }
        default: { // 'S'
            return sample_glyph_3x5(p, array<u32, 5>(7u, 4u, 7u, 1u, 7u));
        }
    }
}

fn premul(rgb: vec3<f32>, a: f32) -> vec4<f32> {
    return vec4<f32>(rgb * a, a);
}

fn blend_over(dst: vec4<f32>, src: vec4<f32>) -> vec4<f32> {
    return src + dst * (1.0 - src.a);
}

fn shade_selection_box(pos: vec2<f32>) -> vec4<f32> {
    let min_p = frame.selection_rect.xy;
    let max_p = frame.selection_rect.zw;
    let center = (min_p + max_p) * 0.5;
    let half_size = max((max_p - min_p) * 0.5, vec2<f32>(1.0, 1.0));
    let d = sd_box(pos - center, half_size);

    let rts_mode = (frame.state_flags.x & 8u) != 0u;
    let accent = select(
        vec3<f32>(0.22, 0.74, 0.97), // Sky cyan for Map Shift-box
        vec3<f32>(0.29, 0.87, 0.50), // Tactical emerald for RTS box
        rts_mode,
    );

    var color = vec4<f32>(0.0);
    if d <= 0.0 {
        color = blend_over(color, premul(accent, 0.14));
    }
    let border_cov = aa_coverage(abs(d) - 0.85);
    if border_cov > 0.0 {
        color = blend_over(color, premul(accent, border_cov * 0.92));
    }
    // Corner brackets (RA2 / Generals style)
    let corner_dist = abs(pos - center) - (half_size - vec2<f32>(6.0, 6.0));
    if abs(d) <= 1.6 && (corner_dist.x > 0.0 && corner_dist.y > 0.0) {
        color = blend_over(color, premul(vec3<f32>(1.0, 1.0, 1.0), aa_coverage(abs(d) - 1.2) * 0.95));
    }
    return color;
}

@fragment
fn main(in: VertexOutput) -> @location(0) vec4<f32> {
    if in.quad_kind == 1u {
        return shade_selection_box(in.logical_pos);
    }

    let p = in.logical_pos - frame.viewport_and_panel.zw;
    let panel_half = vec2<f32>(19.0, 93.0);
    let panel_center = vec2<f32>(19.0, 93.0);
    let d_panel = sd_rounded_box(p - panel_center, panel_half, 11.0);

    // Drop shadow
    let shadow_d = sd_rounded_box(p - (panel_center + vec2<f32>(0.0, 2.0)), panel_half, 11.0);
    let shadow_alpha = clamp(exp(-max(shadow_d, 0.0) * 0.45) * 0.36, 0.0, 0.36);
    var out = premul(vec3<f32>(0.0, 0.0, 0.0), shadow_alpha * step(0.0, d_panel));

    let panel_cov = aa_coverage(d_panel);
    if panel_cov <= 0.0 {
        return out;
    }

    // Frosted dark slate background
    var panel_rgb = vec3<f32>(0.065, 0.086, 0.125);
    let hovered = i32(frame.angles_and_interaction.z);
    let pressed = i32(frame.angles_and_interaction.w);

    // Determine which slot p.y falls in:
    // 0: 0..40, 1: 40..74, 2: 74..104, 3: 104..133, 4: 133..160, 5: 160..186
    var slot: i32 = 5;
    if p.y < 40.0 {
        slot = 0;
    } else if p.y < 74.0 {
        slot = 1;
    } else if p.y < 104.0 {
        slot = 2;
    } else if p.y < 133.0 {
        slot = 3;
    } else if p.y < 160.0 {
        slot = 4;
    }

    if slot == pressed {
        panel_rgb = vec3<f32>(0.15, 0.21, 0.30);
    } else if slot == hovered {
        panel_rgb = vec3<f32>(0.11, 0.15, 0.22);
    }
    out = blend_over(out, premul(panel_rgb, panel_cov * 0.90));

    let flags = frame.state_flags.x;
    let three_d = (flags & 1u) != 0u;
    let auto_tilt = (flags & 2u) != 0u;
    let north_locked = (flags & 4u) != 0u;
    let rts_mode = (flags & 8u) != 0u;

    // Separators
    if abs(p.x - 19.0) < 12.0 {
        let sep_d = min(
            min(abs(p.y - 40.0), abs(p.y - 74.0)),
            min(abs(p.y - 104.0), min(abs(p.y - 133.0), abs(p.y - 160.0))),
        );
        let sep_cov = aa_coverage(sep_d - 0.35);
        if sep_cov > 0.0 {
            out = blend_over(out, premul(vec3<f32>(1.0, 1.0, 1.0), sep_cov * 0.10 * panel_cov));
        }
    }

    // 1. Compass (center = 19, 20)
    if p.y < 41.0 {
        let c = p - vec2<f32>(19.0, 20.0);
        let ring_d = abs(length(c) - 13.0) - 0.85;
        let ring_color = select(
            vec3<f32>(0.45, 0.53, 0.65),
            vec3<f32>(0.22, 0.74, 0.97),
            north_locked,
        );
        let ring_cov = aa_coverage(ring_d);
        if ring_cov > 0.0 {
            out = blend_over(out, premul(ring_color, ring_cov * select(0.55, 0.95, north_locked)));
        }

        // 3D foreshortened compass needle
        let bearing = frame.angles_and_interaction.x;
        let pitch = frame.angles_and_interaction.y;
        let pitch_scale = clamp(cos(pitch * 0.72), 0.42, 1.0);
        let c_unpitched = vec2<f32>(c.x, c.y / pitch_scale);
        let s = sin(bearing);
        let cs = cos(bearing);
        // Rotate by +bearing so the needle points to map North (-bearing on screen)
        let n_pos = vec2<f32>(cs * c_unpitched.x - s * c_unpitched.y, s * c_unpitched.x + cs * c_unpitched.y);

        // Diamond needle: width tapers from 3.4 at y=0 to 0 at y=+-9.5
        let ny = abs(n_pos.y);
        let half_w = clamp((1.0 - ny / 9.5) * 3.1, 0.0, 3.1);
        let needle_d = max(abs(n_pos.x) - half_w, ny - 9.5);
        let needle_cov = aa_coverage(needle_d);
        if needle_cov > 0.0 {
            let needle_rgb = select(
                vec3<f32>(0.78, 0.83, 0.90), // South: silver
                vec3<f32>(0.96, 0.25, 0.37), // North: coral red
                n_pos.y < 0.0,
            );
            out = blend_over(out, premul(needle_rgb, needle_cov * 0.98));
        }

        // Center pivot dot
        let pivot_cov = aa_coverage(length(c) - 1.5);
        if pivot_cov > 0.0 {
            out = blend_over(out, premul(vec3<f32>(0.95, 0.97, 1.0), pivot_cov));
        }

        // Lock indicator dot at top of ring when north_locked
        if north_locked {
            let lock_cov = aa_coverage(length(c - vec2<f32>(0.0, -13.0)) - 2.1);
            if lock_cov > 0.0 {
                out = blend_over(out, premul(vec3<f32>(0.22, 0.74, 0.97), lock_cov));
            }
        }
    }

    // 2. 3D / 2D + AUTO / MAN (y: 40..74)
    if p.y >= 40.0 && p.y < 74.0 {
        // Draw '3D' or '2D' centered at (19, 50), scale = 1.65px per cell
        let scale = 1.65;
        let g_origin = vec2<f32>(19.0 - 3.5 * scale, 45.5);
        let gp = (p - g_origin) / scale;
        let first_digit = select(0u, 1u, three_d); // 0u='2', 1u='3'
        let d_digit = sd_char(gp, first_digit) * scale;
        let d_d = sd_char(gp - vec2<f32>(4.0, 0.0), 2u) * scale; // 2u='D'
        let text_cov = aa_coverage(min(d_digit, d_d));
        if text_cov > 0.0 {
            let text_rgb = select(vec3<f32>(0.68, 0.75, 0.85), vec3<f32>(0.96, 0.98, 1.0), three_d);
            out = blend_over(out, premul(text_rgb, text_cov));
        }

        // Sub-badge at y = 64.5
        let badge_c = p - vec2<f32>(19.0, 64.5);
        let badge_d = sd_rounded_box(badge_c, vec2<f32>(13.5, 5.2), 2.6);
        let badge_cov = aa_coverage(badge_d);
        if badge_cov > 0.0 {
            var badge_bg = vec3<f32>(0.18, 0.23, 0.31);
            if three_d && auto_tilt {
                badge_bg = vec3<f32>(0.08, 0.42, 0.34);
            } else if three_d {
                badge_bg = vec3<f32>(0.48, 0.30, 0.08);
            }
            out = blend_over(out, premul(badge_bg, badge_cov * 0.95));
        }

        // Sub-badge text: 'AUTO' (4 chars) or 'MAN' (3 chars) or '2D'
        let sub_scale = 0.95;
        if three_d && auto_tilt {
            let sp = (p - vec2<f32>(19.0 - 7.5 * sub_scale, 64.5 - 2.5 * sub_scale)) / sub_scale;
            let da = min(
                min(sd_char(sp, 3u), sd_char(sp - vec2<f32>(4.0, 0.0), 4u)),
                min(sd_char(sp - vec2<f32>(8.0, 0.0), 5u), sd_char(sp - vec2<f32>(12.0, 0.0), 6u)),
            ) * sub_scale;
            let scov = aa_coverage(da);
            if scov > 0.0 {
                out = blend_over(out, premul(vec3<f32>(0.45, 0.96, 0.78), scov));
            }
        } else if three_d {
            let sp = (p - vec2<f32>(19.0 - 5.5 * sub_scale, 64.5 - 2.5 * sub_scale)) / sub_scale;
            let dm = min(
                sd_char(sp, 7u),
                min(sd_char(sp - vec2<f32>(4.0, 0.0), 3u), sd_char(sp - vec2<f32>(8.0, 0.0), 8u)),
            ) * sub_scale;
            let scov = aa_coverage(dm);
            if scov > 0.0 {
                out = blend_over(out, premul(vec3<f32>(0.99, 0.78, 0.35), scov));
            }
        }
    }

    // 3. Zoom In '+' (center = 19, 89)
    if p.y >= 74.0 && p.y < 104.0 {
        let c = p - vec2<f32>(19.0, 89.0);
        let d_plus = min(sd_box(c, vec2<f32>(5.5, 0.95)), sd_box(c, vec2<f32>(0.95, 5.5)));
        let cov = aa_coverage(d_plus);
        if cov > 0.0 {
            out = blend_over(out, premul(vec3<f32>(0.92, 0.95, 0.99), cov));
        }
    }

    // 4. Zoom Out '-' (center = 19, 118.5)
    if p.y >= 104.0 && p.y < 133.0 {
        let c = p - vec2<f32>(19.0, 118.5);
        let d_minus = sd_box(c, vec2<f32>(5.5, 0.95));
        let cov = aa_coverage(d_minus);
        if cov > 0.0 {
            out = blend_over(out, premul(vec3<f32>(0.92, 0.95, 0.99), cov));
        }
    }

    // 5. Home button (center = 19, 146.5)
    if p.y >= 133.0 && p.y < 160.0 {
        let c = p - vec2<f32>(19.0, 147.0);
        let roof_l = sd_segment(c, vec2<f32>(-6.2, -0.5), vec2<f32>(0.0, -6.2)) - 0.9;
        let roof_r = sd_segment(c, vec2<f32>(0.0, -6.2), vec2<f32>(6.2, -0.5)) - 0.9;
        let walls = max(sd_box(c - vec2<f32>(0.0, 2.2), vec2<f32>(4.2, 3.6)), -sd_box(c - vec2<f32>(0.0, 3.5), vec2<f32>(1.4, 2.4)));
        let d_home = min(min(roof_l, roof_r), walls);
        let cov = aa_coverage(d_home);
        if cov > 0.0 {
            out = blend_over(out, premul(vec3<f32>(0.88, 0.92, 0.98), cov));
        }
    }

    // 6. MAP / RTS Mode Pill (center = 19, 173)
    if p.y >= 160.0 {
        let badge_c = p - vec2<f32>(19.0, 173.0);
        let badge_d = sd_rounded_box(badge_c, vec2<f32>(13.5, 6.5), 3.2);
        let badge_cov = aa_coverage(badge_d);
        if badge_cov > 0.0 {
            let bg = select(
                vec3<f32>(0.15, 0.22, 0.32),
                vec3<f32>(0.10, 0.42, 0.25),
                rts_mode,
            );
            out = blend_over(out, premul(bg, badge_cov * 0.95));
        }
        let sub_scale = 1.05;
        let sp = (p - vec2<f32>(19.0 - 5.5 * sub_scale, 173.0 - 2.5 * sub_scale)) / sub_scale;
        let d_mode = select(
            // 'MAP': 7u='M', 3u='A', 9u='P'
            min(sd_char(sp, 7u), min(sd_char(sp - vec2<f32>(4.0, 0.0), 3u), sd_char(sp - vec2<f32>(8.0, 0.0), 9u))),
            // 'RTS': 10u='R', 5u='T', 11u='S'
            min(sd_char(sp, 10u), min(sd_char(sp - vec2<f32>(4.0, 0.0), 5u), sd_char(sp - vec2<f32>(8.0, 0.0), 11u))),
            rts_mode,
        ) * sub_scale;
        let mcov = aa_coverage(d_mode);
        if mcov > 0.0 {
            let fg = select(vec3<f32>(0.72, 0.82, 0.94), vec3<f32>(0.55, 0.98, 0.72), rts_mode);
            out = blend_over(out, premul(fg, mcov));
        }
    }

    // Subtle 1px outer border highlight on the capsule
    let border_cov = aa_coverage(abs(d_panel) - 0.55);
    if border_cov > 0.0 {
        out = blend_over(out, premul(vec3<f32>(1.0, 1.0, 1.0), border_cov * 0.16));
    }

    return out;
}
