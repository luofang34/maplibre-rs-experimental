use super::{EmptyNode, RenderGraph, RenderGraphError};

#[test]
fn unconstrained_passes_run_in_the_order_they_were_added() {
    let mut graph = RenderGraph::default();
    for name in ["drape", "density", "main", "debug", "copy", "depth_copy"] {
        graph.add_node(name, EmptyNode).expect("unique pass");
    }
    graph.add_node_edge("drape", "main").expect("passes exist");
    graph
        .add_node_edge("density", "main")
        .expect("passes exist");
    for after in ["debug", "copy", "depth_copy"] {
        graph.add_node_edge("main", after).expect("passes exist");
    }

    assert_eq!(
        graph.node_order().expect("acyclic"),
        ["drape", "density", "main", "debug", "copy", "depth_copy"]
    );
}

#[test]
fn constraints_override_insertion_order() {
    let mut graph = RenderGraph::default();
    graph.add_node("late", EmptyNode).expect("unique pass");
    graph.add_node("early", EmptyNode).expect("unique pass");
    graph.add_node_edge("early", "late").expect("passes exist");

    assert_eq!(graph.node_order().expect("acyclic"), ["early", "late"]);
}

#[test]
fn a_cycle_is_an_error_naming_its_passes() {
    let mut graph = RenderGraph::default();
    graph.add_node("free", EmptyNode).expect("unique pass");
    graph.add_node("a", EmptyNode).expect("unique pass");
    graph.add_node("b", EmptyNode).expect("unique pass");
    graph.add_node_edge("a", "b").expect("passes exist");
    graph.add_node_edge("b", "a").expect("passes exist");

    assert_eq!(
        graph.prepare(),
        Err(RenderGraphError::Cycle(vec!["a".into(), "b".into()]))
    );
}

#[test]
fn names_must_be_unique_and_edges_must_name_passes() {
    let mut graph = RenderGraph::default();
    graph.add_node("main", EmptyNode).expect("unique pass");

    assert_eq!(
        graph.add_node("main", EmptyNode),
        Err(RenderGraphError::DuplicateNode("main".into()))
    );
    assert_eq!(
        graph.add_node_edge("main", "missing"),
        Err(RenderGraphError::InvalidNode("missing".into()))
    );
}
