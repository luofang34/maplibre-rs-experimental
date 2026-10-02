use super::*;

#[test]
fn tile_ids_follow_the_reference_numbering() {
    // Values from the PMTiles specification's reference implementation.
    assert_eq!(tile_id(0, 0, 0).expect("id"), 0);
    assert_eq!(tile_id(1, 0, 0).expect("id"), 1);
    assert_eq!(tile_id(1, 0, 1).expect("id"), 2);
    assert_eq!(tile_id(1, 1, 1).expect("id"), 3);
    assert_eq!(tile_id(1, 1, 0).expect("id"), 4);
    assert_eq!(tile_id(2, 0, 0).expect("id"), 5);
    assert_eq!(tile_id(12, 3423, 1763).expect("id"), 19_078_479);
    assert!(tile_id(32, 0, 0).is_err());
    assert!(tile_id(1, 2, 0).is_err(), "column outside its zoom");
    assert!(tile_id(3, 0, 8).is_err(), "row outside its zoom");
}

fn varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

#[test]
fn a_directory_decodes_runs_lengths_and_contiguous_offsets() {
    let mut bytes = Vec::new();
    for value in [3, 1, 4, 300, 2, 1, 0, 10, 20, 30, 1, 0, 1000] {
        varint(value, &mut bytes);
    }
    let entries = parse_directory(&bytes).expect("directory");
    let entry = |tile_id, offset, length, run_length| Entry {
        tile_id,
        offset,
        length,
        run_length,
    };
    assert_eq!(
        entries,
        vec![
            entry(1, 0, 10, 2),
            entry(5, 10, 20, 1),
            entry(305, 999, 30, 0)
        ]
    );
    assert_eq!(find(&entries, 2), Some(Found::Tile(entries[0])));
    assert_eq!(find(&entries, 3), None, "past the run");
    assert_eq!(find(&entries, 0), None, "before the first entry");
    assert_eq!(find(&entries, 400), Some(Found::Leaf(entries[2])));
    assert!(
        parse_directory(&bytes[..5]).is_err(),
        "a cut directory is an error"
    );
}
