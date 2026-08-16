//! Turn the metrics into the two shapes a reader wants: a short human summary
//! ranked by what is worth fixing, and the same facts as JSON for a gate.

use std::fmt;

use serde::Serialize;

use crate::graph::Metrics;
use crate::plan::Plan;

/// A node worth naming, with the numbers behind it and the sentence that says
/// why it is on the list.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub drv_path: String,
    /// Direct dependents that reach this node only through a carrier variable
    /// and no other way. The headline: these rebuilds are pure loss.
    pub sole_carrier_fan_out: u32,
    /// Direct dependents that reach it through a carrier variable at all.
    pub carrier_fan_out: u32,
    pub fan_out: u32,
    pub blast_radius: u32,
    pub own_closure: u32,
    pub carrier_key: Option<String>,
    pub why: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shape {
    pub derivations: usize,
    pub edges: usize,
    /// Longest chain in nodes: the floor on wall clock at any width.
    pub critical_path_nodes: usize,
    pub critical_path: Vec<String>,
    pub levels: usize,
    pub widest_level: usize,
    pub widest_level_nodes: u32,
    pub median_level_width: u32,
    /// Levels no wider than two nodes: the stretches more builders cannot help.
    pub serial_levels: usize,
    /// Derivations per level, indexed by depth. The whole parallelism curve,
    /// so a caller can chart it or gate on a shape the summary flattens: ix's
    /// CI plan spends 124 of its 318 levels carrying 328 derivations and then
    /// puts 8,408 into three.
    pub level_widths: Vec<u32>,
    /// Derivations whose outputs are content-addressed, so their paths are not
    /// known before the build and nothing can be attributed to them.
    pub unresolved_outputs: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub target: String,
    pub shape: Shape,
    pub top: Vec<Entry>,
}

impl Report {
    pub fn build(target: &str, plan: &Plan, metrics: &Metrics, top: usize) -> Self {
        let mut ranked: Vec<usize> = (0..plan.len()).collect();
        ranked.sort_unstable_by_key(|&id| {
            (
                std::cmp::Reverse(metrics.carrier[id].sole),
                std::cmp::Reverse(metrics.carrier[id].carried),
                std::cmp::Reverse(metrics.fan_out[id]),
                plan.nodes[id].name.clone(),
            )
        });

        let entries = ranked
            .into_iter()
            .take(top)
            .map(|id| {
                let carrier_key =
                    metrics.top_carrier_key[id].map(|top| plan.env_keys[top.key].clone());
                Entry {
                    name: plan.nodes[id].name.clone(),
                    drv_path: plan.nodes[id].drv_path.clone(),
                    sole_carrier_fan_out: metrics.carrier[id].sole,
                    carrier_fan_out: metrics.carrier[id].carried,
                    fan_out: metrics.fan_out[id],
                    blast_radius: metrics.blast[id],
                    own_closure: metrics.own_closure[id],
                    why: why(plan, metrics, id, carrier_key.as_deref()),
                    carrier_key,
                }
            })
            .collect();

        let mut sorted_widths = metrics.widths.clone();
        sorted_widths.sort_unstable();
        let widest_level = metrics
            .widths
            .iter()
            .enumerate()
            .max_by_key(|&(_, &width)| width)
            .map_or(0, |(level, _)| level);

        Self {
            target: target.to_owned(),
            shape: Shape {
                derivations: plan.len(),
                edges: plan.edges(),
                critical_path_nodes: metrics.critical_path.len(),
                critical_path: metrics
                    .critical_path
                    .iter()
                    .map(|&id| plan.nodes[id].name.clone())
                    .collect(),
                levels: metrics.widths.len(),
                widest_level,
                widest_level_nodes: metrics.widths.get(widest_level).copied().unwrap_or(0),
                median_level_width: sorted_widths
                    .get(sorted_widths.len() / 2)
                    .copied()
                    .unwrap_or(0),
                serial_levels: metrics.widths.iter().filter(|&&width| width <= 2).count(),
                level_widths: metrics.widths.clone(),
                unresolved_outputs: plan.unresolved_outputs,
            },
            top: entries,
        }
    }
}

