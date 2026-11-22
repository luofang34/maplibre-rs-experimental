#![allow(clippy::expect_used, clippy::panic)]
use super::metadata::write_glyph_poses;
use crate::{
    coords::ZoomLevel,
    render::shaders::{SDFShaderFeatureMetadata, ShaderSymbolVertex, POSED_GLYPH},
    sdf::{line_glyphs::GlyphPose, Feature, LineLabel},
    vector::tessellation::OverAlignedVertexBuffer,
};

fn vertex(anchor: [i32; 2]) -> ShaderSymbolVertex {
    ShaderSymbolVertex::new(anchor, [0, 0], [0, 0], 0, 0.0, 0.0)
}

#[test]
fn each_glyph_writes_its_own_pose_to_its_six_indices() {
    // Two glyph quads after one icon quad: vertices 4..8 and 8..12.
    let mut buffer = OverAlignedVertexBuffer::empty();
    buffer.buffer.vertices = (0..12).map(|_| vertex([100, 200])).collect();
    for quad in 0..3_u32 {
        let base = quad * 4;
        buffer
            .buffer
            .indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let layer = crate::sdf::SymbolLayerData {
        atlas: None,
        coords: crate::coords::WorldTileCoords {
            x: 0,
            y: 0,
            z: ZoomLevel::from(0),
        },
        source_layer: "roads".into(),
        style_layer_id: "label".into(),
        buffer,
        features: Vec::<Feature>::new(),
    };
    let line = LineLabel {
        polyline: [[0.0, 0.0], [500.0, 0.0]].into(),
        anchor_distance: 250.0,
        glyph_offsets: vec![-10.0, 10.0],
        image_sizes: vec![[0.0; 2]; 2],
        first_glyph_index: 6,
        text_offset_y: 0.0,
    };
    let poses = [
        GlyphPose {
            point: [90.0, 200.0],
            angle: 0.5,
            rise: 0.0,
        },
        GlyphPose {
            point: [110.0, 205.0],
            angle: 0.75,
            rise: 12.5,
        },
    ];
    let mut metadata = vec![SDFShaderFeatureMetadata::default(); 3];
    write_glyph_poses(&layer, &line, &poses, &mut metadata);
    let [m0, m1, m2] = metadata.as_slice() else {
        panic!("expected 3 metadata slots, got {}", metadata.len());
    };
    assert!(!m0.has_pose(), "the icon has no pose");
    let p = m1.decoded_pose();
    assert!([p[0], p[1], p[3]] == [-10.0, 0.0, POSED_GLYPH] && (p[2] - 0.5).abs() < 1e-4);
    let p = m2.decoded_pose();
    assert!([p[0], p[1], p[3]] == [10.0, 5.0, POSED_GLYPH + 12.5] && (p[2] - 0.75).abs() < 1e-4);
}

#[test]
fn a_changed_pose_is_detected_as_dirty_by_metadata_diff() {
    let before = vec![SDFShaderFeatureMetadata::default(); 4];
    let mut after = before.clone();
    if let Some(slot) = after.get_mut(2) {
        slot.set_glyph_pose(1.0, 0.0, 0.0, 0.0);
    }
    assert_ne!(
        before.get(2).map(bytemuck::bytes_of),
        after.get(2).map(bytemuck::bytes_of)
    );
}
