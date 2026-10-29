//! The dependency view of the plan (PRD §15: "later: the task dependency
//! graph").
//!
//! The tree is plain text: every open task with its state, the children
//! indented under the task they depend on, and a task that waits for
//! something that is not done marked `waits T-x`.
//!
//! A task is drawn under the first dependency that is still open. A task
//! whose dependencies are all done is a root of the tree. Every open task
//! is drawn exactly once, even when the plan has a cycle in it.

use std::collections::BTreeMap;

use gear_model::TaskState;
use gear_store::TaskRow;

/// One task of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepNode {
    /// The task id, `T-3`.
    pub id: String,
    /// The state of the task.
    pub state: TaskState,
    /// The dependencies that are not done: the task waits for them.
    pub waits: Vec<String>,
    /// How deep the node is drawn; a root is 0.
    pub depth: usize,
}

/// The open tasks as a dependency tree, in the order the board draws them.
///
/// Open means: in the plan (not a draft) and not finished (not done, not
/// failed, not cancelled).
pub fn tree(tasks: &[TaskRow]) -> Vec<DepNode> {
    let open: Vec<&TaskRow> = tasks
        .iter()
        .filter(|task| task.state != TaskState::Draft && !task.state.is_terminal())
        .collect();
    let done: Vec<&str> = tasks
        .iter()
        .filter(|task| task.state == TaskState::Done)
        .map(|task| task.id.as_str())
        .collect();

    // The parent of a task: the first dependency that is still open. A
    // dependency that is done is not drawn, so it cannot be a parent.
    let mut children: BTreeMap<&str, Vec<&TaskRow>> = BTreeMap::new();
    let mut roots: Vec<&TaskRow> = Vec::new();
    for task in &open {
        let parent = task
            .deps
            .iter()
            .find(|dep| open.iter().any(|candidate| candidate.id == **dep));
        match parent {
            Some(parent) => children.entry(parent.as_str()).or_default().push(task),
            None => roots.push(task),
        }
    }

    let mut nodes: Vec<DepNode> = Vec::new();
    let mut drawn: Vec<String> = Vec::new();
    for root in &roots {
        walk(root, 0, &children, &done, &mut drawn, &mut nodes);
    }
    // A cycle in the plan leaves a task with no root above it: draw it as
    // one, so that no open task is missing from the tree.
    for task in &open {
        if !drawn.contains(&task.id) {
            walk(task, 0, &children, &done, &mut drawn, &mut nodes);
        }
    }
    nodes
}

/// Draw a task and everything below it.
fn walk(
    task: &TaskRow,
    depth: usize,
    children: &BTreeMap<&str, Vec<&TaskRow>>,
    done: &[&str],
    drawn: &mut Vec<String>,
    nodes: &mut Vec<DepNode>,
) {
    if drawn.contains(&task.id) {
        return; // a cycle: the task is drawn once
    }
    drawn.push(task.id.clone());
    nodes.push(DepNode {
        id: task.id.clone(),
        state: task.state,
        waits: task
            .deps
            .iter()
            .filter(|dep| !done.contains(&dep.as_str()))
            .cloned()
            .collect(),
        depth,
    });
    if let Some(kids) = children.get(task.id.as_str()) {
        for kid in kids {
            walk(kid, depth + 1, children, done, drawn, nodes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A task with the given id, state and dependencies.
    fn task(id: &str, state: TaskState, deps: &[&str]) -> TaskRow {
        TaskRow {
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
            project: "p".into(),
            id: id.into(),
            rank: 0,
            state,
            priority: "must".into(),
            lane: Some("data".into()),
            milestone: None,
            required: false,
            contract_version: 1,
            deps: deps.iter().map(|dep| dep.to_string()).collect(),
            followup_of: None,
            wait_on: None,
            wait_cond: None,
            rework_note: None,
            estimate_min: None,
            issue: None,
            updated: 0,
        }
    }

    #[test]
    fn a_chain_is_a_chain() {
        let tasks = vec![
            task("T-1", TaskState::Done, &[]),
            task("T-2", TaskState::Ready, &["T-1"]),
        ];
        let nodes = tree(&tasks);

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "T-2");
        assert_eq!(nodes[0].depth, 0, "its dependency is done, so it is a root");
        assert!(nodes[0].waits.is_empty(), "nothing to wait for");
    }

    #[test]
    fn a_task_waits_for_what_is_not_done() {
        let tasks = vec![
            task("T-1", TaskState::Ready, &[]),
            task("T-2", TaskState::Ready, &["T-1"]),
        ];
        let nodes = tree(&tasks);

        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[1].id, "T-2");
        assert_eq!(nodes[1].depth, 1, "it is drawn under T-1");
        assert_eq!(nodes[1].waits, vec!["T-1".to_string()]);
    }

    #[test]
    fn a_cycle_is_drawn_once() {
        let tasks = vec![
            task("T-1", TaskState::Ready, &["T-2"]),
            task("T-2", TaskState::Ready, &["T-1"]),
        ];
        let nodes = tree(&tasks);

        assert_eq!(nodes.len(), 2, "both tasks are drawn, none twice");
    }
}
