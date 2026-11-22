//! Source-level diffing helpers for [`super::Style::diff_and_replace`].

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use super::{
    canvas_definition_eq, geojson_definition_eq, image_definition_eq, is_raster_like,
    video_definition_eq, StyleReplacementDiff,
};
use crate::style::{
    mutation::classify::push_unique,
    source::{fresh_generation, Source},
};

pub(super) fn diff_sources(
    old_sources: &HashMap<String, Source>,
    next_sources: &mut HashMap<String, Source>,
    diff: &mut StyleReplacementDiff,
) -> HashSet<String> {
    let mut changed_vector_sources = HashSet::default();
    for (name, old_source) in old_sources {
        if !next_sources.contains_key(name) {
            diff.removed_sources.push(name.clone());
            if is_raster_like(old_source) {
                diff.removed_raster_sources.push(name.clone());
            }
        }
    }
    for (name, new_source) in next_sources {
        let Some(old_source) = old_sources.get(name) else {
            mark_added_source(name, new_source, diff);
            continue;
        };
        diff_retained_source(
            name,
            old_source,
            new_source,
            diff,
            &mut changed_vector_sources,
        );
    }
    diff.added_sources.sort();
    diff.removed_sources.sort();
    diff.updated_sources.sort();
    diff.removed_raster_sources.sort();
    diff.reloaded_image_sources.sort();
    changed_vector_sources
}

fn mark_added_source(name: &str, new_source: &mut Source, diff: &mut StyleReplacementDiff) {
    match new_source {
        Source::GeoJson(g) => g.generation = fresh_generation(),
        Source::Image(i) => i.generation = fresh_generation(),
        Source::Video(v) => v.generation = fresh_generation(),
        Source::Canvas(c) => c.generation = fresh_generation(),
        Source::Raster(_) | Source::RasterDem(_) => diff.redraw_raster_tiles = true,
        Source::Vector(_) => {}
    }
    diff.added_sources.push(name.to_owned());
}

fn diff_retained_source(
    name: &str,
    old_source: &Source,
    new_source: &mut Source,
    diff: &mut StyleReplacementDiff,
    changed_vector_sources: &mut HashSet<String>,
) {
    match (old_source, new_source) {
        (Source::Vector(a), Source::Vector(b)) => {
            if a != b {
                diff.updated_sources.push(name.to_owned());
                changed_vector_sources.insert(name.to_owned());
            }
        }
        (Source::Raster(a), Source::Raster(b)) => {
            if a != b {
                diff.updated_sources.push(name.to_owned());
                push_unique(&mut diff.removed_raster_sources, name);
                diff.redraw_raster_tiles = true;
            }
        }
        (Source::RasterDem(a), Source::RasterDem(b)) => {
            if a != b {
                diff.updated_sources.push(name.to_owned());
                push_unique(&mut diff.removed_raster_sources, name);
                diff.redraw_raster_tiles = true;
            }
        }
        (Source::GeoJson(a), Source::GeoJson(b)) => {
            if geojson_definition_eq(a, b) {
                b.generation = a.generation;
            } else {
                b.generation = fresh_generation();
                diff.updated_sources.push(name.to_owned());
            }
        }
        (Source::Image(a), Source::Image(b)) => {
            if image_definition_eq(a, b) {
                b.generation = a.generation;
            } else {
                b.generation = fresh_generation();
                diff.updated_sources.push(name.to_owned());
                push_unique(&mut diff.reloaded_image_sources, name);
            }
        }
        (Source::Video(a), Source::Video(b)) => diff_media_source(
            MediaFrameDiff {
                name,
                same_definition: video_definition_eq(a, b),
                same_media: a.urls == b.urls,
                old_generation: a.generation,
                old_frame: &a.frame,
                new_generation: &mut b.generation,
                new_frame: &mut b.frame,
            },
            diff,
        ),
        (Source::Canvas(a), Source::Canvas(b)) => diff_media_source(
            MediaFrameDiff {
                name,
                same_definition: canvas_definition_eq(a, b),
                same_media: a.canvas == b.canvas,
                old_generation: a.generation,
                old_frame: &a.frame,
                new_generation: &mut b.generation,
                new_frame: &mut b.frame,
            },
            diff,
        ),
        (old_other, new_other) => {
            diff_replaced_source_kind(name, old_other, new_other, diff, changed_vector_sources);
        }
    }
}

struct MediaFrameDiff<'a, T> {
    name: &'a str,
    same_definition: bool,
    same_media: bool,
    old_generation: u64,
    old_frame: &'a Option<T>,
    new_generation: &'a mut u64,
    new_frame: &'a mut Option<T>,
}

fn diff_media_source<T: Clone>(entry: MediaFrameDiff<'_, T>, diff: &mut StyleReplacementDiff) {
    if entry.same_definition {
        if entry.new_frame.is_none() {
            *entry.new_frame = entry.old_frame.clone();
        }
        *entry.new_generation = entry.old_generation;
    } else {
        if entry.new_frame.is_none() && entry.same_media {
            *entry.new_frame = entry.old_frame.clone();
        }
        *entry.new_generation = fresh_generation();
        diff.updated_sources.push(entry.name.to_owned());
        push_unique(&mut diff.reloaded_image_sources, entry.name);
    }
}

fn diff_replaced_source_kind(
    name: &str,
    old_other: &Source,
    new_other: &mut Source,
    diff: &mut StyleReplacementDiff,
    changed_vector_sources: &mut HashSet<String>,
) {
    diff.updated_sources.push(name.to_owned());
    if is_raster_like(old_other) {
        push_unique(&mut diff.removed_raster_sources, name);
    }
    match new_other {
        Source::GeoJson(g) => g.generation = fresh_generation(),
        Source::Image(i) => {
            i.generation = fresh_generation();
            push_unique(&mut diff.reloaded_image_sources, name);
        }
        Source::Video(v) => {
            v.generation = fresh_generation();
            push_unique(&mut diff.reloaded_image_sources, name);
        }
        Source::Canvas(c) => {
            c.generation = fresh_generation();
            push_unique(&mut diff.reloaded_image_sources, name);
        }
        Source::Raster(_) | Source::RasterDem(_) => diff.redraw_raster_tiles = true,
        Source::Vector(_) => {
            changed_vector_sources.insert(name.to_owned());
        }
    }
}
