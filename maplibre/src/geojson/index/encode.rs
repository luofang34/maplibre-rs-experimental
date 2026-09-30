//! Encodes indexed features as the `_geojson` layer of a vector tile.

use std::collections::HashMap;

use geozero::mvt::tile::{Feature, GeomType, Layer, Value as MvtValue};
use serde_json::Value;

use super::{IndexedFeature, Point, EXTENT};
use crate::style::source::GEOJSON_LAYER;

const MOVE_TO: u32 = 1;
const LINE_TO: u32 = 2;
const CLOSE_PATH: u32 = 7;

/// Builds the single `_geojson` layer of a tile.
#[derive(Default)]
pub(super) struct TileEncoder {
    features: Vec<Feature>,
    keys: Vec<String>,
    key_index: HashMap<String, u32>,
    values: Vec<MvtValue>,
    value_index: HashMap<String, u32>,
}

impl TileEncoder {
    fn key(&mut self, name: &str) -> u32 {
        if let Some(index) = self.key_index.get(name) {
            return *index;
        }
        let index = self.keys.len() as u32;
        self.keys.push(name.to_owned());
        self.key_index.insert(name.to_owned(), index);
        index
    }

    fn value(&mut self, value: &Value) -> Option<u32> {
        let (identity, encoded) = match value {
            Value::Null => return None,
            Value::Bool(flag) => (
                format!("b{flag}"),
                MvtValue {
                    bool_value: Some(*flag),
                    ..Default::default()
                },
            ),
            Value::String(text) => (
                format!("s{text}"),
                MvtValue {
                    string_value: Some(text.clone()),
                    ..Default::default()
                },
            ),
            Value::Number(number) => match number.as_i64() {
                Some(integer) => (
                    format!("i{integer}"),
                    MvtValue {
                        int_value: Some(integer),
                        ..Default::default()
                    },
                ),
                None => {
                    let real = number.as_f64()?;
                    (
                        format!("d{}", real.to_bits()),
                        MvtValue {
                            double_value: Some(real),
                            ..Default::default()
                        },
                    )
                }
            },
            other => (
                format!("j{other}"),
                MvtValue {
                    string_value: Some(other.to_string()),
                    ..Default::default()
                },
            ),
        };
        if let Some(index) = self.value_index.get(&identity) {
            return Some(*index);
        }
        let index = self.values.len() as u32;
        self.values.push(encoded);
        self.value_index.insert(identity, index);
        Some(index)
    }

    fn tags(&mut self, properties: &[(String, Value)]) -> Vec<u32> {
        let mut tags = Vec::with_capacity(properties.len() * 2);
        for (name, value) in properties {
            if let Some(value) = self.value(value) {
                tags.push(self.key(name));
                tags.push(value);
            }
        }
        tags
    }

    fn push(&mut self, source: &IndexedFeature, kind: GeomType, geometry: Vec<u32>) {
        if geometry.is_empty() {
            return;
        }
        let tags = self.tags(&source.properties);
        self.features.push(Feature {
            id: source.id,
            tags,
            r#type: Some(kind as i32),
            geometry,
        });
    }

    pub(super) fn feature(
        &mut self,
        source: &IndexedFeature,
        to_tile: &impl Fn(&Point) -> (i32, i32),
    ) {
        let geometry = &source.geometry;
        if !geometry.points.is_empty() {
            let mut commands = Vec::new();
            let mut cursor = (0, 0);
            commands.push(command(MOVE_TO, geometry.points.len() as u32));
            for point in &geometry.points {
                step(&mut commands, &mut cursor, to_tile(point));
            }
            self.push(source, GeomType::Point, commands);
        }
        let mut lines = Vec::new();
        let mut cursor = (0, 0);
        for line in &geometry.lines {
            let points = deduplicated(line.iter().map(to_tile));
            if points.len() >= 2 {
                path(&mut lines, &mut cursor, &points, false);
            }
        }
        self.push(source, GeomType::Linestring, lines);
        let mut polygons = Vec::new();
        let mut cursor = (0, 0);
        for polygon in &geometry.polygons {
            for (ring_index, ring) in polygon.iter().enumerate() {
                let mut points = deduplicated(ring.iter().map(to_tile));
                if points.len() > 1 && points.first() == points.last() {
                    points.pop();
                }
                if points.len() < 3 {
                    if ring_index == 0 {
                        break;
                    }
                    continue;
                }
                if (signed_area(&points) > 0) != (ring_index == 0) {
                    points.reverse();
                }
                path(&mut polygons, &mut cursor, &points, true);
            }
        }
        self.push(source, GeomType::Polygon, polygons);
    }

    pub(super) fn finish(self) -> Layer {
        Layer {
            version: 2,
            name: GEOJSON_LAYER.to_owned(),
            features: self.features,
            keys: self.keys,
            values: self.values,
            extent: Some(EXTENT),
        }
    }
}

fn command(id: u32, count: u32) -> u32 {
    (count << 3) | id
}

fn zigzag(value: i32) -> u32 {
    ((value << 1) ^ (value >> 31)) as u32
}

fn step(out: &mut Vec<u32>, cursor: &mut (i32, i32), to: (i32, i32)) {
    out.push(zigzag(to.0.wrapping_sub(cursor.0)));
    out.push(zigzag(to.1.wrapping_sub(cursor.1)));
    *cursor = to;
}

fn deduplicated(points: impl Iterator<Item = (i32, i32)>) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> = Vec::new();
    for point in points {
        if out.last() != Some(&point) {
            out.push(point);
        }
    }
    out
}

fn path(out: &mut Vec<u32>, cursor: &mut (i32, i32), points: &[(i32, i32)], close: bool) {
    out.push(command(MOVE_TO, 1));
    step(out, cursor, points[0]);
    out.push(command(LINE_TO, points.len() as u32 - 1));
    for point in &points[1..] {
        step(out, cursor, *point);
    }
    if close {
        out.push(command(CLOSE_PATH, 1));
    }
}

/// Twice the shoelace area in tile coordinates, positive for the clockwise rings of a y-down grid.
fn signed_area(points: &[(i32, i32)]) -> i64 {
    let mut area = 0_i64;
    for (index, point) in points.iter().enumerate() {
        let next = points[(index + 1) % points.len()];
        area += i64::from(point.0) * i64::from(next.1) - i64::from(next.0) * i64::from(point.1);
    }
    area
}
