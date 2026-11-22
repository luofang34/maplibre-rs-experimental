//! Low-level byte cursor primitives for stationary GeoJSON scanning.

pub(super) struct Scanner<'a> {
    pub(super) bytes: &'a [u8],
    pub(super) pos: usize,
}

impl<'a> Scanner<'a> {
    #[inline]
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    #[inline]
    pub(super) fn skip_ws(&mut self) {
        while let Some(&b) = self.bytes.get(self.pos) {
            if matches!(b, b' ' | b'\n' | b'\r' | b'\t') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    #[inline]
    pub(super) fn consume(&mut self, expected: u8) -> bool {
        self.skip_ws();
        if self.bytes.get(self.pos) == Some(&expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Scans an unescaped JSON string literal and returns its inner UTF-8 slice (`\` -> `None`).
    #[inline]
    pub(super) fn next_simple_str(&mut self) -> Option<&'a str> {
        self.skip_ws();
        if self.bytes.get(self.pos) != Some(&b'"') {
            return None;
        }
        self.pos += 1;
        let start = self.pos;
        while let Some(&b) = self.bytes.get(self.pos) {
            if b == b'"' {
                let slice = self.bytes.get(start..self.pos)?;
                self.pos += 1;
                return std::str::from_utf8(slice).ok();
            }
            if b == b'\\' || b < 0x20 {
                return None;
            }
            self.pos += 1;
        }
        None
    }

    pub(super) fn scan_number_token(&mut self) -> Option<&'a [u8]> {
        self.skip_ws();
        let start = self.pos;
        while let Some(&b) = self.bytes.get(self.pos) {
            if matches!(b, b'-' | b'+' | b'0'..=b'9' | b'.' | b'e' | b'E') {
                self.pos += 1;
            } else {
                break;
            }
        }
        (self.pos > start)
            .then(|| self.bytes.get(start..self.pos))
            .flatten()
    }

    #[inline]
    pub(super) fn next_f64_exact(&mut self) -> Option<f64> {
        let tok = self.scan_number_token()?;
        let s = std::str::from_utf8(tok).ok()?;
        let v: f64 = s.parse().ok()?;
        v.is_finite().then_some(v)
    }

    #[inline]
    pub(super) fn next_f64_fast(&mut self) -> Option<f64> {
        let tok = self.scan_number_token()?;
        parse_fast_f64(tok)
    }

    pub(super) fn skip_value(&mut self) -> bool {
        self.skip_ws();
        let Some(&first) = self.bytes.get(self.pos) else {
            return false;
        };
        match first {
            b'"' => self.skip_string(),
            b'{' | b'[' => {
                let mut depth = 0_u32;
                while let Some(&b) = self.bytes.get(self.pos) {
                    match b {
                        b'"' => {
                            if !self.skip_string() {
                                return false;
                            }
                        }
                        b'{' | b'[' => {
                            depth += 1;
                            self.pos += 1;
                        }
                        b'}' | b']' => {
                            depth = depth.saturating_sub(1);
                            self.pos += 1;
                            if depth == 0 {
                                return true;
                            }
                        }
                        _ => self.pos += 1,
                    }
                }
                false
            }
            b't' => self.consume_lit(b"true"),
            b'f' => self.consume_lit(b"false"),
            b'n' => self.consume_lit(b"null"),
            b'-' | b'0'..=b'9' => self.next_f64_exact().is_some(),
            _ => false,
        }
    }

    fn skip_string(&mut self) -> bool {
        if self.bytes.get(self.pos) != Some(&b'"') {
            return false;
        }
        self.pos += 1;
        while let Some(&b) = self.bytes.get(self.pos) {
            self.pos += 1;
            if b == b'"' {
                return true;
            }
            if b == b'\\' {
                if self.pos >= self.bytes.len() {
                    return false;
                }
                self.pos += 1;
            }
        }
        false
    }

    #[inline]
    pub(super) fn consume_lit(&mut self, lit: &[u8]) -> bool {
        self.skip_ws();
        let end = self.pos + lit.len();
        if self.bytes.get(self.pos..end) == Some(lit) {
            self.pos = end;
            true
        } else {
            false
        }
    }
}

#[inline]
pub(super) fn parse_u64_digits(tok: &[u8]) -> Option<u64> {
    if tok.is_empty() || tok.len() > 19 {
        return None;
    }
    let mut acc = 0_u64;
    for &b in tok {
        let d = b.checked_sub(b'0')?;
        if d > 9 {
            return None;
        }
        acc = acc * 10 + u64::from(d);
    }
    Some(acc)
}

const POW10: [f64; 16] = [
    1.0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15,
];

#[inline]
fn parse_fast_f64(tok: &[u8]) -> Option<f64> {
    let (&first, _) = tok.split_first()?;
    let (neg, mut i) = if first == b'-' {
        (true, 1)
    } else if first == b'+' {
        (false, 1)
    } else {
        (false, 0)
    };
    if i >= tok.len() {
        return None;
    }
    let mut mant = 0_u64;
    let mut digits = 0_usize;
    let mut frac_digits = 0_usize;
    let mut seen_dot = false;
    while let Some(&b) = tok.get(i) {
        if b.is_ascii_digit() {
            digits += 1;
            if digits > 15 {
                let s = std::str::from_utf8(tok).ok()?;
                let v: f64 = s.parse().ok()?;
                return v.is_finite().then_some(v);
            }
            mant = mant * 10 + u64::from(b - b'0');
            if seen_dot {
                frac_digits += 1;
            }
            i += 1;
        } else if b == b'.' && !seen_dot {
            seen_dot = true;
            i += 1;
        } else {
            let s = std::str::from_utf8(tok).ok()?;
            let v: f64 = s.parse().ok()?;
            return v.is_finite().then_some(v);
        }
    }
    if digits == 0 {
        return None;
    }
    let scale = *POW10.get(frac_digits)?;
    let val = (mant as f64) / scale;
    Some(if neg { -val } else { val })
}
