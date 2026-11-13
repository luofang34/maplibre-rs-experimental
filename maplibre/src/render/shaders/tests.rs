//! Every production shader translates to MSL and SPIR-V, so a native render node can take the
//! same WGSL through naga instead of a second copy of the shader.

use super::{
    fill_extrusion::ExtrusionPass, texture::DemShading, AtmosphereShader, BackgroundPatternShader,
    BackgroundShader, CircleShader, FillExtrusionShader, FillPatternShader, FillShader,
    FlatTileMaskShader, GlobeBackgroundPatternShader, GlobeBackgroundShader,
    HeatmapCompositeShader, HeatmapDensityShader, LineShader, RasterShader, Shader,
    ShaderTileMetadata, SkyShader, SymbolShader, TerrainLineShader, TerrainShader, TileMaskShader,
};

/// A stage's WGSL and its entry point.
type Stage = (&'static str, String, &'static str);

fn stages() -> Vec<Stage> {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let shaders: Vec<(&'static str, Box<dyn Shader>)> = vec![
        ("terrain", Box::new(TerrainShader { format })),
        ("background", Box::new(BackgroundShader { format })),
        (
            "background pattern",
            Box::new(BackgroundPatternShader { format }),
        ),
        (
            "globe background",
            Box::new(GlobeBackgroundShader { format }),
        ),
        (
            "globe background pattern",
            Box::new(GlobeBackgroundPatternShader { format }),
        ),
        ("sky", Box::new(SkyShader { format })),
        ("atmosphere", Box::new(AtmosphereShader { format })),
        #[cfg(feature = "experimental")]
        (
            "globe daylight",
            Box::new(crate::background::daylight::GlobeDaylightShader { format }),
        ),
        ("heatmap density", Box::new(HeatmapDensityShader)),
        (
            "heatmap composite",
            Box::new(HeatmapCompositeShader { format }),
        ),
        ("fill", Box::new(FillShader { format })),
        ("fill pattern", Box::new(FillPatternShader { format })),
        ("symbol", Box::new(SymbolShader { format })),
        ("tile mask flat", Box::new(FlatTileMaskShader { format })),
        (
            "tile mask",
            Box::new(TileMaskShader {
                format,
                draw_colors: false,
                debug_lines: false,
            }),
        ),
        (
            "tile debug",
            Box::new(TileMaskShader {
                format,
                draw_colors: true,
                debug_lines: true,
            }),
        ),
        ("raster", Box::new(RasterShader { format })),
        (
            "hillshade",
            Box::new(super::DemShader {
                format,
                shading: DemShading::Hillshade,
            }),
        ),
        (
            "colour relief",
            Box::new(super::DemShader {
                format,
                shading: DemShading::ColorRelief,
            }),
        ),
        ("line", Box::new(LineShader { format })),
        ("terrain line", Box::new(TerrainLineShader { format })),
        ("circle", Box::new(CircleShader { format })),
    ];
    let extrusions = [
        ExtrusionPass::Depth,
        ExtrusionPass::Color,
        ExtrusionPass::PatternColor,
    ]
    .map(|pass| -> (&'static str, Box<dyn Shader>) {
        (
            "fill extrusion",
            Box::new(FillExtrusionShader { format, pass }),
        )
    });
    let mut stages = Vec::new();
    for (name, shader) in shaders.into_iter().chain(extrusions) {
        let vertex = shader.describe_vertex();
        stages.push((name, vertex.source.to_owned(), vertex.entry_point));
        let fragment = shader.describe_fragment();
        stages.push((name, fragment.source.to_owned(), fragment.entry_point));
    }
    // Passes that build their pipelines without the trait.
    for (name, source) in [
        ("depth copy", include_str!("depth_copy.vertex.wgsl")),
        ("depth copy", include_str!("depth_copy.fragment.wgsl")),
        (
            "depth copy",
            include_str!("depth_copy_multisampled.fragment.wgsl"),
        ),
        ("mipmap", include_str!("mipmap.vertex.wgsl")),
        ("mipmap", include_str!("mipmap.fragment.wgsl")),
        ("overlay", include_str!("overlay.wgsl")),
        ("depth probe", include_str!("depth_probe.wgsl")),
        ("depth probe", include_str!("depth_probe_multisampled.wgsl")),
    ] {
        stages.push((name, source.to_owned(), ""));
    }
    #[cfg(not(feature = "experimental"))]
    for (name, source) in [
        (
            "globe daylight vertex",
            include_str!("globe_daylight.vertex.wgsl"),
        ),
        (
            "globe daylight fragment",
            concat!(
                include_str!("daylight.wgsl.part"),
                include_str!("globe_daylight.fragment.wgsl")
            ),
        ),
    ] {
        stages.push((name, source.to_owned(), ""));
    }
    stages
}

