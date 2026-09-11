//! Symbol visibility configuration for headless and externally driven maps.
use super::HeadlessMap;
impl HeadlessMap {
    /// Sets optional distance-based symbol relevance for externally driven views.
    pub fn set_symbol_visibility(&mut self, policy: crate::sdf::visibility::SymbolVisibility) {
        self.map_context.world.resources.insert(policy);
    }
}
