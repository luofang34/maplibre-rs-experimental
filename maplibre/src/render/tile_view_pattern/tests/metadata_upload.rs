use std::cell::RefCell;

use cgmath::{Matrix4, SquareMatrix};

use super::*;
use crate::render::{camera::ViewProjection, shaders::ShaderTileMetadata};

const STRIDE: u64 = std::mem::size_of::<ShaderTileMetadata>() as u64;

#[derive(Default)]
struct Captured(RefCell<Vec<(u64, Vec<u8>)>>);

impl Queue<TestBuffer> for Captured {
    fn write_buffer(&self, _: &TestBuffer, offset: u64, bytes: &[u8]) {
        self.0.borrow_mut().push((offset, bytes.to_vec()));
    }
}

fn metadata(scale: f32) -> ShaderTileMetadata {
    ShaderTileMetadata::new(Matrix4::identity().into(), scale)
}

fn pattern(capacity: u64) -> TileViewPattern<Captured, TestBuffer> {
    TileViewPattern::new(BackingBufferDescriptor::new(TestBuffer, capacity * STRIDE))
}

#[test]
fn successive_uploads_allocate_disjoint_ranges_and_keep_prior_bytes() {
    let queue = Captured::default();
    let mut pattern = pattern(3);
    let first = pattern
        .upload_extra_metadata(&queue, &[metadata(1.0)])
        .expect("first");
    let second = pattern
        .upload_extra_metadata(&queue, &[metadata(2.0)])
        .expect("second");
    assert_eq!(first, vec![0..STRIDE]);
    assert_eq!(second, vec![STRIDE..2 * STRIDE]);
    assert_eq!(pattern.remaining_metadata_capacity(), 1);
    let writes = queue.0.borrow();
    assert_eq!(writes[0], (0, bytemuck::bytes_of(&metadata(1.0)).to_vec()));
    assert_eq!(
        writes[1],
        (STRIDE, bytemuck::bytes_of(&metadata(2.0)).to_vec())
    );
}

#[test]
fn a_failed_append_leaves_existing_allocations_and_capacity_intact() {
    let queue = Captured::default();
    let mut pattern = pattern(2);
    pattern
        .upload_extra_metadata(&queue, &[metadata(1.0)])
        .expect("first");
    let overflow = pattern.upload_extra_metadata(&queue, &[metadata(2.0); 2]);
    assert!(overflow.is_err(), "one free entry cannot hold two entries");
    assert_eq!(pattern.remaining_metadata_capacity(), 1);
    assert_eq!(
        queue.0.borrow().len(),
        1,
        "overflow must not write to the buffer"
    );
    let final_range = pattern
        .upload_extra_metadata(&queue, &[metadata(3.0)])
        .expect("last slot");
    assert_eq!(final_range, vec![STRIDE..2 * STRIDE]);
    assert_eq!(pattern.remaining_metadata_capacity(), 0);
}

#[test]
fn uploading_a_new_frame_resets_the_append_cursor() {
    let queue = Captured::default();
    let mut pattern = pattern(2);
    pattern
        .upload_extra_metadata(&queue, &[metadata(1.0); 2])
        .expect("full frame");
    assert_eq!(pattern.remaining_metadata_capacity(), 0);
    pattern.upload_pattern(
        &queue,
        &ViewProjection(Matrix4::identity()),
        512.0,
        512.0,
        Zoom::new(0.0),
    );
    assert_eq!(pattern.remaining_metadata_capacity(), 2);
    let ranges = pattern
        .upload_extra_metadata(&queue, &[metadata(2.0)])
        .expect("new frame");
    assert_eq!(ranges, vec![0..STRIDE]);
}
