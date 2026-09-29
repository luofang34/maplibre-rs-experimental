//! Cancellation releases GPU work while returning the map's services to its host.
use super::dispatch::Dispatch;
use maplibre::{
    environment::Environment,
    map::MapError,
    window::{HeadedMapWindow, MapWindowConfig},
};
use std::{cell::RefCell, rc::Rc};

pub(super) struct Initialization<E: Environment>
where
    <E::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    dispatch: Option<Dispatch<E>>,
    recovery: Rc<RefCell<Option<Dispatch<E>>>>,
}
impl<E: Environment> Initialization<E>
where
    <E::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    pub fn new(dispatch: Dispatch<E>, recovery: Rc<RefCell<Option<Dispatch<E>>>>) -> Self {
        Self {
            dispatch: Some(dispatch),
            recovery,
        }
    }
    pub async fn run(mut self) -> Result<Dispatch<E>, MapError> {
        let dispatch = self.dispatch.as_mut().ok_or(MapError::RendererNotReady)?;
        dispatch.map.initialize_renderer().await?;
        self.dispatch.take().ok_or(MapError::RendererNotReady)
    }
}
impl<E: Environment> Drop for Initialization<E>
where
    <E::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    fn drop(&mut self) {
        if let Some(mut dispatch) = self.dispatch.take() {
            dispatch.map.reset();
            *self.recovery.borrow_mut() = Some(dispatch);
        }
    }
}
