//! Every production shader translates to MSL and SPIR-V, so a native render node can take the
//! same WGSL through naga instead of a second copy of the shader.

use super::{
    fill_extrusion::ExtrusionPass, texture::DemShading, AtmosphereShader, BackgroundPatternShader,
    BackgroundShader, CircleDynamicRadiusShader, CircleFeatureStrokeShader, CircleShader,
    FillExtrusionShader, FillPatternShader, FillShader, FlatTileMaskShader,
    GlobeBackgroundPatternShader, GlobeBackgroundShader, HeatmapCompositeShader,
    HeatmapDensityShader, LineShader, PlainLineShader, RasterShader, Shader, ShaderTileMetadata,
    SkyShader, StencilClearShader, SymbolShader, TerrainLineShader, TerrainShader, TileMaskShader,
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
        (
            "plain line flat",
            Box::new(PlainLineShader {
                format,
                globe: false,
            }),
        ),
        (
            "plain line globe",
            Box::new(PlainLineShader {
                format,
                globe: true,
            }),
        ),
        ("terrain line", Box::new(TerrainLineShader { format })),
        ("stencil clear", Box::new(StencilClearShader { format })),
        ("circle", Box::new(CircleShader { format })),
        (
            "circle feature stroke",
            Box::new(CircleFeatureStrokeShader { format }),
        ),
        (
            "circle dynamic radius",
            Box::new(CircleDynamicRadiusShader { format }),
        ),
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
    let fill_shader = FillShader { format };
    let flat_fill_v = fill_shader.describe_vertex_for_globe(false);
    stages.push((
        "fill flat",
        flat_fill_v.source.to_owned(),
        flat_fill_v.entry_point,
    ));
    let opaque_fill_f = fill_shader.describe_fragment_opaque();
    stages.push((
        "fill opaque",
        opaque_fill_f.source.to_owned(),
        opaque_fill_f.entry_point,
    ));
    let symbol_shader = SymbolShader { format };
    let flat_symbol_v = symbol_shader.describe_vertex_for_globe(false);
    stages.push((
        "symbol flat",
        flat_symbol_v.source.to_owned(),
        flat_symbol_v.entry_point,
    ));
    let symbol_f = symbol_shader.describe_fragment();
    stages.push((
        "symbol flat",
        symbol_f.source.to_owned(),
        symbol_f.entry_point,
    ));
    // Passes that build their pipelines without the trait.
    for (name, source) in [
        (
            "depth copy",
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/engine/render/gpu/shaders/depth_copy.vertex.wgsl"
            )),
        ),
        (
            "depth copy",
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/engine/render/gpu/shaders/depth_copy.fragment.wgsl"
            )),
        ),
        (
            "depth copy",
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/engine/render/gpu/shaders/depth_copy_multisampled.fragment.wgsl"
            )),
        ),
        (
            "mipmap",
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/engine/render/gpu/shaders/mipmap.vertex.wgsl"
            )),
        ),
        (
            "mipmap",
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/engine/render/gpu/shaders/mipmap.fragment.wgsl"
            )),
        ),
        (
            "overlay",
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/engine/render/gpu/shaders/overlay.wgsl"
            )),
        ),
        (
            "depth probe",
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/engine/render/gpu/shaders/depth_probe.wgsl"
            )),
        ),
        (
            "depth probe",
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/engine/render/gpu/shaders/depth_probe_multisampled.wgsl"
            )),
        ),
    ] {
        stages.push((name, source.to_owned(), ""));
    }
    #[cfg(not(feature = "experimental"))]
    for (name, source) in [
        (
            "globe daylight vertex",
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/engine/render/gpu/shaders/globe_daylight.vertex.wgsl"
            )),
        ),
        (
            "globe daylight fragment",
            concat!(
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/engine/render/gpu/shaders/daylight.wgsl.part"
                )),
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/engine/render/gpu/shaders/globe_daylight.fragment.wgsl"
                ))
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
fn every_webgl2_shader_translates_to_glsl_es300() {
    let options = naga::back::glsl::Options {
        version: naga::back::glsl::Version::Embedded {
            version: 300,
            is_webgl: true,
        },
        writer_flags: naga::back::glsl::WriterFlags::ADJUST_COORDINATE_SPACE,
        ..naga::back::glsl::Options::default()
    };
    for (name, source, entry) in stages() {
        // Compute/multisampled depth shaders are WebGPU/native-only; WebGL2 runs 1x MSAA.
        if source.contains("texture_multisampled_2d")
            || source.contains("texture_depth_multisampled_2d")
            || source.contains("@compute")
        {
            continue;
        }
        let (module, info) = validated(name, &source);
        for point in module
            .entry_points
            .iter()
            .filter(|point| entry.is_empty() || point.name == entry)
        {
            let pipeline_options = naga::back::glsl::PipelineOptions {
                shader_stage: point.stage,
                entry_point: point.name.clone(),
                multiview: None,
            };
            let mut glsl = String::new();
            let mut writer = naga::back::glsl::Writer::new(
                &mut glsl,
                &module,
                &info,
                &options,
                &pipeline_options,
                naga::proc::BoundsCheckPolicies::default(),
            )
            .unwrap_or_else(|error| panic!("{name} ({}): GLSL ES 300 init {error:?}", point.name));
            writer
                .write()
                .unwrap_or_else(|error| panic!("{name} ({}): GLSL ES 300 {error:?}", point.name));
            assert!(!glsl.is_empty(), "{name}: empty GLSL ES 300 output");
        }
    }
}

#[test]
fn every_shader_spirv_roundtrips_with_strict_capabilities() {
    let spv_out = naga::back::spv::Options {
        flags: naga::back::spv::WriterFlags::LABEL_VARYINGS
            | naga::back::spv::WriterFlags::CLAMP_FRAG_DEPTH,
        ..naga::back::spv::Options::default()
    };
    let spv_in = naga::front::spv::Options {
        adjust_coordinate_space: false,
        strict_capabilities: true,
        block_ctx_dump_prefix: None,
    };
    for (name, source, _) in stages() {
        let (module, info) = validated(name, &source);
        let words = naga::back::spv::write_vec(&module, &info, &spv_out, None)
            .unwrap_or_else(|error| panic!("{name}: SPIR-V encode {error:?}"));
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_ne_bytes()).collect();
        let decoded = naga::front::spv::parse_u8_slice(&bytes, &spv_in)
            .unwrap_or_else(|error| panic!("{name}: SPIR-V decode {error:?}"));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&decoded)
        .unwrap_or_else(|error| panic!("{name}: SPIR-V validate {error:?}"));
    }
}

fn minify_wgsl_for_test(source: &str) -> String {
    let mut out = String::new();
    for raw_line in source.lines() {
        let trimmed = raw_line
            .split_once("//")
            .map_or(raw_line, |(head, _)| head)
            .trim();
        if !trimmed.is_empty() {
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out
}

#[test]
fn every_shader_file_is_part_of_a_pipeline() {
    let stages = stages();
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/engine/render/gpu/shaders");
    for entry in std::fs::read_dir(&directory).expect("shader directory") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("wgsl") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("shader");
        let minified = minify_wgsl_for_test(&text);
        assert!(
            stages
                .iter()
                .any(|(_, source, _)| source.contains(minified.trim())),
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
