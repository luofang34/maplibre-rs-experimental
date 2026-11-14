//! Synthetic OpenType (`sfnt` + `GSUB` + `GPOS`), `WOFF`, and `WOFF2` container builders for
//! pure-Rust unit tests.

use std::io::Write;

use flate2::{write::ZlibEncoder, Compression};

use super::super::{
    container::{assemble_sfnt, SfntTable},
    FontError,
};

pub(super) const SYNTH_BOXES: &[Option<(i16, i16, i16, i16)>] = &[
    Some((50, 0, 450, 700)),     // 0 .notdef
    None,                        // 1 space
    Some((40, 0, 660, 700)),     // 2 A
    Some((40, 0, 660, 700)),     // 3 V
    Some((60, 0, 380, 720)),     // 4 f
    Some((80, 0, 260, 520)),     // 5 i
    Some((60, 0, 620, 720)),     // 6 fi ligature (unencoded)
    Some((50, 0, 650, 680)),     // 7 ka
    Some((100, -180, 350, -20)), // 8 virama
    Some((50, 0, 650, 680)),     // 9 ssa
    Some((50, -120, 710, 680)),  // 10 ksha conjunct (unencoded)
    Some((50, -100, 950, 820)),  // 11 U+2A6A5
];

/// Builds a synthetic OpenType `sfnt` font (`upem = 1000`) with `GSUB` ligatures (`fi`, `क्ष`)
/// and `GPOS` pair kerning (`AV -> -120`).
pub(crate) fn build_synthetic_sfnt() -> Result<Vec<u8>, FontError> {
    let advances: &[(u16, i16)] = &[
        (500, 50),
        (250, 0),
        (700, 40),
        (700, 40),
        (400, 60),
        (320, 80),
        (680, 60),
        (700, 50),
        (0, 100),
        (700, 50),
        (760, 50),
        (1000, 50),
    ];

    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    for &bbox in SYNTH_BOXES {
        loca.extend_from_slice(&((glyf.len() / 2) as u16).to_be_bytes());
        if let Some((x0, y0, x1, y1)) = bbox {
            glyf.extend_from_slice(&1_i16.to_be_bytes());
            glyf.extend_from_slice(&x0.to_be_bytes());
            glyf.extend_from_slice(&y0.to_be_bytes());
            glyf.extend_from_slice(&x1.to_be_bytes());
            glyf.extend_from_slice(&y1.to_be_bytes());
            glyf.extend_from_slice(&3_u16.to_be_bytes());
            glyf.extend_from_slice(&0_u16.to_be_bytes());
            glyf.extend_from_slice(&[0x01, 0x01, 0x01, 0x01]);
            for dx in [x0, 0, x1 - x0, 0] {
                glyf.extend_from_slice(&dx.to_be_bytes());
            }
            for dy in [y0, y1 - y0, 0, y0 - y1] {
                glyf.extend_from_slice(&dy.to_be_bytes());
            }
            if glyf.len() % 2 != 0 {
                glyf.push(0);
            }
        }
    }
    loca.extend_from_slice(&((glyf.len() / 2) as u16).to_be_bytes());

    let mut head = vec![0_u8; 54];
    head[0..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
    head[12..16].copy_from_slice(&0x5F0F_3CF5_u32.to_be_bytes());
    head[16..18].copy_from_slice(&0x000B_u16.to_be_bytes());
    head[18..20].copy_from_slice(&1000_u16.to_be_bytes());
    head[38..40].copy_from_slice(&(-200_i16).to_be_bytes());
    head[40..42].copy_from_slice(&1000_i16.to_be_bytes());
    head[42..44].copy_from_slice(&850_i16.to_be_bytes());

    let mut hhea = vec![0_u8; 36];
    hhea[0..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
    hhea[4..6].copy_from_slice(&820_i16.to_be_bytes());
    hhea[6..8].copy_from_slice(&(-180_i16).to_be_bytes());
    hhea[10..12].copy_from_slice(&1000_u16.to_be_bytes());
    hhea[34..36].copy_from_slice(&(SYNTH_BOXES.len() as u16).to_be_bytes());

    let mut maxp = vec![0_u8; 32];
    maxp[0..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
    maxp[4..6].copy_from_slice(&(SYNTH_BOXES.len() as u16).to_be_bytes());
    maxp[6..8].copy_from_slice(&8_u16.to_be_bytes());
    maxp[8..10].copy_from_slice(&2_u16.to_be_bytes());

    let mut hmtx = Vec::with_capacity(advances.len() * 4);
    for &(adv, lsb) in advances {
        hmtx.extend_from_slice(&adv.to_be_bytes());
        hmtx.extend_from_slice(&lsb.to_be_bytes());
    }

    let mut os2 = vec![0_u8; 78];
    os2[0..2].copy_from_slice(&2_u16.to_be_bytes());
    os2[4..6].copy_from_slice(&400_u16.to_be_bytes());
    os2[6..8].copy_from_slice(&5_u16.to_be_bytes());
    os2[62..64].copy_from_slice(&0x0040_u16.to_be_bytes());

    let mut post = vec![0_u8; 32];
    post[0..4].copy_from_slice(&0x0003_0000_u32.to_be_bytes());

    assemble_sfnt(
        0x0001_0000,
        vec![
            SfntTable {
                tag: *b"OS/2",
                data: os2,
            },
            SfntTable {
                tag: *b"cmap",
                data: build_cmap_format12(&[
                    (0x0020, 1),
                    (0x0041, 2),
                    (0x0056, 3),
                    (0x0066, 4),
                    (0x0069, 5),
                    (0x0915, 7),
                    (0x094D, 8),
                    (0x0937, 9),
                    (0x2A6A5, 11),
                ]),
            },
            SfntTable {
                tag: *b"glyf",
                data: glyf,
            },
            SfntTable {
                tag: *b"head",
                data: head,
            },
            SfntTable {
                tag: *b"hhea",
                data: hhea,
            },
            SfntTable {
                tag: *b"hmtx",
                data: hmtx,
            },
            SfntTable {
                tag: *b"loca",
                data: loca,
            },
            SfntTable {
                tag: *b"maxp",
                data: maxp,
            },
            SfntTable {
                tag: *b"name",
                data: build_name_table("TestSans"),
            },
            SfntTable {
                tag: *b"post",
                data: post,
            },
            SfntTable {
                tag: *b"GSUB",
                data: build_gsub_ligatures(),
            },
            SfntTable {
                tag: *b"GPOS",
                data: build_gpos_kern(),
            },
        ],
    )
}

fn build_name_table(family: &str) -> Vec<u8> {
    let utf16: Vec<u8> = family
        .encode_utf16()
        .flat_map(|u| u.to_be_bytes())
        .collect();
    let mut out = Vec::new();
    for v in [0_u16, 1, 18, 3, 1, 0x0409, 1, utf16.len() as u16, 0] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&utf16);
    out
}

fn build_cmap_format12(mappings: &[(u32, u32)]) -> Vec<u8> {
    let mut sorted = mappings.to_vec();
    sorted.sort_by_key(|&(cp, _)| cp);
    let sub_len = 16 + sorted.len() * 12;
    let mut out = Vec::with_capacity(12 + sub_len);
    for v in [0_u16, 1, 3, 10] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&12_u32.to_be_bytes());
    out.extend_from_slice(&12_u16.to_be_bytes());
    out.extend_from_slice(&0_u16.to_be_bytes());
    out.extend_from_slice(&(sub_len as u32).to_be_bytes());
    out.extend_from_slice(&0_u32.to_be_bytes());
    out.extend_from_slice(&(sorted.len() as u32).to_be_bytes());
    for (cp, gid) in sorted {
        out.extend_from_slice(&cp.to_be_bytes());
        out.extend_from_slice(&cp.to_be_bytes());
        out.extend_from_slice(&gid.to_be_bytes());
    }
    out
}

fn build_gsub_ligatures() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&[0x00, 0x01, 0x00, 0x00, 0x00, 10, 0x00, 46, 0x00, 66]);
    b.extend_from_slice(&3_u16.to_be_bytes());
    for tag in [b"DFLT", b"dev2", b"latn"] {
        b.extend_from_slice(tag);
        b.extend_from_slice(&20_u16.to_be_bytes());
    }
    b.extend_from_slice(&[0x00, 4, 0x00, 0]);
    b.extend_from_slice(&[0x00, 0, 0xFF, 0xFF, 0x00, 2, 0x00, 0, 0x00, 1, 0, 0]);
    b.extend_from_slice(&2_u16.to_be_bytes());
    for tag in [b"akhn", b"liga"] {
        b.extend_from_slice(tag);
        b.extend_from_slice(&14_u16.to_be_bytes());
    }
    b.extend_from_slice(&[0x00, 0, 0x00, 1, 0x00, 0]);
    b.extend_from_slice(&[0x00, 1, 0x00, 4]);
    b.extend_from_slice(&[0x00, 4, 0x00, 0, 0x00, 1, 0x00, 8]);
    b.extend_from_slice(&[0x00, 1, 0x00, 10, 0x00, 2, 0x00, 18, 0x00, 32]);
    b.extend_from_slice(&[0x00, 1, 0x00, 2, 0x00, 4, 0x00, 7]);
    b.extend_from_slice(&[0x00, 1, 0x00, 4]);
    b.extend_from_slice(&[0x00, 6, 0x00, 2, 0x00, 5, 0, 0, 0, 0]);
    b.extend_from_slice(&[0x00, 1, 0x00, 4]);
    b.extend_from_slice(&[0x00, 10, 0x00, 3, 0x00, 8, 0x00, 9]);
    b
}

