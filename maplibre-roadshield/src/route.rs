//! The image names that ask for a route's shield.

use roadshield::RouteDescriptor;

use crate::ShieldRenderError;

/// The image namespace [`crate::RoadShieldProvider`] serves: names start with `roadshield:`.
pub const NAMESPACE: &str = "roadshield";

/// Separates the route name from the rest of an image id. OpenStreetMap tags cannot hold
/// control characters, so no network, ref or name contains it.
const NAME_SEPARATOR: char = '\u{1f}';

/// One route, as an image id: `network=ref`, the form of OpenMapTiles' `route_N` attributes,
/// optionally followed by U+001F and the route's name.
///
/// The network may be empty when the data does not say which network the route belongs to;
/// the shield is then roadshield's generic one, and no jurisdiction is guessed. Banners and
/// variants are part of the network, as in `US:US:Truck:Bypass`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteRequest {
    /// The route network, such as `US:I` or `US:NJ:CR`; empty when unknown.
    pub network: String,
    /// The route number or code.
    pub reference: String,
    /// The route's name, which some rules draw instead of a number.
    pub name: Option<String>,
}

impl RouteRequest {
    /// Reads an image id, the part of the name after `roadshield:`.
    pub fn parse(id: &str) -> Result<Self, ShieldRenderError> {
        let invalid = |reason| ShieldRenderError::Request {
            name: id.to_owned(),
            reason,
        };
        let (route, name) = match id.split_once(NAME_SEPARATOR) {
            Some((route, name)) => (route, Some(name)),
            None => (id, None),
        };
        let (network, reference) = route
            .split_once('=')
            .ok_or_else(|| invalid("it has no `=` between network and ref"))?;
        let name = name.filter(|name| !name.is_empty()).map(str::to_owned);
        if reference.is_empty() && name.is_none() {
            return Err(invalid("it has neither a ref nor a name"));
        }
        Ok(Self {
            network: network.to_owned(),
            reference: reference.to_owned(),
            name,
        })
    }

    /// The image id of the request, which [`Self::parse`] reads back.
    pub fn id(&self) -> String {
        let mut id = format!("{}={}", self.network, self.reference);
        if let Some(name) = &self.name {
            id.push(NAME_SEPARATOR);
            id.push_str(name);
        }
        id
    }

    /// The whole image name, namespace included.
    pub fn image_name(&self) -> String {
        format!("{NAMESPACE}:{}", self.id())
    }

    /// The route as roadshield sees it. A route of unknown network is given a network no
    /// rule names, so roadshield draws its generic shield rather than none.
    pub(crate) fn descriptor(&self) -> RouteDescriptor {
        let network = if self.network.is_empty() {
            UNKNOWN_NETWORK.to_owned()
        } else {
            self.network.clone()
        };
        let mut route = RouteDescriptor::new(network, self.reference.clone());
        if self.reference.is_empty() {
            route.ref_ = None;
        }
        route.name.clone_from(&self.name);
        route.source = Some(self.image_name());
        route
    }
}

/// The network a route of unknown network is drawn under, which no Americana rule names.
const UNKNOWN_NETWORK: &str = "unknown";

#[cfg(test)]
mod tests;
