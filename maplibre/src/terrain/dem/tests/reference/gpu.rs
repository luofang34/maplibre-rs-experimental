use super::{DemTile, Fixture, RgbaImage, EXTENT};
use crate::{
    render::{resource::Texture, settings::Msaa},
    terrain::dem::gpu_readback::{pipeline, sample_gpu_blocking},
};

fn assert_gpu_matches_cpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    dem: &DemTile,
) {
    let texture = Texture::new(
        Some("DEM regression"),
        device,
        wgpu::TextureFormat::Rgba8Unorm,
        dem.stride(),
        dem.stride(),
        Msaa { samples: 1 },
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    );
    queue.write_texture(
        texture.texture.as_image_copy(),
        dem.pixels(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(dem.stride() * 4),
            rows_per_image: None,
        },
        texture.size,
    );
    let heights = sample_gpu_blocking(device, queue, pipeline, dem, &texture);
    for (i, actual) in heights.into_iter().enumerate() {
        let x = (i % 9) as f64 * EXTENT / 8.0;
        let y = (i / 9) as f64 * EXTENT / 8.0;
        let expected = dem.elevation_at_tile_coords(x, y);
        let tolerance = expected.abs().max(1.0) * 1e-6;
        assert!(
            (f64::from(actual) - expected).abs() <= tolerance,
            "at ({x}, {y}): GPU {actual} != CPU {expected}"
        );
    }
}

#[tokio::test]
async fn production_shader_matches_cpu_at_pixel_centres_and_borders() {
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&Default::default())
        .await
        .expect("GPU adapter");
    let (device, queue) = adapter
        .request_device(&Default::default())
        .await
        .expect("GPU device");
    let pipeline = pipeline(&device);
    let fixture: Fixture =
        serde_json::from_str(include_str!("../gljs.json")).expect("GL JS fixture");
    for case in fixture.cases {
        let image = RgbaImage::from_raw(case.dim, case.dim, case.rgba).expect("tile pixels");
        let dem = DemTile::from_image(&image, case.unpack).expect("DEM");
        assert_gpu_matches_cpu(&device, &queue, &pipeline, &dem);
        let image =
            RgbaImage::from_raw(case.dim, case.dim, case.neighbour_rgba).expect("neighbour pixels");
        let neighbour = DemTile::from_image(&image, case.unpack).expect("DEM");
        for border in case.backfilled {
            let mut filled = dem.clone();
            filled
                .backfill_border(&neighbour, border.dx, border.dy)
                .expect("border");
            assert_gpu_matches_cpu(&device, &queue, &pipeline, &filled);
        }
    }
}