/// The human summary. `Display` rather than a `-> String` builder so each line
/// is one `write!` into the caller's sink instead of a per-line allocation.
impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shape = &self.shape;
        writeln!(f, "{}", self.target)?;
        writeln!(
            f,
            "  {} derivations, {} edges\n",
            shape.derivations, shape.edges
        )?;

        writeln!(f, "Shape")?;
        writeln!(
            f,
            "  critical path  {} nodes; no width of builders finishes this plan in fewer steps",
            shape.critical_path_nodes
        )?;
        if !shape.critical_path.is_empty() {
            writeln!(f, "                 {}", elide_chain(&shape.critical_path))?;
        }
        writeln!(
            f,
            "  parallelism    {} levels, widest {} at level {}, median {}",
            shape.levels, shape.widest_level_nodes, shape.widest_level, shape.median_level_width
        )?;
        writeln!(
            f,
            "                 {} levels are 2 nodes or narrower; more builders do nothing there",
            shape.serial_levels
        )?;
        if shape.unresolved_outputs > 0 {
            writeln!(
                f,
                "  note           {} derivations are content-addressed, so their output paths are\n\
                 \x20                unknown before the build and nothing is attributed to them",
                shape.unresolved_outputs
            )?;
        }

        let flagged = self
            .top
            .iter()
            .filter(|entry| entry.sole_carrier_fan_out > 0)
            .count();
        writeln!(f)?;
        if flagged == 0 {
            f.write_str(
                "No derivation is reached only through an environment variable. Ranked by\n\
                 fan-out instead; these are hubs, which is normal unless one is trivial.\n\n",
            )?;
        } else {
            writeln!(
                f,
                "Top {} by avoidable invalidation: dependents that reach a node ONLY because an\n\
                 environment variable names it, and would stop rebuilding if it did not.\n\n\
                 This is cost per change, not cost. Multiply it by how often the node actually\n\
                 moves, which a single plan cannot show. A node built from this repo is the\n\
                 expensive case; one pinned to an upstream input that already invalidates the\n\
                 graph another way is free however high it ranks here.\n",
                self.top.len()
            )?;
        }

        for (rank, entry) in self.top.iter().enumerate() {
            writeln!(
                f,
                "{:>3}. {}\n     sole {} of {} direct  blast {}  own deps {}\n     {}",
                rank + 1,
                entry.name,
                entry.sole_carrier_fan_out,
                entry.fan_out,
                entry.blast_radius,
                entry.own_closure,
                entry.why
            )?;
        }
        Ok(())
    }
}

/// The sentence a reader argues with. Every claim in it is one of the numbers on
/// the line above, so disagreeing means disagreeing with a count.
fn why(plan: &Plan, metrics: &Metrics, id: usize, carrier_key: Option<&str>) -> String {
    let counts = metrics.carrier[id];
    let fan_out = metrics.fan_out[id];
    let key = carrier_key.unwrap_or("an environment variable");
    if counts.sole > 0 {
        return format!(
            "{} of {fan_out} dependents name it only in {key} and reach it no other way: \
             drop that and they stop rebuilding when this changes",
            counts.sole
        );
    }
    if counts.carried > 0 {
        return format!(
            "{} dependents carry it in {key}, but every one also reaches it structurally, \
             so removing the variable saves no rebuild",
            counts.carried
        );
    }
    if metrics.own_closure[id] <= 4 && fan_out >= 8 {
        return format!(
            "trivial node ({} own dependencies) that {fan_out} derivations depend on directly; \
             cheap to produce, expensive to change",
            metrics.own_closure[id]
        );
    }
    format!(
        "hub: {fan_out} direct and {} transitive dependents, all structural; expected for \
         something {} derivations deep",
        metrics.blast[id],
        plan.nodes[id].deps.len()
    )
}

/// Chains run to hundreds of nodes; show both ends and say how much was cut.
fn elide_chain(path: &[String]) -> String {
    const ENDS: usize = 3;
    if path.len() <= ENDS * 2 {
        return path.join(" <- ");
    }
    format!(
        "{} <- ... {} more ... <- {}",
        path[..ENDS].join(" <- "),
        path.len() - ENDS * 2,
        path[path.len() - ENDS..].join(" <- ")
    )
}

#[cfg(test)]
mod tests {
    use super::elide_chain;

    #[test]
    fn short_chains_are_not_elided() {
        let path: Vec<String> = ["a", "b", "c"].iter().map(ToString::to_string).collect();
        assert_eq!(elide_chain(&path), "a <- b <- c");
    }

    #[test]
    fn long_chains_report_what_was_cut() {
        let path: Vec<String> = (0..10).map(|n| n.to_string()).collect();
        assert_eq!(
            elide_chain(&path),
            "0 <- 1 <- 2 <- ... 4 more ... <- 7 <- 8 <- 9"
        );
    }
}
