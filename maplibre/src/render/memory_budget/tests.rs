use super::{
    MemoryBudget, MemoryPressure, CRITICAL_MEMORY_BYTES, DRAPE_TEXTURES_WHEN_CRITICAL,
    DRAPE_TEXTURES_WHEN_LOW, DRAPE_TEXTURE_LIMIT, LOW_MEMORY_BYTES, TILES_IN_FLIGHT_WHEN_LOW,
};

#[test]
fn an_unknown_budget_only_holds_the_texture_cap() {
    let budget = MemoryBudget::default();
    assert!(!budget.is_tight());
    assert!(budget.allows_drape_texture(DRAPE_TEXTURE_LIMIT - 1));
    assert!(!budget.allows_drape_texture(DRAPE_TEXTURE_LIMIT));
}

#[test]
fn a_low_budget_allows_no_new_texture_and_a_few_tiles_in_flight() {
    let budget = MemoryBudget {
        available_bytes: Some(LOW_MEMORY_BYTES - 1),
    };
    assert_eq!(budget.pressure(), MemoryPressure::Low);
    assert!(budget.is_tight());
    assert!(budget.allows_drape_texture(DRAPE_TEXTURES_WHEN_LOW - 1));
    assert!(!budget.allows_drape_texture(DRAPE_TEXTURES_WHEN_LOW));
    assert_eq!(budget.tiles_in_flight_allowed(24), TILES_IN_FLIGHT_WHEN_LOW);
    let roomy = MemoryBudget {
        available_bytes: Some(LOW_MEMORY_BYTES),
    };
    assert_eq!(roomy.pressure(), MemoryPressure::Comfortable);
    assert!(roomy.allows_drape_texture(0));
    assert_eq!(roomy.tiles_in_flight_allowed(24), 24);
}

#[test]
fn a_critical_budget_allows_nothing_in_flight() {
    let budget = MemoryBudget {
        available_bytes: Some(CRITICAL_MEMORY_BYTES - 1),
    };
    assert_eq!(budget.pressure(), MemoryPressure::Critical);
    assert_eq!(budget.tiles_in_flight_allowed(24), 0);
    assert!(budget.allows_drape_texture(DRAPE_TEXTURES_WHEN_CRITICAL - 1));
    assert!(!budget.allows_drape_texture(DRAPE_TEXTURES_WHEN_CRITICAL));
}

#[test]
fn oscillating_host_reports_cannot_switch_drape_covering_each_frame() {
    let mut tracker = super::MemoryBudgetTracker::default();
    assert_eq!(tracker.update(Some(2 << 30)).drape_textures_allowed(), 64);
    for megabytes in [990, 1046, 981, 1100, 997, 1200, 1041] {
        let budget = tracker.update(Some(megabytes << 20));
        assert_eq!(budget.pressure(), MemoryPressure::Low);
        assert_eq!(budget.drape_textures_allowed(), 32);
    }
    assert_eq!(
        tracker.update(Some(1600 << 20)).drape_textures_allowed(),
        64
    );
    assert_eq!(
        tracker.update(Some(400 << 20)).pressure(),
        MemoryPressure::Critical
    );
    assert_eq!(
        tracker.update(Some(600 << 20)).pressure(),
        MemoryPressure::Critical
    );
    assert_eq!(
        tracker.update(Some(800 << 20)).pressure(),
        MemoryPressure::Low
    );
}

#[test]
fn a_frame_admits_its_allowance_and_the_first_upload_of_each_kind() {
    use super::{FrameUploads, UploadKind::*, FRAME_UPLOAD_BYTES, IMAGE_UPLOAD_FLOOR_BYTES};
    let geometry_share = FRAME_UPLOAD_BYTES - IMAGE_UPLOAD_FLOOR_BYTES;
    let mut frame = FrameUploads::default();
    assert!(frame.take(Geometry, geometry_share - 10));
    assert!(
        !frame.take(Geometry, 11),
        "geometry leaves the images their floor"
    );
    assert!(frame.take(Geometry, 10));
    assert!(
        frame.take(Image, IMAGE_UPLOAD_FLOOR_BYTES),
        "images keep their floor"
    );
    assert!(!frame.take(Image, 1));
    assert_eq!(frame.used(), FRAME_UPLOAD_BYTES);

    frame.reset();
    assert!(
        frame.take(Geometry, 2 * FRAME_UPLOAD_BYTES),
        "one oversized layer is admitted"
    );
    assert!(!frame.take(Geometry, 1));
    assert!(
        frame.take(Image, 2 * FRAME_UPLOAD_BYTES),
        "and one oversized texture"
    );
    assert!(!frame.take(Image, 1));
}

#[test]
fn images_use_what_geometry_leaves() {
    use super::{FrameUploads, UploadKind::*, FRAME_UPLOAD_BYTES};
    let mut frame = FrameUploads::default();
    assert!(frame.take(Image, FRAME_UPLOAD_BYTES - 1));
    assert!(frame.take(Image, 1));
    assert!(!frame.take(Image, 1));
    assert!(frame.take(Geometry, 1), "geometry still starts");
    assert!(!frame.take(Geometry, 1));
}

#[test]
fn can_take_probes_allowance_without_burning_budget_on_saturated_uploads() {
    use super::{FrameUploads, UploadKind::*, FRAME_UPLOAD_BYTES, IMAGE_UPLOAD_FLOOR_BYTES};
    let geometry_share = FRAME_UPLOAD_BYTES - IMAGE_UPLOAD_FLOOR_BYTES;
    let mut frame = FrameUploads::default();
    assert!(frame.can_take(Geometry, 2 * FRAME_UPLOAD_BYTES));
    assert_eq!(frame.used(), 0, "can_take must not consume allowance");
    assert!(frame.take(Geometry, geometry_share - 4));
    assert!(!frame.can_take(Geometry, 8));
    assert!(frame.can_take(Geometry, 4));
}

#[test]
fn uncovered_tiles_get_emergency_moving_upload_headroom_past_soft_cap() {
    use super::{
        FrameUploads, MOVING_GEOMETRY_UPLOAD_BYTES, MOVING_UNCOVERED_UPLOADS_PER_FRAME,
        MOVING_UPLOADS_PER_FRAME,
    };
    let mut frame = FrameUploads::default();
    frame.set_moving(true);
    for _ in 0..MOVING_UPLOADS_PER_FRAME {
        frame.count_geometry_tile();
    }
    assert!(
        frame.geometry_tiles_exhausted_for(false),
        "covered refinement tiles stop at soft moving cap"
    );
    assert!(
        !frame.geometry_tiles_exhausted_for(true),
        "uncovered hole-filling tiles continue up to emergency moving cap"
    );
    assert!(frame.take_geometry(MOVING_GEOMETRY_UPLOAD_BYTES, false));
    assert!(!frame.can_take_geometry(1024, false));
    assert!(
        frame.can_take_geometry(1024, true),
        "uncovered tiles retain byte headroom past covered moving cap"
    );
    while !frame.geometry_tiles_exhausted_for(true) {
        frame.count_geometry_tile();
    }
    assert!(frame.geometry_tiles_exhausted_for(true));
    assert_eq!(frame.geometry_tiles, MOVING_UNCOVERED_UPLOADS_PER_FRAME);
}

#[cfg(all(feature = "headless", not(target_arch = "wasm32")))]
mod uploads;
