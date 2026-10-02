//! Design-space geometry for the admission checks: the same affine
//! semantics the reference rasterizer applies (post-multiplied
//! translate/rotate, y-down clockwise rotation), with rotated shapes
//! reduced to conservative axis-aligned bounds.
//!
//! The graphics state here mirrors the rasterizer's own: `Save` pushes
//! transform, clip, and paint together and `Restore` pops all three, so
//! a check that walks a scene sees what a backend would.

use indicate_instrument_scene::{Cmd, HAlign, VAlign, nominal_text_ink_width, nominal_text_width};

#[cfg(test)]
mod tests;

/// An axis-aligned rectangle in design-frame units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Rect {
    pub(super) min_x: f32,
    pub(super) min_y: f32,
    pub(super) max_x: f32,
    pub(super) max_y: f32,
}

impl Rect {
    /// A rectangle covering nothing, so unioning it in changes nothing.
    pub(super) const EMPTY: Rect = Rect {
        min_x: f32::INFINITY,
        min_y: f32::INFINITY,
        max_x: f32::NEG_INFINITY,
        max_y: f32::NEG_INFINITY,
    };

    pub(super) fn is_empty(&self) -> bool {
        !(self.max_x > self.min_x && self.max_y > self.min_y)
    }

    /// The smallest rectangle containing both.
    pub(super) fn union(&self, other: &Rect) -> Rect {
        Rect {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }

    /// Grows every edge outward by `margin` — a stroked shape inks half
    /// its line width beyond the geometry it is drawn from.
    pub(super) fn inflate(&self, margin: f32) -> Rect {
        Rect {
            min_x: self.min_x - margin,
            min_y: self.min_y - margin,
            max_x: self.max_x + margin,
            max_y: self.max_y + margin,
        }
    }

    pub(super) fn intersects(&self, other: &Rect) -> bool {
        self.min_x < other.max_x
            && other.min_x < self.max_x
            && self.min_y < other.max_y
            && other.min_y < self.max_y
    }

    pub(super) fn contains(&self, other: &Rect) -> bool {
        other.min_x >= self.min_x
            && other.max_x <= self.max_x
            && other.min_y >= self.min_y
            && other.max_y <= self.max_y
    }

    pub(super) fn intersect(&self, other: &Rect) -> Rect {
        Rect {
            min_x: self.min_x.max(other.min_x),
            min_y: self.min_y.max(other.min_y),
            max_x: self.max_x.min(other.max_x),
            max_y: self.max_y.min(other.max_y),
        }
    }
}

/// Row-major affine transform, matching the rasterizer's `Affine`:
/// `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
#[derive(Debug, Clone, Copy)]
pub(super) struct Ctm {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Ctm {
    pub(super) const IDENTITY: Ctm = Ctm {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    /// Post-multiplies a translation, as the IR's `Translate` does.
    pub(super) fn translate(&mut self, tx: f32, ty: f32) {
        self.e += self.a * tx + self.c * ty;
        self.f += self.b * tx + self.d * ty;
    }

    /// Post-multiplies a rotation; positive is clockwise in the y-down
    /// logical space, matching the IR.
    pub(super) fn rotate(&mut self, radians: f32) {
        let (s, k) = (radians.sin(), radians.cos());
        let (a, b, c, d) = (self.a, self.b, self.c, self.d);
        self.a = a * k + c * s;
        self.b = b * k + d * s;
        self.c = -a * s + c * k;
        self.d = -b * s + d * k;
    }

    /// Whether the transform is a pure translation/scale (no rotation
    /// or shear): the case where a mapped rectangle's bounds ARE the
    /// rectangle, so bbox containment is exact rather than merely
    /// conservative.
    pub(super) fn is_axis_aligned(&self) -> bool {
        self.b == 0.0 && self.c == 0.0
    }

    fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// The axis-aligned bounds of `rect`'s four mapped corners —
    /// conservative under rotation.
    pub(super) fn map_rect(&self, rect: &Rect) -> Rect {
        let corners = [
            self.apply(rect.min_x, rect.min_y),
            self.apply(rect.max_x, rect.min_y),
            self.apply(rect.min_x, rect.max_y),
            self.apply(rect.max_x, rect.max_y),
        ];
        let mut out = Rect {
            min_x: f32::INFINITY,
            min_y: f32::INFINITY,
            max_x: f32::NEG_INFINITY,
            max_y: f32::NEG_INFINITY,
        };
        for (x, y) in corners {
            out.min_x = out.min_x.min(x);
            out.min_y = out.min_y.min(y);
            out.max_x = out.max_x.max(x);
            out.max_y = out.max_y.max(y);
        }
        out
    }
}

/// Graphics state as the real state machine carries it: Save pushes
/// transform, clip, and paint state together; Restore pops all three.
#[derive(Clone, Copy)]
pub(super) struct Gs {
    pub(super) ctm: Ctm,
    pub(super) clip: Option<Rect>,
    pub(super) fill_alpha: u8,
    pub(super) stroke_width: f32,
}

impl Gs {
    /// The state a scene starts in, matching the rasterizer's own
    /// defaults.
    pub(super) const DEFAULT: Self = Self {
        ctm: Ctm::IDENTITY,
        clip: None,
        fill_alpha: 255,
        stroke_width: 1.0,
    };

