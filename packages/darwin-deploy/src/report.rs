//! The structured deployment report: one JSON document on stdout under
//! `--json`, a per-node summary otherwise. Fields are always present so
//! consumers get a stable schema.

use serde::Serialize;

use crate::node::NodeSpec;

#[derive(Serialize)]
pub struct Report {
    pub dry_run: bool,
    pub ok: bool,
    pub nodes: Vec<NodeReport>,
}

impl Report {
    pub fn new(nodes: Vec<NodeReport>, dry_run: bool) -> Self {
        let ok = nodes.iter().all(|node| node.ok);
        Self { dry_run, ok, nodes }
    }

    pub fn print_human(&self) {
        for node in &self.nodes {
            println!("{}: {}", node.name, node.status(self.dry_run));
        }
    }
}

#[derive(Serialize)]
pub struct NodeReport {
    pub name: String,
    pub target: String,
    /// The freshly built system closure, once the build succeeded.
    pub system: Option<String>,
    /// What `/run/current-system` pointed at before this deploy, if anything.
    pub previous: Option<String>,
    pub changed: Option<bool>,
    pub ok: bool,
    pub error: Option<String>,
}

impl NodeReport {
    pub fn new(spec: &NodeSpec) -> Self {
        Self {
            name: spec.name.clone(),
            target: spec.target.ssh_destination(),
            system: None,
            previous: None,
            changed: None,
            ok: false,
            error: None,
        }
    }

    fn status(&self, dry_run: bool) -> String {
        if let Some(error) = &self.error {
            return format!("FAILED: {error}");
        }
        let system = self.system.as_deref().unwrap_or("<unknown>");
        match (dry_run, self.changed) {
            (true, Some(true)) => format!("would activate {system}"),
            (true, _) => format!("up to date at {system}"),
            (false, _) => format!("activated {system}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> NodeSpec {
        "mac1=admin@mac1.local".parse().expect("valid spec")
    }

    #[test]
    fn serializes_a_stable_schema() {
        let mut node = NodeReport::new(&spec());
        node.system = Some("/nix/store/new".to_owned());
        node.previous = Some("/nix/store/old".to_owned());
        node.changed = Some(true);
        node.ok = true;
        let report = Report::new(vec![node], true);

        let value = serde_json::to_value(&report).expect("serializes");
        assert_eq!(
            value,
            serde_json::json!({
                "dry_run": true,
                "ok": true,
                "nodes": [{
                    "name": "mac1",
                    "target": "admin@mac1.local",
                    "system": "/nix/store/new",
                    "previous": "/nix/store/old",
                    "changed": true,
                    "ok": true,
                    "error": null,
                }],
            })
        );
    }

    #[test]
    fn a_failed_node_fails_the_report() {
        let mut node = NodeReport::new(&spec());
        node.error = Some("ssh exploded".to_owned());
        let report = Report::new(vec![node], false);
        assert!(!report.ok);
        assert_eq!(report.nodes[0].status(false), "FAILED: ssh exploded");
    }
}
