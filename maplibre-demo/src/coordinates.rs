//! Geographic coordinates accepted by the headless command.
use maplibre::coords::LatLon;

pub(super) fn parse_lat_long(input: &str) -> Result<LatLon, String> {
    let (latitude, longitude) = input
        .split_once(',')
        .ok_or_else(|| format!("expected latitude,longitude, got {input:?}"))?;
    let parse = |value: &str, name: &str| {
        let number = value
            .trim()
            .parse::<f64>()
            .map_err(|error| format!("invalid {name} {value:?}: {error}"))?;
        if !number.is_finite() {
            return Err(format!("{name} must be finite, got {value:?}"));
        }
        Ok(number)
    };
    Ok(LatLon::new(
        parse(latitude, "latitude")?,
        parse(longitude, "longitude")?,
    ))
}

#[cfg(test)]
mod tests;
