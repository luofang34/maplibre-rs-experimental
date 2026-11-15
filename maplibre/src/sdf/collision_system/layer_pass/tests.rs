#![allow(clippy::expect_used, clippy::panic)]
use super::metadata::write_glyph_poses;
use crate::{
    coords::ZoomLevel,
    render::shaders::{SDFShaderFeatureMetadata, ShaderSymbolVertex, POSED_GLYPH},
    sdf::{line_glyphs::GlyphPose, Feature, LineLabel},
    vector::tessellation::OverAlignedVertexBuffer,
};

fn vertex(anchor: [i32; 2]) -> ShaderSymbolVertex {
    ShaderSymbolVertex {
        a_pos_offset: [anchor[0], anchor[1], 0, 0],
        a_data: [0; 4],
        a_pixeloffset: [0; 4],
    }
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
    let mut metadata = vec![SDFShaderFeatureMetadata::default(); 12];
    write_glyph_poses(&layer, &line, &poses, &mut metadata);
    assert!(
        metadata[..4].iter().all(|entry| entry.pose[3] == 0.0),
        "the icon has no pose"
    );
    assert!(metadata[4..8]
        .iter()
        .all(|entry| entry.pose == [-10.0, 0.0, 0.5, POSED_GLYPH]));
    assert!(metadata[8..]
        .iter()
        .all(|entry| entry.pose == [10.0, 5.0, 0.75, POSED_GLYPH + 12.5]));
}

#[test]
fn a_changed_pose_is_detected_as_dirty_by_metadata_diff() {
    let before = vec![SDFShaderFeatureMetadata::default(); 4];
    let mut after = before.clone();
    after[2].pose = [1.0, 0.0, 0.0, 1.0];
    assert_ne!(
        bytemuck::bytes_of(&before[2]),
        bytemuck::bytes_of(&after[2])
    );
}
