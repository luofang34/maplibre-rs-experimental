//! The credits a map owes for what it draws, as GL JS's attribution control collects them.

use super::{source::Source, Style};

impl Style {
    /// The attributions of the sources the style draws at `zoom`: those of visible layers in
    /// painter order, then the terrain's. Repeated credits, blank ones and any contained in a
    /// longer one are left out, as GL JS's attribution control leaves them out.
    pub fn attributions(&self, zoom: f64) -> Vec<String> {
        let drawn = self
            .layers
            .iter()
            .filter(|layer| layer.is_visible_at(zoom))
            .filter_map(|layer| layer.source.as_deref())
            .chain(self.terrain.as_ref().map(|terrain| terrain.source.as_str()));
        let mut credits: Vec<String> = Vec::new();
        for name in drawn {
            let Some(credit) = self.sources.get(name).and_then(attribution) else {
                continue;
            };
            let credit = credit.trim();
            if !credit.is_empty() && !credits.iter().any(|kept| kept == credit) {
                credits.push(credit.to_owned());
            }
        }
        credits
            .iter()
            .filter(|credit| {
                !credits
                    .iter()
                    .any(|other| other != *credit && other.contains(credit.as_str()))
            })
            .cloned()
            .collect()
    }
}

fn attribution(source: &Source) -> Option<&str> {
    match source {
        Source::Vector(source) | Source::Raster(source) => source.attribution.as_deref(),
        Source::RasterDem(source) => source.attribution.as_deref(),
        Source::GeoJson(_) | Source::Image(_) => None,
    }
}

#[cfg(test)]
mod tests;