fn build_gpos_kern() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&[0x00, 0x01, 0x00, 0x00, 0x00, 10, 0x00, 36, 0x00, 50]);
    b.extend_from_slice(&2_u16.to_be_bytes());
    for tag in [b"DFLT", b"latn"] {
        b.extend_from_slice(tag);
        b.extend_from_slice(&14_u16.to_be_bytes());
    }
    b.extend_from_slice(&[0x00, 4, 0x00, 0]);
    b.extend_from_slice(&[0x00, 0, 0xFF, 0xFF, 0x00, 1, 0x00, 0]);
    b.extend_from_slice(&1_u16.to_be_bytes());
    b.extend_from_slice(b"kern");
    b.extend_from_slice(&8_u16.to_be_bytes());
    b.extend_from_slice(&[0x00, 0, 0x00, 1, 0x00, 0]);
    b.extend_from_slice(&[0x00, 1, 0x00, 4]);
    b.extend_from_slice(&[0x00, 2, 0x00, 0, 0x00, 1, 0x00, 8]);
    b.extend_from_slice(&[0x00, 1, 0x00, 12, 0x00, 0x04, 0x00, 0x00, 0x00, 1, 0x00, 18]);
    b.extend_from_slice(&[0x00, 1, 0x00, 1, 0x00, 2]);
    b.extend_from_slice(&1_u16.to_be_bytes());
    b.extend_from_slice(&3_u16.to_be_bytes());
    b.extend_from_slice(&(-120_i16).to_be_bytes());
    b
}

