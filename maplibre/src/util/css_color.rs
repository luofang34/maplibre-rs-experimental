//! Compact MapLibre-spec CSS color parser (`#hex`, `rgb`/`rgba`, `hsl`/`hsla`, `hwb`, named colors).

/// Parses a CSS color string into straight sRGB `[r, g, b, a]` in `0.0..=1.0`.
pub(crate) fn parse_css_color(input: &str) -> Option<[f32; 4]> {
    let s = input.trim();
    if s.is_empty() || !s.is_ascii() {
        return None;
    }
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(open) = s.find('(') {
        let close_body = s.strip_suffix(')')?;
        let name = close_body[..open].trim();
        let args = &close_body[open + 1..];
        if name.eq_ignore_ascii_case("rgb") || name.eq_ignore_ascii_case("rgba") {
            return parse_rgb(args);
        }
        if name.eq_ignore_ascii_case("hsl") || name.eq_ignore_ascii_case("hsla") {
            return parse_hsl(args);
        }
        if name.eq_ignore_ascii_case("hwb") {
            return parse_hwb(args);
        }
        return None;
    }
    if s.eq_ignore_ascii_case("transparent") {
        return Some([0.0, 0.0, 0.0, 0.0]);
    }
    let idx = NAMED_COLORS
        .binary_search_by(|(candidate, _)| cmp_ascii_lower(candidate, s))
        .ok()?;
    let [r, g, b] = NAMED_COLORS[idx].1;
    Some([
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        1.0,
    ])
}

fn cmp_ascii_lower(lhs: &str, rhs: &str) -> std::cmp::Ordering {
    for (a, b) in lhs.bytes().zip(rhs.bytes()) {
        match a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()) {
            std::cmp::Ordering::Equal => {}
            ord => return ord,
        }
    }
    lhs.len().cmp(&rhs.len())
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn hex_byte(hi: u8, lo: u8) -> Option<u8> {
    Some((hex_nibble(hi)? << 4) | hex_nibble(lo)?)
}

fn parse_hex(hex: &str) -> Option<[f32; 4]> {
    let b = hex.as_bytes();
    match b.len() {
        3 => Some([
            f32::from(hex_nibble(b[0])? * 17) / 255.0,
            f32::from(hex_nibble(b[1])? * 17) / 255.0,
            f32::from(hex_nibble(b[2])? * 17) / 255.0,
            1.0,
        ]),
        4 => Some([
            f32::from(hex_nibble(b[0])? * 17) / 255.0,
            f32::from(hex_nibble(b[1])? * 17) / 255.0,
            f32::from(hex_nibble(b[2])? * 17) / 255.0,
            f32::from(hex_nibble(b[3])? * 17) / 255.0,
        ]),
        6 => Some([
            f32::from(hex_byte(b[0], b[1])?) / 255.0,
            f32::from(hex_byte(b[2], b[3])?) / 255.0,
            f32::from(hex_byte(b[4], b[5])?) / 255.0,
            1.0,
        ]),
        8 => Some([
            f32::from(hex_byte(b[0], b[1])?) / 255.0,
            f32::from(hex_byte(b[2], b[3])?) / 255.0,
            f32::from(hex_byte(b[4], b[5])?) / 255.0,
            f32::from(hex_byte(b[6], b[7])?) / 255.0,
        ]),
        _ => None,
    }
}

enum Args<'a> {
    Three([&'a str; 3], Option<&'a str>),
}

fn split_func_args(raw: &str) -> Option<Args<'_>> {
    let raw = raw.trim();
    if raw.contains(',') {
        let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
        match parts.as_slice() {
            [a, b, c] => Some(Args::Three([a, b, c], None)),
            [a, b, c, d] => Some(Args::Three([a, b, c], Some(d))),
            _ => None,
        }
    } else if let Some((left, right)) = raw.split_once('/') {
        let mut iter = left.split_whitespace();
        let a = iter.next()?;
        let b = iter.next()?;
        let c = iter.next()?;
        if iter.next().is_some() {
            return None;
        }
        let alpha = right.trim();
        if alpha.is_empty() || alpha.split_whitespace().count() != 1 {
            return None;
        }
        Some(Args::Three([a, b, c], Some(alpha)))
    } else {
        let mut iter = raw.split_whitespace();
        let a = iter.next()?;
        let b = iter.next()?;
        let c = iter.next()?;
        if iter.next().is_some() {
            return None;
        }
        Some(Args::Three([a, b, c], None))
    }
}

