//! The providers of images that labels name and no sprite supplies.

use super::HeadlessMap;
use crate::{io::apc::AsyncProcedureCall, sdf::assets::ImageProviders};

impl HeadlessMap {
    /// The registry this map's tile workers ask for provided images.
    pub fn image_providers(&self) -> Option<ImageProviders> {
        self.kernel.apc().image_providers().cloned()
    }

    /// Forgets every image of `namespace` and requests again the tiles whose labels drew one,
    /// such as after its provider's resource pack changed. Returns how many tiles are
    /// requested again.
    pub fn invalidate_provided_images(&mut self, namespace: &str) -> usize {
        crate::sdf::provided::invalidate_namespace(
            self.kernel.apc().image_providers(),
            &mut self.map_context.world,
            namespace,
        )
    }
}
