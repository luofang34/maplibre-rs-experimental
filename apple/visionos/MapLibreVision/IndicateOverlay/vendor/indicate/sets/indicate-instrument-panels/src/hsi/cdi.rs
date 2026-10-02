//! The course deviation indicator: course arrow, deviation bar, scale
//! dots, and TO/FROM triangle.

use indicate_instrument_scene::{Anchor, PaintMode, Rgba8, SceneError, SceneWriter};
use indicate_instrument_state::{GroupId, NavFromTo, NavResolved, NavSource};

use indicate_instrument_symbology::palette;

/// Two dots of lateral deviation = 75 px (37.5 px/dot, G5 proportions).
const PX_PER_DOT: f32 = 37.5;

pub(crate) fn source_color(source: NavSource) -> Rgba8 {
    match source {
        NavSource::Gps => palette::MAGENTA,
        _ => palette::GREEN,
    }
}

/// Which receiver drives the needle: `GPS` / `NAV1` / `NAV2`. Nav1 and
/// Nav2 wear the same green, so the text is the only thing that tells
/// them apart. The numerals make this a claimed run — it derives from
/// the nav group, so it claims `GroupId::Nav`, and the claim is honest
/// exactly where the CDI gate is: a withheld or failed nav group never
/// reaches this draw.
///
/// Distinct from [`indicate_instrument_symbology::source_label`], which
/// names the SENSOR feeding a function and colors it by health. This
/// names the receiver a selection points at, and colors it by source
/// class. One panel paints both, so they never share a name.
pub(crate) fn receiver_text(source: NavSource) -> Option<&'static str> {
    match source {
        NavSource::Gps => Some("GPS"),
        NavSource::Nav1 => Some("NAV1"),
        NavSource::Nav2 => Some("NAV2"),
        // No source is gated out by the caller; an unknown one fails the
        // nav group in resolve. Neither may invent a label here.
        NavSource::None | NavSource::Unknown => None,
    }
}

/// The label position, lower left beside the rose: clear of the rose's
/// outermost ink and directly above the course box, whose value wears
/// the same source color. The clearance is a test, not a comment, so
/// that growing the rose fails rather than overlapping the label.
pub(crate) const RECEIVER_LABEL_POS: (f32, f32) = (60.0, super::CY + 110.0);

/// The run size the label paints at. Named so the clearance test
/// measures the box the label really occupies.
pub(crate) const RECEIVER_LABEL_SIZE: f32 = 14.0;

/// Draws the receiver label in the source color, claimed from the nav
/// group. Draws nothing for `None`/`Unknown` — the caller's gate already
/// excludes both, and this arm keeps that property local.
pub fn draw_receiver_label(
    scene: &mut SceneWriter<'_>,
    source: NavSource,
) -> Result<(), SceneError> {
    let Some(text) = receiver_text(source) else {
        return Ok(());
    };
    scene.fill_color(source_color(source))?;
    scene.text_attributed(
        GroupId::Nav.to_u8(),
        RECEIVER_LABEL_POS.0,
        RECEIVER_LABEL_POS.1,
        RECEIVER_LABEL_SIZE,
        Anchor::CENTER,
        text,
    )?;
    Ok(())
}

/// Where the scale label sits: under the receiver label, so the two
/// facts about the needle — which receiver drives it, and what a dot is
/// worth — read together.
pub(crate) const SCALE_LABEL_POS: (f32, f32) = (60.0, super::CY + 128.0);
/// The run size the scale label paints at. Named so the clearance
/// tests measure the box the label really occupies.
pub(crate) const SCALE_LABEL_SIZE: f32 = 14.0;

/// Draws the scale the deflection is on, in the source color, under the
/// same gate as the needle.
///
/// Two dots is two dots on the glass whatever the scale, so the label is
/// what stops one needle position from meaning different distances in
/// different phases. An unknown scale never reaches here: the nav group
/// fails first, and the needle goes with it.
pub fn draw_scale_label(scene: &mut SceneWriter<'_>, nav: &NavResolved) -> Result<(), SceneError> {
    let label = nav.data.scale.label();
    if label.is_empty() {
        return Ok(());
    }
    scene.fill_color(source_color(nav.data.source))?;
    scene.text_attributed(
        GroupId::Nav.to_u8(),
        SCALE_LABEL_POS.0,
        SCALE_LABEL_POS.1,
        SCALE_LABEL_SIZE,
        Anchor::CENTER,
        label,
    )?;
    Ok(())
}

/// Draws the CDI in the rose frame, rotated to the selected course.
pub fn draw_cdi(
    scene: &mut SceneWriter<'_>,
    nav: &NavResolved,
    heading_rad: f32,
) -> Result<(), SceneError> {
    let color = source_color(nav.data.source);
    scene.save()?;
    scene.translate(super::CX, super::CY)?;
    scene.rotate(nav.course_rose_rad.value - heading_rad)?;

    // Course arrow: head, fore shaft, aft shaft.
    scene.fill_color(color)?;
    scene.stroke(color, 4.0)?;
    scene.polygon(
        PaintMode::Fill,
        &[[0.0, -90.0], [-10.0, -70.0], [10.0, -70.0]],
    )?;
    scene.line(0.0, -70.0, 0.0, -38.0)?;
    scene.line(0.0, 38.0, 0.0, 90.0)?;

    // Scale dots on the perpendicular.
    scene.stroke(palette::GREY, 2.0)?;
    for dx in [-2.0f32, -1.0, 1.0, 2.0] {
        scene.circle(PaintMode::Stroke, dx * PX_PER_DOT, 0.0, 4.0)?;
    }

    // Deviation bar: where the course line *is*, relative to the aircraft.
    let dx = (nav.data.cdi_dots.clamp(-2.4, 2.4)) * PX_PER_DOT;
    scene.fill_color(color)?;
    scene.rect(PaintMode::Fill, dx - 2.5, -36.0, 5.0, 72.0)?;

    // TO/FROM triangle beside the fore shaft.
    match nav.data.fromto {
        NavFromTo::To => {
            scene.polygon(
                PaintMode::Fill,
                &[[0.0, -34.0], [-8.0, -18.0], [8.0, -18.0]],
            )?;
        }
        NavFromTo::From => {
            scene.polygon(PaintMode::Fill, &[[0.0, 34.0], [-8.0, 18.0], [8.0, 18.0]])?;
        }
        // Unknown resolution never reaches here (the nav group fails
        // before drawing); the arm is the exhaustiveness fail-safe and
        // draws no flag rather than inventing one.
        NavFromTo::Off | NavFromTo::Unknown => {}
    }
    scene.restore()?;
    Ok(())
}