pub(crate) fn encode_woff1(sfnt: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let num_tables = u16::from_be_bytes([sfnt[4], sfnt[5]]) as usize;
    let mut recs = Vec::with_capacity(num_tables);
    for idx in 0..num_tables {
        let base = 12 + idx * 16;
        let tag: [u8; 4] = sfnt[base..base + 4].try_into()?;
        let csum = u32::from_be_bytes(sfnt[base + 4..base + 8].try_into()?);
        let off = u32::from_be_bytes(sfnt[base + 8..base + 12].try_into()?) as usize;
        let len = u32::from_be_bytes(sfnt[base + 12..base + 16].try_into()?) as usize;
        let orig = sfnt[off..off + len].to_vec();
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&orig)?;
        let z = enc.finish()?;
        let comp = if z.len() < orig.len() {
            z
        } else {
            orig.clone()
        };
        recs.push((tag, orig, comp, csum));
    }

    let dir_end = 44 + recs.len() * 20;
    let body_len: usize = recs.iter().map(|r| (r.2.len() + 3) & !3).sum();
    let total_len = dir_end + body_len;
    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(b"wOFF");
    out.extend_from_slice(&sfnt[0..4]);
    out.extend_from_slice(&(total_len as u32).to_be_bytes());
    out.extend_from_slice(&(num_tables as u16).to_be_bytes());
    out.extend_from_slice(&0_u16.to_be_bytes());
    out.extend_from_slice(&(sfnt.len() as u32).to_be_bytes());
    out.extend_from_slice(&[0_u8; 24]);

    let mut cursor = dir_end;
    for (tag, orig, comp, csum) in &recs {
        out.extend_from_slice(tag);
        out.extend_from_slice(&(cursor as u32).to_be_bytes());
        out.extend_from_slice(&(comp.len() as u32).to_be_bytes());
        out.extend_from_slice(&(orig.len() as u32).to_be_bytes());
        out.extend_from_slice(&csum.to_be_bytes());
        cursor += (comp.len() + 3) & !3;
    }
    for (_, _, comp, _) in &recs {
        out.extend_from_slice(comp);
        while (out.len() - dir_end) % 4 != 0 {
            out.push(0);
        }
    }
    Ok(out)
}

