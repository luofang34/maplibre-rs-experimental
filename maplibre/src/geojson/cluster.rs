//! Groups the points of a GeoJSON source into clusters for each zoom, as GL JS does with
//! supercluster: points within a pixel radius of one another merge, level by level from the
//! deepest zoom up, into a cluster at their weighted centre.

use rstar::{primitives::GeomWithData, RTree};
use serde_json::{json, Value};

use crate::style::expression::{EvaluationContext, Expression, FeatureProperties, Value as Datum};

/// Side of the tile the radius is measured on, in pixels.
const TILE_PIXELS: f64 = 512.0;

/// How a cluster property combines the values of the points it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reduce {
    Sum,
    Product,
    Max,
    Min,
    Any,
    All,
}

impl Reduce {
    fn parse(operator: &str) -> Option<Self> {
        Some(match operator {
            "+" => Self::Sum,
            "*" => Self::Product,
            "max" => Self::Max,
            "min" => Self::Min,
            "any" => Self::Any,
            "all" => Self::All,
            _ => return None,
        })
    }

    fn combine(self, left: &Value, right: &Value) -> Value {
        let numbers = || Some((left.as_f64()?, right.as_f64()?));
        let flags = || {
            (
                left.as_bool().unwrap_or(false),
                right.as_bool().unwrap_or(false),
            )
        };
        match self {
            Self::Sum => numbers().map_or(Value::Null, |(a, b)| json!(a + b)),
            Self::Product => numbers().map_or(Value::Null, |(a, b)| json!(a * b)),
            Self::Max => numbers().map_or(Value::Null, |(a, b)| json!(a.max(b))),
            Self::Min => numbers().map_or(Value::Null, |(a, b)| json!(a.min(b))),
            Self::Any => Value::Bool(flags().0 || flags().1),
            Self::All => Value::Bool(flags().0 && flags().1),
        }
    }
}

/// One cluster property: the expression mapping a point to its value and how values combine.
struct Property {
    name: String,
    map: Expression,
    reduce: Reduce,
}

/// The clustering a source asks for.
pub struct ClusterOptions {
    radius: f64,
    max_zoom: u8,
    min_points: usize,
    properties: Vec<Property>,
}