fn parse_percent_or_number(tok: &str) -> Option<(f32, bool)> {
    if tok.eq_ignore_ascii_case("none") {
        return Some((0.0, false));
    }
    if let Some(pct) = tok.strip_suffix('%') {
        let v: f32 = pct.parse().ok()?;
        return v.is_finite().then_some((v, true));
    }
    let v: f32 = tok.parse().ok()?;
    v.is_finite().then_some((v, false))
}

fn parse_alpha(tok: Option<&str>) -> Option<f32> {
    let Some(tok) = tok else {
        return Some(1.0);
    };
    let (v, is_pct) = parse_percent_or_number(tok)?;
    let a = if is_pct { v / 100.0 } else { v };
    Some(a.clamp(0.0, 1.0))
}

fn parse_rgb_channel(tok: &str) -> Option<(f32, bool)> {
    let (v, is_pct) = parse_percent_or_number(tok)?;
    let ch = if is_pct { v / 100.0 } else { v / 255.0 };
    Some((ch.clamp(0.0, 1.0), is_pct))
}

fn parse_rgb(raw: &str) -> Option<[f32; 4]> {
    let Args::Three([r_t, g_t, b_t], a_t) = split_func_args(raw)?;
    let (r, r_pct) = parse_rgb_channel(r_t)?;
    let (g, g_pct) = parse_rgb_channel(g_t)?;
    let (b, b_pct) = parse_rgb_channel(b_t)?;
    if raw.contains(',') && !((r_pct == g_pct) && (g_pct == b_pct)) {
        return None;
    }
    let a = parse_alpha(a_t)?;
    Some([r, g, b, a])
}

fn parse_hue_deg(tok: &str) -> Option<f32> {
    if tok.eq_ignore_ascii_case("none") {
        return Some(0.0);
    }
    let lower = tok.to_ascii_lowercase();
    let deg = if let Some(v) = lower.strip_suffix("deg") {
        v.parse::<f32>().ok()?
    } else if let Some(v) = lower.strip_suffix("grad") {
        v.parse::<f32>().ok()? * 0.9
    } else if let Some(v) = lower.strip_suffix("rad") {
        v.parse::<f32>().ok()?.to_degrees()
    } else if let Some(v) = lower.strip_suffix("turn") {
        v.parse::<f32>().ok()? * 360.0
    } else {
        tok.parse::<f32>().ok()?
    };
    deg.is_finite().then(|| deg.rem_euclid(360.0))
}

fn hsl_to_rgb(h_deg: f32, s: f32, l: f32) -> [f32; 3] {
    let hue_to_rgb = |n: f32| -> f32 {
        let k = (n + h_deg / 30.0) % 12.0;
        let a = s * l.min(1.0 - l);
        l - a * (-1.0_f32).max((k - 3.0).min(9.0 - k).min(1.0))
    };
    [hue_to_rgb(0.0), hue_to_rgb(8.0), hue_to_rgb(4.0)]
}

fn parse_hsl(raw: &str) -> Option<[f32; 4]> {
    let Args::Three([h_t, s_t, l_t], a_t) = split_func_args(raw)?;
    let h = parse_hue_deg(h_t)?;
    let (sv, s_pct) = parse_percent_or_number(s_t)?;
    let (lv, l_pct) = parse_percent_or_number(l_t)?;
    if raw.contains(',') && (!s_pct || !l_pct) {
        return None;
    }
    let s = (sv / 100.0).clamp(0.0, 1.0);
    let l = (lv / 100.0).clamp(0.0, 1.0);
    let [r, g, b] = hsl_to_rgb(h, s, l);
    let a = parse_alpha(a_t)?;
    Some([r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0), a])
}

