//! Structural metrics over a build plan.
//!
//! Everything here is counted on the derivation graph alone, so the numbers hold
//! before anything is built. Reachability uses one bitset per node, which is
//! `nodes^2 / 8` bytes: 56 MiB for a 21k-derivation plan, and quadratic beyond
//! that, so [`Metrics::compute`] refuses plans past [`MAX_NODES`] rather than
//! swapping to death.

use color_eyre::eyre::{Result, bail};

use crate::plan::Plan;

/// Reachability needs `nodes^2` bits twice over. At this ceiling that is 1 GiB
/// per matrix, which is already past what a laptop should spend on a report.
pub const MAX_NODES: usize = 92_000;

/// How each direct dependent reaches the node under test.
#[derive(Clone, Copy, Default)]
pub struct CarrierCounts {
    /// Dependents whose every reference to this node sits in a carrier variable.
    pub carried: u32,
    /// Of those, the ones with no other path to this node at all, so dropping
    /// the variable would stop them rebuilding when it changes.
    pub sole: u32,
}

/// The carrier variable most of a node's carried dependents name it through.
#[derive(Clone, Copy)]
pub struct TopCarrier {
    /// Index into [`Plan::env_keys`].
    pub key: usize,
    /// Direct dependents naming the node through that key.
    pub count: u32,
}

/// Per-node structural numbers.
pub struct Metrics {
    /// Direct dependents.
    pub fan_out: Vec<u32>,
    /// Transitive dependents: what one change to this node invalidates.
    pub blast: Vec<u32>,
    /// Transitive dependencies: a proxy for what this node itself costs to
    /// produce, so a trivial node with a wide blast radius stands out from a
    /// compiler with the same reach.
    pub own_closure: Vec<u32>,
    pub carrier: Vec<CarrierCounts>,
    /// The carrier variable most dependents use to reach this node, with its
    /// count. `None` when nothing carries it.
    pub top_carrier_key: Vec<Option<TopCarrier>>,
    /// Nodes per level, indexed by depth. The parallelism actually available.
    pub widths: Vec<u32>,
    /// Longest chain, root first. Its length is the floor on wall clock: no
    /// number of builders makes the plan shorter than this many steps.
    pub critical_path: Vec<usize>,
}

/// A dense `nodes x nodes` bit matrix, one row per node.
struct Reachability {
    words: usize,
    data: Vec<u64>,
}

impl Reachability {
    fn new(nodes: usize) -> Self {
        let words = nodes.div_ceil(64);
        Self {
            words,
            data: vec![0; words * nodes],
        }
    }

    fn set(&mut self, row: usize, bit: usize) {
        self.data[row * self.words + bit / 64] |= 1 << (bit % 64);
    }

    fn get(&self, row: usize, bit: usize) -> bool {
        self.data[row * self.words + bit / 64] & (1 << (bit % 64)) != 0
    }

    /// `dst |= src`, for two distinct rows. Split rather than indexed so the
    /// inner loop is a bounds-check-free word-wise OR; this runs once per edge
    /// and the plans of interest have most of a million of them.
    fn union_into(&mut self, dst: usize, src: usize) {
        debug_assert_ne!(dst, src, "a derivation cannot depend on itself");
        let (dst_start, src_start) = (dst * self.words, src * self.words);
        let (dst_row, src_row) = if dst_start < src_start {
            let (left, right) = self.data.split_at_mut(src_start);
            (&mut left[dst_start..dst_start + self.words], &right[..self.words])
        } else {
            let (left, right) = self.data.split_at_mut(dst_start);
            (&mut right[..self.words], &left[src_start..src_start + self.words])
        };
        for (into, from) in dst_row.iter_mut().zip(src_row) {
            *into |= *from;
        }
    }

    fn count(&self, row: usize) -> u32 {
        self.data[row * self.words..(row + 1) * self.words]
            .iter()
            .map(|word| word.count_ones())
            .sum()
    }
}

/// Dependency order, dependencies before dependents. Errors on a cycle, which a
/// derivation graph cannot have: seeing one means the input is not a plan.
fn topological_order(plan: &Plan) -> Result<Vec<usize>> {
    let mut remaining: Vec<usize> = plan.nodes.iter().map(|node| node.deps.len()).collect();
    let mut queue: Vec<usize> = (0..plan.len()).filter(|&id| remaining[id] == 0).collect();
    let mut order = Vec::with_capacity(plan.len());
    while let Some(id) = queue.pop() {
        order.push(id);
        for &dependent in &plan.dependents[id] {
            remaining[dependent] -= 1;
            if remaining[dependent] == 0 {
                queue.push(dependent);
            }
        }
    }
    if order.len() != plan.len() {
        bail!(
            "the input is not a DAG: {} of {} derivations sit on a cycle",
            plan.len() - order.len(),
            plan.len()
        );
    }
    Ok(order)
}

