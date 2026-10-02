use super::*;

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

#[test]
fn percentiles_take_the_nearest_rank() {
    let values = (1..=100).map(ms).collect();
    assert_eq!(
        Percentiles::of(values),
        Some(Percentiles {
            p50: ms(50),
            p95: ms(95),
            p99: ms(99)
        })
    );
    assert_eq!(Percentiles::of(vec![ms(7)]).map(|p| p.p99), Some(ms(7)));
    assert_eq!(Percentiles::of(Vec::new()), None);
}

#[test]
fn a_frame_gathers_the_map_s_and_the_host_s_measurements_under_one_number() {
    let mut trace = FrameTrace::new(8);
    trace.record_map_frame(&FrameStats {
        frame: 5,
        host_frame: 3,
        stages: vec![("Queue".into(), ms(2))],
        gpu: Some(ms(4)),
        upload_bytes: 1024,
        drape_redraws: 2,
        draws: 10,
    });
    trace.record_span(3, "queue-wait", Clock::Cpu, ms(1));
    // Encoding the copy is CPU time; the copy itself is GPU time, and the two stay apart.
    trace.record_span(3, "copy", Clock::Cpu, ms(1));
    trace.record_span(3, "copy", Clock::Gpu, ms(3));
    trace.record_presentation(3, ms(11), ms(9));
    trace.record_device(
        3,
        DeviceSample {
            resident_bytes: Some(1 << 30),
            thermal_state: Some(1),
        },
    );
    let summary = trace.summary();
    assert_eq!(summary.frames, 1);
    assert_eq!(summary.spans["cpu/copy"].p50, ms(1));
    assert_eq!(summary.spans["gpu/copy"].p50, ms(3));
    assert!(
        !summary.spans.contains_key("gpu/map"),
        "the map's GPU time comes with its readback, under the frame it measured"
    );
    assert_eq!(summary.spans["cpu/Queue"].p50, ms(2));
    assert_eq!((summary.upload_bytes, summary.drape_redraws), (1024, 2));
    assert_eq!(summary.missed_deadlines, 0);
    let export = trace.export();
    let json = serde_json::to_value(&export).expect("JSON");
    assert_eq!(json["frames"][0]["device"]["thermal_state"], 1);
    assert_eq!(json["summary"]["frames"], 1);
    assert_eq!(trace.summary().frames, 0, "an export empties the window");
}

#[test]
fn late_frames_are_counted_as_missed() {
    let mut trace = FrameTrace::new(8);
    trace.record_presentation(1, ms(11), ms(9));
    trace.record_presentation(2, ms(11), ms(13));
    trace.record_presentation(3, ms(11), ms(12));
    let summary = trace.summary();
    assert_eq!(summary.missed_deadlines, 2);
    assert_eq!(summary.completed.map(|p| p.p50), Some(ms(12)));
}

#[test]
fn records_that_lose_their_frame_are_counted_as_dropped() {
    let mut trace = FrameTrace::new(2);
    trace.record_span(1, "queue-wait", Clock::Cpu, ms(1));
    trace.record_span(2, "queue-wait", Clock::Cpu, ms(1));
    trace.record_span(3, "queue-wait", Clock::Cpu, ms(1));
    assert_eq!(
        trace.summary().dropped,
        1,
        "frame 1 was pushed out unexported"
    );
    trace.record_span(1, "copy", Clock::Gpu, ms(1));
    assert_eq!(
        trace.summary().dropped,
        2,
        "a record for a frame that left is dropped"
    );
    let frames: Vec<u64> = trace.export().frames.iter().map(|r| r.frame).collect();
    assert_eq!(frames, [2, 3]);
    assert_eq!(
        trace.summary().dropped,
        0,
        "an export starts the count again"
    );
}