    /// How far a stroked shape inks beyond its geometry.
    pub(super) fn stroke_margin(&self) -> f32 {
        (self.stroke_width / 2.0).max(0.0)
    }
}

/// Applies one command to the graphics-state stack, exactly as the real
/// state machine would. Drawing commands leave it untouched.
pub(super) fn track_state(stack: &mut Vec<Gs>, cmd: &Cmd<'_>) {
    match *cmd {
        Cmd::Save => {
            if let Some(top) = stack.last().copied() {
                stack.push(top);
            }
        }
        Cmd::Restore => {
            stack.pop();
            if stack.is_empty() {
                stack.push(Gs::DEFAULT);
            }
        }
        Cmd::Translate { x, y } => {
            if let Some(gs) = stack.last_mut() {
                gs.ctm.translate(x, y);
            }
        }
        Cmd::Rotate { radians } => {
            if let Some(gs) = stack.last_mut() {
                gs.ctm.rotate(radians);
            }
        }
        Cmd::FillColor { color } => {
            if let Some(gs) = stack.last_mut() {
                gs.fill_alpha = color.a;
            }
        }
        Cmd::Stroke { width, .. } => {
            if let Some(gs) = stack.last_mut() {
                gs.stroke_width = width;
            }
        }
        Cmd::ClipRect { x, y, w, h } => {
            if let Some(gs) = stack.last_mut() {
                let mapped = gs.ctm.map_rect(&Rect {
                    min_x: x,
                    min_y: y,
                    max_x: x + w,
                    max_y: y + h,
                });
                gs.clip = Some(match gs.clip {
                    None => mapped,
                    Some(previous) => previous.intersect(&mapped),
                });
            }
        }
        _ => {}
    }
}

/// The local-space ink rectangle of a text run: nominal ink width from
/// the scene text-metrics contract, cap height approximated by the run
/// size, positioned by the anchor.
pub(super) fn text_rect(x: f32, y: f32, size: f32, h: HAlign, v: VAlign, chars: usize) -> Rect {
    let advance = nominal_text_width(size, chars);
    let ink = nominal_text_ink_width(size, chars);
    let min_x = match h {
        HAlign::Left => x,
        HAlign::Center => x - advance / 2.0,
        HAlign::Right => x - advance,
    };
    let min_y = match v {
        VAlign::Baseline | VAlign::Bottom => y - size,
        VAlign::Middle => y - size / 2.0,
        VAlign::Top => y,
    };
    Rect {
        min_x,
        min_y,
        max_x: min_x + ink,
        max_y: min_y + size,
    }
}