impl Metrics {
    pub fn compute(plan: &Plan) -> Result<Self> {
        if plan.len() > MAX_NODES {
            bail!(
                "plan has {} derivations, past the {MAX_NODES} reachability ceiling; \
                 narrow the installable and rerun",
                plan.len()
            );
        }
        let order = topological_order(plan)?;

        // Transitive dependents. Built in reverse dependency order so a node's
        // dependents are already final when it is visited, then dropped: only
        // the counts survive, so the two matrices are never both resident.
        let blast = {
            let mut reach = Reachability::new(plan.len());
            for &id in order.iter().rev() {
                for &dependent in &plan.dependents[id] {
                    reach.union_into(id, dependent);
                    reach.set(id, dependent);
                }
            }
            (0..plan.len()).map(|id| reach.count(id)).collect()
        };

        // Transitive dependencies. Kept live past the count because the sole
        // carrier test asks whether a dependent reaches the node some other way.
        let mut closure = Reachability::new(plan.len());
        for &id in &order {
            for &dep in &plan.nodes[id].deps {
                closure.union_into(id, dep);
                closure.set(id, dep);
            }
        }
        let own_closure = (0..plan.len()).map(|id| closure.count(id)).collect();

        let mut depth = vec![0_u32; plan.len()];
        for &id in &order {
            depth[id] = plan.nodes[id]
                .deps
                .iter()
                .map(|&dep| depth[dep] + 1)
                .max()
                .unwrap_or(0);
        }
        let levels = depth.iter().copied().max().unwrap_or(0) as usize + 1;
        let mut widths = vec![0_u32; levels];
        for &level in &depth {
            widths[level as usize] += 1;
        }

        let carriers = carrier_counts(plan, &closure);
        let critical_path = critical_path(plan, &depth);

        // `MAX_NODES` is far below `u32::MAX`, so no dependent list can overflow
        // the count; the conversion is propagated rather than clamped because a
        // clamp would silently understate a fan-out.
        let fan_out = plan
            .dependents
            .iter()
            .map(|dependents| u32::try_from(dependents.len()))
            .collect::<Result<Vec<u32>, _>>()?;

        Ok(Self {
            fan_out,
            blast,
            own_closure,
            carrier: carriers.per_node,
            top_carrier_key: carriers.top,
            widths,
            critical_path,
        })
    }
}

/// What one pass over the plan's environment references yields per node.
struct CarrierAnalysis {
    per_node: Vec<CarrierCounts>,
    top: Vec<Option<TopCarrier>>,
}

/// For every node, how many direct dependents only ever name it in a carrier
/// variable, and how many of those have no other route to it.
///
/// The second number is the one that separates a design defect from a fact of
/// life. `gcc-wrapper` is handed to thousands of cargo units through
/// `CC_x86_64_unknown_linux_gnu`, but each of those units also reaches it
/// through stdenv, so deleting the variable would not save a single rebuild. A
/// library injected into every unit's environment and reached no other way is
/// pure invalidation: nothing consumes it, and everything rebuilds when it moves.
fn carrier_counts(plan: &Plan, closure: &Reachability) -> CarrierAnalysis {
    let mut counts = vec![CarrierCounts::default(); plan.len()];
    // (target, key) tallies, collected flat and folded once, so the common case
    // of a node nothing carries costs no allocation.
    let mut key_tally: Vec<(usize, usize)> = Vec::new();

    for node in &plan.nodes {
        let mut at = 0;
        while at < node.env_refs.len() {
            let target = node.env_refs[at].target;
            let mut end = at;
            let mut all_carrier = true;
            while end < node.env_refs.len() && node.env_refs[end].target == target {
                all_carrier &= node.env_refs[end].carrier;
                end += 1;
            }
            // Only direct dependents count. Nix derives `inputDrvs` from the
            // store paths in the attributes, so a carried path is normally a
            // direct input already; the check keeps the fan-out denominator
            // honest if it ever is not.
            if all_carrier && node.deps.binary_search(&target).is_ok() {
                counts[target].carried += 1;
                for reference in &node.env_refs[at..end] {
                    key_tally.push((target, reference.key));
                }
                let reached_otherwise = node
                    .deps
                    .iter()
                    .any(|&dep| dep != target && closure.get(dep, target));
                if !reached_otherwise {
                    counts[target].sole += 1;
                }
            }
            at = end;
        }
    }

    key_tally.sort_unstable();
    let mut top: Vec<Option<TopCarrier>> = vec![None; plan.len()];
    let mut at = 0;
    while at < key_tally.len() {
        let (target, key) = key_tally[at];
        let mut end = at;
        // Counted in the run rather than differenced afterwards, so the tally
        // width never has to be narrowed from a `usize`.
        let mut count: u32 = 0;
        while end < key_tally.len() && key_tally[end] == (target, key) {
            end += 1;
            count += 1;
        }
        if top[target].is_none_or(|best| count > best.count) {
            top[target] = Some(TopCarrier { key, count });
        }
        at = end;
    }
    CarrierAnalysis {
        per_node: counts,
        top,
    }
}