impl ClusterOptions {
    /// The options from a source declaration; properties that cannot be read are left out.
    pub fn new(
        radius: Option<f64>,
        max_zoom: Option<u8>,
        min_points: Option<usize>,
        properties: Option<&Value>,
        source_max_zoom: Option<u8>,
    ) -> Self {
        let max_zoom = max_zoom.unwrap_or_else(|| source_max_zoom.unwrap_or(18).saturating_sub(1));
        Self {
            radius: radius.unwrap_or(50.0),
            max_zoom: max_zoom.min(24),
            min_points: min_points.unwrap_or(2),
            properties: properties
                .and_then(Value::as_object)
                .map(|map| {
                    map.iter()
                        .filter_map(|(name, spec)| Self::property(name, spec))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// `[operator, map]` or `[operator, ["accumulated"], map]`, as the style specification
    /// writes a cluster property.
    fn property(name: &str, spec: &Value) -> Option<Property> {
        let items = spec.as_array()?;
        let operator = match items.first()? {
            Value::String(operator) => Reduce::parse(operator)?,
            _ => return None,
        };
        let map = items
            .get(1)
            .filter(|value| value.get(0) != Some(&json!("accumulated")))?;
        let map = if items.len() == 3 { items.get(2)? } else { map };
        Some(Property {
            name: name.to_owned(),
            map: Expression::parse(map).ok()?,
            reduce: operator,
        })
    }

    fn values(&self, properties: &[(String, Value)]) -> Vec<Value> {
        let feature: FeatureProperties = properties
            .iter()
            .map(|(key, value)| (key.clone(), Datum::from_json(value)))
            .collect();
        self.properties
            .iter()
            .map(|property| {
                property
                    .map
                    .evaluate(&EvaluationContext::for_feature(0.0, &feature))
                    .map_or(Value::Null, |value| value.to_json())
            })
            .collect()
    }
}

/// A point or a cluster on one level.
#[derive(Clone)]
struct Item {
    position: [f64; 2],
    /// Number of points a cluster holds; zero for a point.
    count: usize,
    /// The point's index, or the cluster's identifier.
    id: usize,
    /// Cluster property values, in the order of the options.
    values: Vec<Value>,
}

struct Level {
    items: Vec<Item>,
    tree: RTree<GeomWithData<[f64; 2], usize>>,
}

impl Level {
    fn new(items: Vec<Item>) -> Self {
        let entries = items
            .iter()
            .enumerate()
            .map(|(index, item)| GeomWithData::new(item.position, index))
            .collect();
        Self {
            items,
            tree: RTree::bulk_load(entries),
        }
    }
}

/// What lies in a window at a zoom.
pub enum Clustered {
    /// A point that stayed alone: its index among the points given to [`Clusters::new`].
    Point(usize, [f64; 2]),
    /// A cluster: where it is and the properties it carries.
    Cluster([f64; 2], u64, Vec<(String, Value)>),
}

/// A point to cluster: its world position and its properties.
pub type ClusterInput<'a> = ([f64; 2], &'a [(String, Value)]);

/// The points of a source clustered at every zoom.
pub struct Clusters {
    options: ClusterOptions,
    levels: Vec<Level>,
}

impl Clusters {
    /// Clusters `points`, given as world positions with their properties.
    pub fn new(points: &[ClusterInput<'_>], options: ClusterOptions) -> Self {
        let mut items: Vec<Item> = points
            .iter()
            .enumerate()
            .map(|(index, (position, properties))| Item {
                position: *position,
                count: 0,
                id: index,
                values: options.values(properties),
            })
            .collect();
        let max_zoom = usize::from(options.max_zoom);
        let mut levels: Vec<Option<Level>> = (0..=max_zoom + 1).map(|_| None).collect();
        levels[max_zoom + 1] = Some(Level::new(items.clone()));
        for zoom in (0..=max_zoom).rev() {
            let Some(previous) = levels[zoom + 1].as_ref() else {
                break;
            };
            items = Self::merge(previous, zoom, &options);
            levels[zoom] = Some(Level::new(items.clone()));
        }
        Self {
            options,
            levels: levels.into_iter().flatten().collect(),
        }
    }

    fn merge(previous: &Level, zoom: usize, options: &ClusterOptions) -> Vec<Item> {
        let radius = options.radius / (TILE_PIXELS * 2.0_f64.powi(zoom as i32));
        let mut merged_at = vec![usize::MAX; previous.items.len()];
        let mut result = Vec::new();
        for (index, item) in previous.items.iter().enumerate() {
            if merged_at[index] <= zoom {
                continue;
            }
            merged_at[index] = zoom;
            let neighbours: Vec<usize> = previous
                .tree
                .locate_within_distance(item.position, radius * radius)
                .map(|entry| entry.data)
                .collect();
            let own = item.count.max(1);
            let total = own
                + neighbours
                    .iter()
                    .filter(|neighbour| merged_at[**neighbour] > zoom)
                    .map(|neighbour| previous.items[*neighbour].count.max(1))
                    .sum::<usize>();
            if total > own && total >= options.min_points {
                let mut weighted = [item.position[0] * own as f64, item.position[1] * own as f64];
                let mut values = item.values.clone();
                for neighbour in neighbours {
                    if merged_at[neighbour] <= zoom {
                        continue;
                    }
                    merged_at[neighbour] = zoom;
                    let other = &previous.items[neighbour];
                    let weight = other.count.max(1) as f64;
                    weighted[0] += other.position[0] * weight;
                    weighted[1] += other.position[1] * weight;
                    for (index, property) in options.properties.iter().enumerate() {
                        values[index] = property
                            .reduce
                            .combine(&values[index], &other.values[index]);
                    }
                }
                result.push(Item {
                    position: [weighted[0] / total as f64, weighted[1] / total as f64],
                    count: total,
                    id: (index << 5) + zoom + 1 + previous.items.len(),
                    values,
                });
            } else {
                result.push(item.clone());
                if total > 1 {
                    for neighbour in neighbours {
                        if merged_at[neighbour] > zoom {
                            merged_at[neighbour] = zoom;
                            result.push(previous.items[neighbour].clone());
                        }
                    }
                }
            }
        }
        result
    }

    /// The points and clusters inside `window` (`[west, north, east, south]` in world units)
    /// at a tile zoom, in the order they were made.
    pub fn within(&self, zoom: u8, window: [f64; 4]) -> Vec<Clustered> {
        let level = &self.levels[usize::from(zoom).min(self.levels.len() - 1)];
        let corners = ([window[0], window[1]], [window[2], window[3]]);
        let mut hits: Vec<usize> = level
            .tree
            .locate_in_envelope_intersecting(&rstar::AABB::from_corners(corners.0, corners.1))
            .map(|entry| entry.data)
            .collect();
        hits.sort_unstable();
        hits.into_iter()
            .map(|index| {
                let item = &level.items[index];
                if item.count == 0 {
                    return Clustered::Point(item.id, item.position);
                }
                let mut properties = vec![
                    ("cluster".to_owned(), json!(true)),
                    ("cluster_id".to_owned(), json!(item.id)),
                    ("point_count".to_owned(), json!(item.count)),
                    (
                        "point_count_abbreviated".to_owned(),
                        abbreviated(item.count),
                    ),
                ];
                for (property, value) in self.options.properties.iter().zip(&item.values) {
                    properties.push((property.name.clone(), value.clone()));
                }
                Clustered::Cluster(item.position, item.id as u64, properties)
            })
            .collect()
    }
}

fn abbreviated(count: usize) -> Value {
    if count >= 10_000 {
        json!(format!("{}k", (count as f64 / 1000.0).round()))
    } else if count >= 1000 {
        json!(format!("{}k", (count as f64 / 100.0).round() / 10.0))
    } else {
        json!(count)
    }
}

/// Longitude and latitude in degrees of a world position where the world is `0..1`.
pub fn unproject(position: [f64; 2]) -> [f64; 2] {
    let longitude = (position[0] - 0.5) * 360.0;
    let y = (180.0 - position[1] * 360.0).to_radians();
    [
        longitude,
        360.0 * y.exp().atan() / std::f64::consts::PI - 90.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> ClusterOptions {
        ClusterOptions::new(Some(40.0), Some(4), None, None, None)
    }

    #[test]
    fn points_within_the_radius_merge_at_low_zoom_and_part_at_high_zoom() {
        let properties: [(String, Value); 0] = [];
        let points = [
            ([0.5, 0.5], &properties[..]),
            ([0.5 + 0.0001, 0.5], &properties[..]),
            ([0.9, 0.9], &properties[..]),
        ];
        let clusters = Clusters::new(&points, options());
        let world = [0.0, 0.0, 1.0, 1.0];
        let low = clusters.within(0, world);
        assert_eq!(low.len(), 2, "the two close points merge");
        assert!(low
            .iter()
            .any(|item| matches!(item, Clustered::Cluster(_, _, p)
            if p.iter().any(|(name, value)| name == "point_count" && *value == json!(2)))));
        assert_eq!(
            clusters.within(5, world).len(),
            3,
            "past the cluster zoom nothing merges"
        );
    }

    #[test]
    fn counts_of_a_thousand_or_more_abbreviate() {
        assert_eq!(abbreviated(999), json!(999));
        assert_eq!(abbreviated(1234), json!("1.2k"));
        assert_eq!(abbreviated(12_400), json!("12k"));
    }
}