fn validated(name: &str, source: &str) -> (naga::Module, naga::valid::ModuleInfo) {
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(source)));
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("{name}: {error:?}"));
    (module, info)
}

#[test]
fn every_shader_translates_to_msl_and_spirv() {
    for (name, source, entry) in stages() {
        let (module, info) = validated(name, &source);
        let entries: Vec<_> = module
            .entry_points
            .iter()
            .filter(|point| entry.is_empty() || point.name == entry)
            .collect();
        assert!(!entries.is_empty(), "{name}: no entry point {entry}");
        naga::back::msl::write_string(
            &module,
            &info,
            // Invariant positions need MSL 2.1; every Apple GPU a native node targets has more.
            &naga::back::msl::Options {
                lang_version: (2, 4),
                ..naga::back::msl::Options::default()
            },
            &naga::back::msl::PipelineOptions::default(),
        )
        .unwrap_or_else(|error| panic!("{name}: MSL {error:?}"));
        naga::back::spv::write_vec(&module, &info, &naga::back::spv::Options::default(), None)
            .unwrap_or_else(|error| panic!("{name}: SPIR-V {error:?}"));
    }
}

#[test]
fn every_shader_file_is_part_of_a_pipeline() {
    let stages = stages();
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/render/shaders");
    for entry in std::fs::read_dir(&directory).expect("shader directory") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("wgsl") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("shader");
        assert!(
            stages
                .iter()
                .any(|(_, source, _)| source.contains(text.trim())),
            "{} is not part of any pipeline this test translates",
            path.display()
        );
    }
}

#[test]
fn shader_tile_metadata_and_globe_mesh_vertex_satisfy_mobile_vulkan_alignments() {
    use std::mem::{offset_of, size_of};

    use crate::render::tile_mesh::GpuTileMeshVertex;

    assert_eq!(size_of::<ShaderTileMetadata>(), 128);
    assert_eq!(size_of::<ShaderTileMetadata>() % 16, 0);

    // Every Vec4f32 / mat4x4 column in ShaderTileMetadata sits on a 16-byte boundary.
    assert_eq!(offset_of!(ShaderTileMetadata, transform) % 16, 0);
    assert_eq!(offset_of!(ShaderTileMetadata, tile_mercator_coords) % 16, 0);
    assert_eq!(offset_of!(ShaderTileMetadata, clip_region) % 16, 0);

    // Paired Float32x2 scalar attributes sit contiguously on 8-byte boundaries.
    assert_eq!(offset_of!(ShaderTileMetadata, viewport_width) % 8, 0);
    assert_eq!(
        offset_of!(ShaderTileMetadata, viewport_height),
        offset_of!(ShaderTileMetadata, viewport_width) + 4
    );
    assert_eq!(offset_of!(ShaderTileMetadata, line_width_scale) % 8, 0);
    assert_eq!(
        offset_of!(ShaderTileMetadata, line_units_per_pixel),
        offset_of!(ShaderTileMetadata, line_width_scale) + 4
    );
    assert_eq!(offset_of!(ShaderTileMetadata, zoom_factor) % 4, 0);
    assert_eq!(offset_of!(ShaderTileMetadata, clip_antimeridian) % 4, 0);

    // GlobeTileMesh vertices use 8-byte Float32x2 positions (32-bit aligned).
    assert_eq!(size_of::<GpuTileMeshVertex>(), 8);
    let mesh_layout = GpuTileMeshVertex::vertex_buffer_layout();
    assert_eq!(mesh_layout.array_stride, 8);
    assert_eq!(
        mesh_layout.attributes[0].format,
        wgpu::VertexFormat::Float32x2
    );
    assert_eq!(mesh_layout.attributes[0].offset, 0);
}

#[path = "tests/pure_globe.rs"]
mod pure_globe;
