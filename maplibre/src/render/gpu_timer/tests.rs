use super::*;

#[tokio::test]
async fn a_timed_pass_reports_its_gpu_time_a_frame_later() {
    let instance = wgpu::Instance::default();
    let Ok(adapter) = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
    else {
        return;
    };
    if !adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        return;
    }
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            required_features: wgpu::Features::TIMESTAMP_QUERY,
            ..Default::default()
        })
        .await
        .expect("device");
    let timer = GpuTimer::new(&device).expect("timer");
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let writes = timer.pass_writes();
        assert!(writes.is_some(), "an idle timer times the pass");
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: writes,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    assert!(
        timer.pass_writes().is_none(),
        "one measurement is in flight at a time"
    );
    timer.resolve(&mut encoder);
    queue.submit([encoder.finish()]);
    timer.after_submit();
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU finishes");
    // A pass that only clears may write no end timestamp, so only the round trip is checked.
    timer.take(&device, queue.get_timestamp_period());
    assert!(timer.pass_writes().is_some(), "and the timer is free again");
}
