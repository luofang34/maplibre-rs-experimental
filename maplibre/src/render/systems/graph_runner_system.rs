//! Records, submits and presents the frame.

use std::borrow::Cow;

use crate::{
    context::MapContext,
    render::{
        error::RenderError, eventually::Eventually::Initialized, graph_runner::run_graph, Renderer,
    },
    tcs::system::{System, SystemError, SystemResult},
};

/// Records the frame's render passes, submits them and presents the frame.
#[derive(Default)]
pub struct GraphRunnerSystem;

impl System for GraphRunnerSystem {
    fn name(&self) -> Cow<'static, str> {
        "graph_runner".into()
    }

    fn run(
        &mut self,
        MapContext {
            world,
            renderer:
                Renderer {
                    device,
                    queue,
                    resources: state,
                    render_graph,
                    ..
                },
            ..
        }: &mut MapContext,
    ) -> SystemResult {
        render_graph.prepare().map_err(RenderError::Graph)?;
        run_graph(render_graph, device, queue, state, world).map_err(RenderError::Graph)?;

        {
            let _span = tracing::info_span!("present_frames").entered();

            let Initialized(render_target) = state.render_target.take() else {
                return Err(SystemError::Dependencies);
            };

            if let Some(surface_texture) = render_target.take_surface_texture() {
                queue.present(surface_texture);
            }

            #[cfg(feature = "tracing-tracy")]
            tracing::event!(
                tracing::Level::INFO,
                message = "finished frame",
                tracy.frame_mark = true
            );

            Ok(())
        }
    }
}
