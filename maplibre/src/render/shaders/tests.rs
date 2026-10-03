//! Every production shader translates to MSL and SPIR-V, so a native render node can take the
//! same WGSL through naga instead of a second copy of the shader.

use super::{
    fill_extrusion::ExtrusionPass, texture::DemShading, AtmosphereShader, BackgroundPatternShader,
    BackgroundShader, CircleShader, FillExtrusionShader, FillPatternShader, FillShader,
    GlobeBackgroundPatternShader, GlobeBackgroundShader, HeatmapCompositeShader,
    HeatmapDensityShader, LineShader, RasterShader, Shader, SkyShader, SymbolShader, TerrainShader,
    TileMaskShader,
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
        ("heatmap density", Box::new(HeatmapDensityShader)),
        (
            "heatmap composite",
            Box::new(HeatmapCompositeShader { format }),
        ),
        ("fill", Box::new(FillShader { format })),
        ("fill pattern", Box::new(FillPatternShader { format })),
        ("symbol", Box::new(SymbolShader { format })),
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

#[path = "tests/pure_globe.rs"]
mod pure_globe;