fn parse_hwb(raw: &str) -> Option<[f32; 4]> {
    if raw.contains(',') {
        return None;
    }
    let Args::Three([h_t, w_t, b_t], a_t) = split_func_args(raw)?;
    let h = parse_hue_deg(h_t)?;
    let (wv, _) = parse_percent_or_number(w_t)?;
    let (bv, _) = parse_percent_or_number(b_t)?;
    let w = (wv / 100.0).max(0.0);
    let b = (bv / 100.0).max(0.0);
    let [r, g, bl] = hsl_to_rgb(h, 1.0, 0.5);
    let (w, b) = if w + b > 1.0 {
        let sum = w + b;
        (w / sum, b / sum)
    } else {
        (w, b)
    };
    let factor = 1.0 - w - b;
    let a = parse_alpha(a_t)?;
    Some([
        (r * factor + w).clamp(0.0, 1.0),
        (g * factor + w).clamp(0.0, 1.0),
        (bl * factor + w).clamp(0.0, 1.0),
        a,
    ])
}

const NAMED_COLORS: &[(&str, [u8; 3])] = &[
    ("aliceblue", [240, 248, 255]),
    ("antiquewhite", [250, 235, 215]),
    ("aqua", [0, 255, 255]),
    ("aquamarine", [127, 255, 212]),
    ("azure", [240, 255, 255]),
    ("beige", [245, 245, 220]),
    ("bisque", [255, 228, 196]),
    ("black", [0, 0, 0]),
    ("blanchedalmond", [255, 235, 205]),
    ("blue", [0, 0, 255]),
    ("blueviolet", [138, 43, 226]),
    ("brown", [165, 42, 42]),
    ("burlywood", [222, 184, 135]),
    ("cadetblue", [95, 158, 160]),
    ("chartreuse", [127, 255, 0]),
    ("chocolate", [210, 105, 30]),
    ("coral", [255, 127, 80]),
    ("cornflowerblue", [100, 149, 237]),
    ("cornsilk", [255, 248, 220]),
    ("crimson", [220, 20, 60]),
    ("cyan", [0, 255, 255]),
    ("darkblue", [0, 0, 139]),
    ("darkcyan", [0, 139, 139]),
    ("darkgoldenrod", [184, 134, 11]),
    ("darkgray", [169, 169, 169]),
    ("darkgreen", [0, 100, 0]),
    ("darkgrey", [169, 169, 169]),
    ("darkkhaki", [189, 183, 107]),
    ("darkmagenta", [139, 0, 139]),
    ("darkolivegreen", [85, 107, 47]),
    ("darkorange", [255, 140, 0]),
    ("darkorchid", [153, 50, 204]),
    ("darkred", [139, 0, 0]),
    ("darksalmon", [233, 150, 122]),
    ("darkseagreen", [143, 188, 143]),
    ("darkslateblue", [72, 61, 139]),
    ("darkslategray", [47, 79, 79]),
    ("darkslategrey", [47, 79, 79]),
    ("darkturquoise", [0, 206, 209]),
    ("darkviolet", [148, 0, 211]),
    ("deeppink", [255, 20, 147]),
    ("deepskyblue", [0, 191, 255]),
    ("dimgray", [105, 105, 105]),
    ("dimgrey", [105, 105, 105]),
    ("dodgerblue", [30, 144, 255]),
    ("firebrick", [178, 34, 34]),
    ("floralwhite", [255, 250, 240]),
    ("forestgreen", [34, 139, 34]),
    ("fuchsia", [255, 0, 255]),
    ("gainsboro", [220, 220, 220]),
    ("ghostwhite", [248, 248, 255]),
    ("gold", [255, 215, 0]),
    ("goldenrod", [218, 165, 32]),
    ("gray", [128, 128, 128]),
    ("green", [0, 128, 0]),
    ("greenyellow", [173, 255, 47]),
    ("grey", [128, 128, 128]),
    ("honeydew", [240, 255, 240]),
    ("hotpink", [255, 105, 180]),
    ("indianred", [205, 92, 92]),
    ("indigo", [75, 0, 130]),
    ("ivory", [255, 255, 240]),
    ("khaki", [240, 230, 140]),
    ("lavender", [230, 230, 250]),
    ("lavenderblush", [255, 240, 245]),
    ("lawngreen", [124, 252, 0]),
    ("lemonchiffon", [255, 250, 205]),
    ("lightblue", [173, 216, 230]),
    ("lightcoral", [240, 128, 128]),
    ("lightcyan", [224, 255, 255]),
    ("lightgoldenrodyellow", [250, 250, 210]),
    ("lightgray", [211, 211, 211]),
    ("lightgreen", [144, 238, 144]),
    ("lightgrey", [211, 211, 211]),
    ("lightpink", [255, 182, 193]),
    ("lightsalmon", [255, 160, 122]),
    ("lightseagreen", [32, 178, 170]),
    ("lightskyblue", [135, 206, 250]),
    ("lightslategray", [119, 136, 153]),
    ("lightslategrey", [119, 136, 153]),
    ("lightsteelblue", [176, 196, 222]),
    ("lightyellow", [255, 255, 224]),
    ("lime", [0, 255, 0]),
    ("limegreen", [50, 205, 50]),
    ("linen", [250, 240, 230]),
    ("magenta", [255, 0, 255]),
    ("maroon", [128, 0, 0]),
    ("mediumaquamarine", [102, 205, 170]),
    ("mediumblue", [0, 0, 205]),
    ("mediumorchid", [186, 85, 211]),
    ("mediumpurple", [147, 112, 219]),
    ("mediumseagreen", [60, 179, 113]),
    ("mediumslateblue", [123, 104, 238]),
    ("mediumspringgreen", [0, 250, 154]),
    ("mediumturquoise", [72, 209, 204]),
    ("mediumvioletred", [199, 21, 133]),
    ("midnightblue", [25, 25, 112]),
    ("mintcream", [245, 255, 250]),
    ("mistyrose", [255, 228, 225]),
    ("moccasin", [255, 228, 181]),
    ("navajowhite", [255, 222, 173]),
    ("navy", [0, 0, 128]),
    ("oldlace", [253, 245, 230]),
    ("olive", [128, 128, 0]),
    ("olivedrab", [107, 142, 35]),
    ("orange", [255, 165, 0]),
    ("orangered", [255, 69, 0]),
    ("orchid", [218, 112, 214]),
    ("palegoldenrod", [238, 232, 170]),
    ("palegreen", [152, 251, 152]),
    ("paleturquoise", [175, 238, 238]),
    ("palevioletred", [219, 112, 147]),
    ("papayawhip", [255, 239, 213]),
    ("peachpuff", [255, 218, 185]),
    ("peru", [205, 133, 63]),
    ("pink", [255, 192, 203]),
    ("plum", [221, 160, 221]),
    ("powderblue", [176, 224, 230]),
    ("purple", [128, 0, 128]),
    ("rebeccapurple", [102, 51, 153]),
    ("red", [255, 0, 0]),
    ("rosybrown", [188, 143, 143]),
    ("royalblue", [65, 105, 225]),
    ("saddlebrown", [139, 69, 19]),
    ("salmon", [250, 128, 114]),
    ("sandybrown", [244, 164, 96]),
    ("seagreen", [46, 139, 87]),
    ("seashell", [255, 245, 238]),
    ("sienna", [160, 82, 45]),
    ("silver", [192, 192, 192]),
    ("skyblue", [135, 206, 235]),
    ("slateblue", [106, 90, 205]),
    ("slategray", [112, 128, 144]),
    ("slategrey", [112, 128, 144]),
    ("snow", [255, 250, 250]),
    ("springgreen", [0, 255, 127]),
    ("steelblue", [70, 130, 180]),
    ("tan", [210, 180, 140]),
    ("teal", [0, 128, 128]),
    ("thistle", [216, 191, 216]),
    ("tomato", [255, 99, 71]),
    ("turquoise", [64, 224, 208]),
    ("violet", [238, 130, 238]),
    ("wheat", [245, 222, 179]),
    ("white", [255, 255, 255]),
    ("whitesmoke", [245, 245, 245]),
    ("yellow", [255, 255, 0]),
    ("yellowgreen", [154, 205, 50]),
];
