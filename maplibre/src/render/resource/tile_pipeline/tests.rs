use super::{counting_overdraw, OVERDRAW_FRAGMENT};
use crate::render::resource::FragmentState;

fn stage(write_mask: wgpu::ColorWrites) -> FragmentState {
    FragmentState {
        source: "shading",
        entry_point: "shade",
        targets: vec![Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8Unorm,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask,
        })],
    }
}

#[test]
fn a_colour_stage_adds_one_step_per_draw() {
    let counted = counting_overdraw(stage(wgpu::ColorWrites::ALL));
    assert_eq!(counted.source, OVERDRAW_FRAGMENT);
    assert_eq!(counted.entry_point, "main");
    let target = counted.targets[0].as_ref().expect("a colour target");
    let adding = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    assert_eq!(
        target.blend,
        Some(wgpu::BlendState {
            color: adding,
            alpha: adding,
        })
    );
    assert_eq!(target.write_mask, wgpu::ColorWrites::ALL);
}

#[test]
fn a_stage_that_writes_no_colour_is_left_alone() {
    let mask = stage(wgpu::ColorWrites::empty());
    assert_eq!(counting_overdraw(mask.clone()), mask);
}
