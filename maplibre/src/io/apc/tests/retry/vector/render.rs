use super::super::fixture::{Fixture, TestEnvironment};
use crate::{
    plugin::Plugin,
    render::{resource::Head, RenderPlugin},
    schedule::{Schedule, Stage},
    vector::{DefaultVectorTransferables, VectorPlugin},
};

pub(super) struct Frames(Schedule);

impl Frames {
    pub fn new(test: &mut Fixture) -> Self {
        let mut schedule = Schedule::default();
        let mut plugins: Vec<Box<dyn Plugin<TestEnvironment>>> = vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        ];
        if test.context.style.terrain.is_some() {
            plugins.push(Box::new(crate::raster::RasterPlugin::<
                crate::raster::DefaultRasterTransferables,
            >::default()));
            plugins.push(Box::new(crate::terrain::TerrainPlugin::<
                crate::terrain::DefaultDemTransferables,
            >::default()));
        }
        for plugin in plugins {
            plugin.build(
                &mut schedule,
                test.kernel.clone(),
                &mut test.context.world,
                &mut test.context.renderer.render_graph,
            );
        }
        Self(schedule)
    }

    pub fn render(&mut self, test: &mut Fixture) -> Vec<u8> {
        self.0.run(&mut test.context).expect("rendered frame");
        read_blocking(test)
    }
}

fn read_blocking(test: &Fixture) -> Vec<u8> {
    let renderer = &test.context.renderer;
    let Head::Headless(head) = renderer.resources.surface.head() else {
        panic!("offscreen target");
    };
    let texture = head.texture();
    let padded = (texture.width() * 4).div_ceil(256) * 256;
    let buffer = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vector retry pixels"),
        size: u64::from(padded * texture.height()),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    renderer.queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |result| {
        result.expect("readback mapping")
    });
    renderer
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU completes");
    let pixels = buffer
        .slice(..)
        .get_mapped_range()
        .expect("mapped pixels")
        .chunks_exact(padded as usize)
        .flat_map(|row| row[..texture.width() as usize * 4].iter().copied())
        .collect();
    buffer.unmap();
    pixels
}

pub(super) fn assert_green(pixels: &[u8]) {
    let center = &pixels[(8 * 16 + 8) * 4..(8 * 16 + 8) * 4 + 4];
    assert_eq!(
        center,
        [0, 255, 0, 255],
        "recovered MVT must reach the actual draw"
    );
}