pub(crate) fn encode_woff2_transformed(sfnt: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut n_contour = Vec::new();
    let mut n_points = Vec::new();
    let mut flags = Vec::new();
    let mut glyph_coords = Vec::new();
    let bbox_bitmap = vec![0_u8; 4];

    let push_pt = |f: &mut Vec<u8>, c: &mut Vec<u8>, dx: i16, dy: i16| {
        f.push(124 + u8::from(dx >= 0) + if dy >= 0 { 2 } else { 0 });
        c.extend_from_slice(&dx.unsigned_abs().to_be_bytes());
        c.extend_from_slice(&dy.unsigned_abs().to_be_bytes());
    };
    for &bbox in SYNTH_BOXES {
        if let Some((x0, y0, x1, y1)) = bbox {
            n_contour.extend_from_slice(&1_i16.to_be_bytes());
            n_points.push(4_u8);
            push_pt(&mut flags, &mut glyph_coords, x0, y0);
            push_pt(&mut flags, &mut glyph_coords, 0, y1 - y0);
            push_pt(&mut flags, &mut glyph_coords, x1 - x0, 0);
            push_pt(&mut flags, &mut glyph_coords, 0, y0 - y1);
            glyph_coords.push(0_u8); // instructionLength = 0 (255UShort)
        } else {
            n_contour.extend_from_slice(&0_i16.to_be_bytes());
        }
    }

    let mut xform_glyf = Vec::new();
    for v in [0_u16, 0, SYNTH_BOXES.len() as u16, 0] {
        xform_glyf.extend_from_slice(&v.to_be_bytes());
    }
    for sz in [
        n_contour.len(),
        n_points.len(),
        flags.len(),
        glyph_coords.len(),
        0,
        bbox_bitmap.len(),
        0,
    ] {
        xform_glyf.extend_from_slice(&(sz as u32).to_be_bytes());
    }
    for s in [&n_contour, &n_points, &flags, &glyph_coords, &bbox_bitmap] {
        xform_glyf.extend_from_slice(s);
    }

    let num_tables = u16::from_be_bytes([sfnt[4], sfnt[5]]) as usize;
    let mut dir = Vec::new();
    let mut stream = Vec::new();
    for idx in 0..num_tables {
        let base = 12 + idx * 16;
        let tag: [u8; 4] = sfnt[base..base + 4].try_into()?;
        let off = u32::from_be_bytes(sfnt[base + 8..base + 12].try_into()?) as usize;
        let len = u32::from_be_bytes(sfnt[base + 12..base + 16].try_into()?) as usize;
        if &tag == b"glyf" {
            dir.push(10_u8);
            push_uint_base128(&mut dir, len as u32);
            push_uint_base128(&mut dir, xform_glyf.len() as u32);
            stream.extend_from_slice(&xform_glyf);
        } else if &tag == b"loca" {
            dir.push(11_u8);
            push_uint_base128(&mut dir, len as u32);
            push_uint_base128(&mut dir, 0);
        } else {
            dir.push(63_u8);
            dir.extend_from_slice(&tag);
            push_uint_base128(&mut dir, len as u32);
            stream.extend_from_slice(&sfnt[off..off + len]);
        }
    }

    let m = (stream.len().saturating_sub(1)) as u32;
    let hdr24: u32 = ((m & 0xFFFF) << 4) | (1 << 20);
    let mut compressed = Vec::with_capacity(stream.len() + 4);
    compressed.extend_from_slice(&[
        (hdr24 & 0xFF) as u8,
        ((hdr24 >> 8) & 0xFF) as u8,
        ((hdr24 >> 16) & 0xFF) as u8,
    ]);
    compressed.extend_from_slice(&stream);
    compressed.push(0x03);

    let total_len = 48 + dir.len() + compressed.len();
    let mut woff2 = Vec::with_capacity(total_len);
    woff2.extend_from_slice(b"wOF2");
    woff2.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
    woff2.extend_from_slice(&(total_len as u32).to_be_bytes());
    woff2.extend_from_slice(&(num_tables as u16).to_be_bytes());
    woff2.extend_from_slice(&0_u16.to_be_bytes());
    woff2.extend_from_slice(&(sfnt.len() as u32).to_be_bytes());
    woff2.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
    woff2.extend_from_slice(&[0_u8; 24]);
    woff2.extend_from_slice(&dir);
    woff2.extend_from_slice(&compressed);
    Ok(woff2)
}

fn push_uint_base128(out: &mut Vec<u8>, mut val: u32) {
    let mut tmp = [0_u8; 5];
    let mut len = 0usize;
    loop {
        tmp[len] = (val & 0x7F) as u8;
        val >>= 7;
        len += 1;
        if val == 0 {
            break;
        }
    }
    for i in (0..len).rev() {
        out.push(tmp[i] | if i > 0 { 0x80 } else { 0x00 });
    }
}