/// The longest chain in the plan, deepest node first down to a source.
fn critical_path(plan: &Plan, depth: &[u32]) -> Vec<usize> {
    let Some(mut at) = (0..plan.len()).max_by_key(|&id| depth[id]) else {
        return Vec::new();
    };
    let mut path = vec![at];
    while let Some(&next) = plan.nodes[at]
        .deps
        .iter()
        .max_by_key(|&&dep| depth[dep])
    {
        path.push(next);
        at = next;
    }
    path
}

#[cfg(test)]
mod tests {
    use super::Metrics;
    use crate::plan::Plan;

    const HASH: &str = "abcdefghijklmnopqrstuvwxyz012345";

    fn drv(name: &str, deps: &[&str], env: &str) -> String {
        let inputs = deps
            .iter()
            .map(|dep| format!(r#""/nix/store/{HASH}-{dep}.drv":["out"]"#))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#""/nix/store/{HASH}-{name}.drv":{{"name":"{name}",
               "outputs":{{"out":{{"path":"/nix/store/{HASH}-{name}"}}}},
               "inputs":{{"drvs":{{{inputs}}}}},"env":{{{env}}}}}"#
        )
    }

    /// A diamond with a carried leaf:
    ///
    /// ```text
    ///   root -> mid-a -> base
    ///   root -> mid-b -> base
    ///   mid-a, mid-b -> carried   (env only)
    ///   mid-b -> carried          (also structurally, via used)
    /// ```
    fn fixture() -> Plan {
        let pointer = format!(r#""IX_LIB":"/nix/store/{HASH}-carried/lib""#);
        let nodes = [
            drv("base", &[], ""),
            drv("carried", &[], ""),
            drv("used", &["carried"], ""),
            drv("mid-a", &["base", "carried"], &pointer),
            drv("mid-b", &["base", "carried", "used"], &pointer),
            drv("root", &["mid-a", "mid-b"], ""),
        ]
        .join(",");
        Plan::from_json(&format!(r#"{{"version":4,"derivations":{{{nodes}}}}}"#))
            .expect("fixture parses")
    }

    fn id(plan: &Plan, name: &str) -> usize {
        plan.nodes
            .iter()
            .position(|node| node.name == name)
            .unwrap_or_else(|| panic!("no node {name}"))
    }

    // Blast radius counts transitive dependents once even when several paths
    // reach them, which a naive edge count would double.
    #[test]
    fn blast_radius_counts_each_dependent_once() {
        let plan = fixture();
        let metrics = Metrics::compute(&plan).expect("metrics");
        // base <- mid-a, mid-b <- root: three distinct dependents through two paths.
        assert_eq!(metrics.blast[id(&plan, "base")], 3);
        assert_eq!(metrics.fan_out[id(&plan, "base")], 2);
        assert_eq!(metrics.own_closure[id(&plan, "root")], 5);
    }

    // The point of the sole count: `mid-b` also reaches `carried` through `used`,
    // so deleting IX_LIB would not stop it rebuilding, while `mid-a` has no other
    // route and would. Both are carriers; only one is avoidable.
    #[test]
    fn sole_carrier_excludes_dependents_with_another_route() {
        let plan = fixture();
        let metrics = Metrics::compute(&plan).expect("metrics");
        let carried = id(&plan, "carried");
        assert_eq!(metrics.carrier[carried].carried, 2);
        assert_eq!(metrics.carrier[carried].sole, 1);
        let top = metrics.top_carrier_key[carried].expect("a carrier key");
        assert_eq!(plan.env_keys[top.key], "IX_LIB");
        assert_eq!(top.count, 2);
    }

    #[test]
    fn critical_path_is_the_longest_chain() {
        let plan = fixture();
        let metrics = Metrics::compute(&plan).expect("metrics");
        // root -> mid-b -> used -> carried is 4 nodes; the base branch is 3.
        assert_eq!(metrics.critical_path.len(), 4);
        assert_eq!(plan.nodes[metrics.critical_path[0]].name, "root");
        let nodes = u32::try_from(plan.len()).expect("the fixture is small");
        assert_eq!(metrics.widths.iter().sum::<u32>(), nodes);
    }

    // A cycle is not a build plan; say so instead of reporting numbers computed
    // over a partial traversal.
    #[test]
    fn cycles_are_rejected() {
        let nodes = [drv("a", &["b"], ""), drv("b", &["a"], "")].join(",");
        let plan = Plan::from_json(&format!(r#"{{"version":4,"derivations":{{{nodes}}}}}"#))
            .expect("parses");
        let Err(error) = Metrics::compute(&plan) else {
            panic!("a cycle must be rejected")
        };
        assert!(error.to_string().contains("not a DAG"), "{error}");
    }
}
