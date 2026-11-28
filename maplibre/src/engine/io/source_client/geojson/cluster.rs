//! Groups the points of a GeoJSON source into clusters for each zoom, as GL JS does with
//! supercluster: points within a pixel radius of one another merge, level by level from the
//! deepest zoom up, into a cluster at their weighted centre.

use rstar::{primitives::GeomWithData, RTree};
use serde_json::{json, Value};

use crate::style::expression::{EvaluationContext, Expression, FeatureProperties, Value as Datum};

/// Side of the tile the radius is measured on, in pixels.
const TILE_PIXELS: f64 = 512.0;

/// How a cluster property combines the values of the points and sub-clusters it holds.
#[derive(Clone, Debug)]
enum Reduce {
    Sum,
    Product,
    Max,
    Min,
    Any,
    All,
    Expression(Expression),
}

impl Reduce {
    fn parse_builtin(operator: &str) -> Option<Self> {
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

    fn combine(&self, left: &Datum, right: &Datum, context: &EvaluationContext<'_>) -> Datum {
        let numbers = || Some((left.as_number()?, right.as_number()?));
        let flags = || {
            (
                left.as_bool().unwrap_or(false),
                right.as_bool().unwrap_or(false),
            )
        };
        match self {
            Self::Sum => numbers().map_or(Datum::Null, |(a, b)| Datum::Number(a + b)),
            Self::Product => numbers().map_or(Datum::Null, |(a, b)| Datum::Number(a * b)),
            Self::Max => numbers().map_or(Datum::Null, |(a, b)| Datum::Number(a.max(b))),
            Self::Min => numbers().map_or(Datum::Null, |(a, b)| Datum::Number(a.min(b))),
            Self::Any => Datum::Bool(flags().0 || flags().1),
            Self::All => Datum::Bool(flags().0 && flags().1),
            Self::Expression(expr) => expr.evaluate(context).unwrap_or(Datum::Null),
        }
    }
}

/// One cluster property: the expression mapping a point to its initial value and how values combine.
struct Property {
    name: String,
    map: Expression,
    source_key: Option<String>,
    reduce: Reduce,
}

fn direct_property_key(expr: &Expression) -> Option<String> {
    match expr {
        Expression::Get { key, object: None } => match key.as_ref() {
            Expression::Literal(Datum::String(name)) => Some(name.clone()),
            _ => None,
        },
        _ => None,
    }
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

    /// Parses `[operator, map]`, `[operator, ["accumulated"], map]`, or `[reduce_expr, map_expr]`.
    fn property(name: &str, spec: &Value) -> Option<Property> {
        let items = spec.as_array()?;
        let (reduce, map_json) = match items.as_slice() {
            [Value::String(op), map_json] => (Reduce::parse_builtin(op)?, map_json),
            [Value::String(op), acc, map_json]
                if acc
                    .as_array()
                    .is_some_and(|a| a.as_slice() == [json!("accumulated")]) =>
            {
                (Reduce::parse_builtin(op)?, map_json)
            }
            [reduce_json, map_json] => {
                let reduce_expr = crate::style::expression::parse(reduce_json).ok()?;
                (Reduce::Expression(reduce_expr), map_json)
            }
            _ => return None,
        };
        let map = crate::style::expression::parse(map_json).ok()?;
        let source_key = direct_property_key(&map);
        Some(Property {
            name: name.to_owned(),
            map,
            source_key,
            reduce,
        })
    }

    fn values(&self, properties: &[(String, Value)]) -> (FeatureProperties, Vec<Datum>) {
        let feature: FeatureProperties = properties
            .iter()
            .map(|(key, value)| (key.clone(), Datum::from_json(value)))
            .collect();
        let mapped = self
            .properties
            .iter()
            .map(|property| {
                property
                    .map
                    .evaluate(&EvaluationContext::for_feature(0.0, &feature))
                    .unwrap_or(Datum::Null)
            })
            .collect();
        (feature, mapped)
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
    /// Original feature properties when `count == 0`.
    point_props: Option<FeatureProperties>,
    /// Cluster property values, in the order of the options.
    values: Vec<Datum>,
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
        let has_custom_reduce = options
            .properties
            .iter()
            .any(|prop| matches!(prop.reduce, Reduce::Expression(_)));
        let mut items: Vec<Item> = points
            .iter()
            .enumerate()
            .map(|(index, (position, properties))| {
                let (feature, values) = options.values(properties);
                Item {
                    position: *position,
                    count: 0,
                    id: index,
                    point_props: has_custom_reduce.then_some(feature),
                    values,
                }
            })
            .collect();
        let max_zoom = usize::from(options.max_zoom);
        let mut levels: Vec<Level> = Vec::with_capacity(max_zoom + 2);
        levels.push(Level::new(items.clone()));
        for zoom in (0..=max_zoom).rev() {
            let Some(previous) = levels.last() else {
                break;
            };
            items = Self::merge(previous, zoom, &options);
            levels.push(Level::new(items.clone()));
        }
        levels.reverse();
        Self { options, levels }
    }

    fn merge(previous: &Level, zoom: usize, options: &ClusterOptions) -> Vec<Item> {
        let radius = options.radius / (TILE_PIXELS * 2.0_f64.powi(zoom as i32));
        let mut merged_at = vec![usize::MAX; previous.items.len()];
        let mut result = Vec::new();
        for (index, item) in previous.items.iter().enumerate() {
            let Some(slot) = merged_at.get_mut(index) else {
                continue;
            };
            if *slot <= zoom {
                continue;
            }
            *slot = zoom;
            let neighbours: Vec<usize> = previous
                .tree
                .locate_within_distance(item.position, radius * radius)
                .map(|entry| entry.data)
                .collect();
            let own = item.count.max(1);
            let total = own
                + neighbours
                    .iter()
                    .filter(|neighbour| merged_at.get(**neighbour).is_some_and(|&m| m > zoom))
                    .filter_map(|neighbour| previous.items.get(*neighbour))
                    .map(|other| other.count.max(1))
                    .sum::<usize>();
            if total > own && total >= options.min_points {
                result.push(Self::build_merged_cluster(
                    (index, item, own, total),
                    &neighbours,
                    &mut merged_at,
                    previous,
                    zoom,
                    options,
                ));
            } else {
                result.push(item.clone());
                if total > 1 {
                    for neighbour in neighbours {
                        if let Some(m) = merged_at.get_mut(neighbour) {
                            if *m > zoom {
                                *m = zoom;
                                if let Some(other) = previous.items.get(neighbour) {
                                    result.push(other.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        result
    }

    fn build_merged_cluster(
        (index, item, own, total): (usize, &Item, usize, usize),
        neighbours: &[usize],
        merged_at: &mut [usize],
        previous: &Level,
        zoom: usize,
        options: &ClusterOptions,
    ) -> Item {
        let mut weighted = [item.position[0] * own as f64, item.position[1] * own as f64];
        let mut values = item.values.clone();
        for &neighbour in neighbours {
            let Some(m) = merged_at.get_mut(neighbour) else {
                continue;
            };
            if *m <= zoom {
                continue;
            }
            *m = zoom;
            let Some(other) = previous.items.get(neighbour) else {
                continue;
            };
            let weight = other.count.max(1) as f64;
            weighted[0] += other.position[0] * weight;
            weighted[1] += other.position[1] * weight;
            Self::combine_properties(&mut values, other, zoom, options);
        }
        Item {
            position: [weighted[0] / total as f64, weighted[1] / total as f64],
            count: total,
            id: (index << 5) + zoom + 1 + previous.items.len(),
            point_props: None,
            values,
        }
    }

    fn combine_properties(
        values: &mut [Datum],
        other: &Item,
        zoom: usize,
        options: &ClusterOptions,
    ) {
        for (prop_idx, property) in options.properties.iter().enumerate() {
            let (Some(acc), Some(other_val)) =
                (values.get_mut(prop_idx), other.values.get(prop_idx))
            else {
                continue;
            };
            *acc = if matches!(property.reduce, Reduce::Expression(_)) {
                let mut props = other.point_props.clone().unwrap_or_default();
                for (other_prop, val) in options.properties.iter().zip(&other.values) {
                    props.insert(other_prop.name.clone(), val.clone());
                }
                if let Some(src_key) = &property.source_key {
                    props
                        .entry(src_key.clone())
                        .or_insert_with(|| other_val.clone());
                }
                props.insert(property.name.clone(), other_val.clone());
                let ctx = EvaluationContext {
                    zoom: zoom as f64,
                    properties: Some(&props),
                    accumulated: Some(acc),
                    ..EvaluationContext::default()
                };
                property.reduce.combine(acc, other_val, &ctx)
            } else {
                property
                    .reduce
                    .combine(acc, other_val, &EvaluationContext::default())
            };
        }
    }

    /// The points and clusters inside `window` (`[west, north, east, south]` in world units)
    /// at a tile zoom, in the order they were made.
    pub fn within(&self, zoom: u8, window: [f64; 4]) -> Vec<Clustered> {
        let Some(level) = self
            .levels
            .get(usize::from(zoom).min(self.levels.len().saturating_sub(1)))
        else {
            return Vec::new();
        };
        let corners = ([window[0], window[1]], [window[2], window[3]]);
        let mut hits: Vec<usize> = level
            .tree
            .locate_in_envelope_intersecting(&rstar::AABB::from_corners(corners.0, corners.1))
            .map(|entry| entry.data)
            .collect();
        hits.sort_unstable();
        hits.into_iter()
            .filter_map(|index| {
                let item = level.items.get(index)?;
                if item.count == 0 {
                    return Some(Clustered::Point(item.id, item.position));
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
                    properties.push((property.name.clone(), value.to_json()));
                }
                Some(Clustered::Cluster(
                    item.position,
                    item.id as u64,
                    properties,
                ))
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
mod tests;
