//! The providers of images that labels name and no sprite supplies.

use super::Map;
use crate::{
    environment::Environment,
    io::apc::AsyncProcedureCall,
    sdf::assets::ImageProviders,
    window::{HeadedMapWindow, MapWindowConfig},
};

impl<E: Environment> Map<E>
where
    <<E as Environment>::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    /// The registry this map's tile workers ask for provided images, where they share the
    /// map's memory. Workers that build their own configuration, such as single-threaded web
    /// workers, register providers in that configuration instead.
    pub fn image_providers(&self) -> Option<ImageProviders> {
        self.kernel.apc().image_providers().cloned()
    }

    /// Forgets every image of `namespace` and requests again the tiles whose labels drew one,
    /// such as after its provider's resource pack changed. Returns how many tiles are
    /// requested again; none before the renderer is ready.
    pub fn invalidate_provided_images(&mut self, namespace: &str) -> usize {
        let kernel = self.kernel.clone();
        match self.context_mut() {
            Ok(context) => crate::sdf::provided::invalidate_namespace(
                kernel.apc().image_providers(),
                &mut context.world,
                namespace,
            ),
            // Before the renderer is ready no tile has drawn anything yet.
            Err(_) => {
                if let Some(providers) = kernel.apc().image_providers() {
                    providers.invalidate(namespace);
                }
                0
            }
        }
    }
}
