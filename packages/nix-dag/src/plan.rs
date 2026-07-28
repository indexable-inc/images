//! Read a build plan out of `nix derivation show --recursive` and index the
//! store-path references hiding in each derivation's environment.
//!
//! Evaluation only: `nix derivation show` forces the derivations but builds
//! nothing, so a plan can be scored while every builder is busy.

use std::collections::{BTreeMap, HashMap};
use std::io::Read as _;
use std::path::Path;
use std::process::Command;

use color_eyre::eyre::{Context, Result, bail};
use serde::Deserialize;

/// Environment variables Nix itself reads to build the dependency graph, plus
/// the ones stdenv uses to place a store path on a search path. A reference
/// through one of these is the derivation genuinely consuming its input, so it
/// is never a carrier however path-shaped the value looks.
const STRUCTURAL_KEYS: &[&str] = &[
    "PATH",
    "allowedReferences",
    "allowedRequisites",
    "args",
    "builder",
    "buildInputs",
    "depsBuildBuild",
    "depsBuildBuildPropagated",
    "depsBuildTarget",
    "depsBuildTargetPropagated",
    "depsHostHost",
    "depsHostHostPropagated",
    "depsTargetTarget",
    "depsTargetTargetPropagated",
    "disallowedReferences",
    "disallowedRequisites",
    "nativeBuildInputs",
    "outputs",
    "propagatedBuildInputs",
    "propagatedNativeBuildInputs",
    "src",
    "srcs",
    "stdenv",
    "system",
];

/// `nix derivation show` schema 4+: a `{ version, derivations }` envelope whose
/// `derivations` map is keyed by `.drv` path.
#[derive(Deserialize)]
struct ShowOutput {
    derivations: BTreeMap<String, ShowDrv>,
}

#[derive(Deserialize)]
struct ShowDrv {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    inputs: ShowInputs,
    #[serde(default)]
    outputs: BTreeMap<String, ShowDrvOutput>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

#[derive(Deserialize, Default)]
struct ShowInputs {
    #[serde(default)]
    drvs: BTreeMap<String, serde_json::Value>,
}

/// An output of a derivation. `path` is absent for content-addressed outputs,
/// whose path is not known until the build runs; see [`Plan::unresolved_outputs`].
#[derive(Deserialize)]
struct ShowDrvOutput {
    #[serde(default)]
    path: Option<String>,
}

/// One derivation's reference to another, found in an environment value.
pub struct EnvRef {
    /// The derivation whose output path the value names.
    pub target: usize,
    /// Index into [`Plan::env_keys`].
    pub key: usize,
    /// The value holds store paths and nothing else, and the key is not one Nix
    /// or stdenv reads structurally. The variable points at the target rather
    /// than using it, so the dependent rebuilds on every change to a thing it
    /// may never open.
    pub carrier: bool,
}

/// One derivation in the plan.
pub struct Node {
    /// The `.drv` path, as `nix derivation show` keys it.
    pub drv_path: String,
    /// Readable name, hash prefix stripped.
    pub name: String,
    /// Input derivations, as node indices.
    pub deps: Vec<usize>,
    /// References to other derivations found in this one's environment.
    pub env_refs: Vec<EnvRef>,
}

/// A whole build plan: every derivation in the closure, plus the reverse edges.
pub struct Plan {
    pub nodes: Vec<Node>,
    pub dependents: Vec<Vec<usize>>,
    /// Interned environment variable names, indexed by [`EnvRef::key`].
    pub env_keys: Vec<String>,
    /// Derivations whose output paths are not known before the build, so their
    /// environment references cannot be resolved. Reported, not hidden: a plan
    /// that is mostly content-addressed gets a weaker carrier analysis and the
    /// reader has to know that.
    pub unresolved_outputs: usize,
}

impl Plan {
    pub const fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn edges(&self) -> usize {
        self.nodes.iter().map(|node| node.deps.len()).sum()
    }

    /// Ask Nix for the closure of `installable`. Pure evaluation; no builder is
    /// started, though import-from-derivation inside the flake may still build.
    pub fn from_installable(installable: &str) -> Result<Self> {
        let output = Command::new("nix")
            .args([
                "derivation",
                "show",
                "--recursive",
                "--extra-experimental-features",
                "nix-command flakes ca-derivations",
                installable,
            ])
            .output()
            .context("spawn nix derivation show")?;
        if !output.status.success() {
            bail!(
                "nix derivation show --recursive {installable} failed ({}):\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let stdout = String::from_utf8(output.stdout).context("nix output was not UTF-8")?;
        Self::from_json(&stdout)
    }

    /// Read a plan from a captured `nix derivation show --recursive` dump, or
    /// from stdin when `path` is `-`.
    pub fn from_file(path: &Path) -> Result<Self> {
        let text = if path == Path::new("-") {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("read plan from stdin")?;
            buf
        } else {
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?
        };
        Self::from_json(&text)
    }

    pub fn from_json(text: &str) -> Result<Self> {
        let shown: ShowOutput =
            serde_json::from_str(text).context("parse nix derivation show output")?;
        Ok(Self::build(&shown.derivations))
    }

    fn build(derivations: &BTreeMap<String, ShowDrv>) -> Self {
        let index: HashMap<&str, usize> = derivations
            .keys()
            .enumerate()
            .map(|(id, key)| (key.as_str(), id))
            .collect();

        // Output path back to the derivation that produces it, so an env value
        // naming a store path can be attributed to a node. Keyed by basename
        // because the two sides are spelled differently: `nix derivation show`
        // prints outputs and input keys store-relative (`<hash>-<name>`), while
        // the environment values it prints are the literal build-time strings,
        // absolute (`/nix/store/<hash>-<name>/lib`). Matching on the basename
        // joins them whichever spelling a given Nix emits.
        let mut producer: HashMap<&str, usize> = HashMap::new();
        let mut unresolved_outputs = 0;
        for (key, drv) in derivations {
            let id = index[key.as_str()];
            let mut resolved = false;
            for out in drv.outputs.values() {
                if let Some(path) = &out.path {
                    producer.insert(store_basename(path), id);
                    resolved = true;
                }
            }
            if !resolved && !drv.outputs.is_empty() {
                unresolved_outputs += 1;
            }
        }

        let mut env_keys: Vec<String> = Vec::new();
        let mut key_ids: HashMap<&str, usize> = HashMap::new();
        let mut nodes: Vec<Node> = Vec::with_capacity(derivations.len());

        for (key, drv) in derivations {
            // Sorted so `carrier_counts` can binary-search for a direct edge.
            let mut deps: Vec<usize> = drv
                .inputs
                .drvs
                .keys()
                .filter_map(|input| index.get(input.as_str()).copied())
                .collect();
            deps.sort_unstable();
            deps.dedup();

            let mut env_refs: Vec<EnvRef> = Vec::new();
            for (name, value) in &drv.env {
                if !value.contains("/nix/store/") {
                    continue;
                }
                let carrier = !STRUCTURAL_KEYS.contains(&name.as_str()) && is_carrier_value(value);
                let key_id = *key_ids.entry(name.as_str()).or_insert_with(|| {
                    env_keys.push(name.clone());
                    env_keys.len() - 1
                });
                for root in store_roots(value) {
                    if let Some(&target) = producer.get(root) {
                        env_refs.push(EnvRef {
                            target,
                            key: key_id,
                            carrier,
                        });
                    }
                }
            }
            env_refs.sort_by_key(|reference| (reference.target, reference.key));
            env_refs.dedup_by_key(|reference| (reference.target, reference.key));

            nodes.push(Node {
                drv_path: key.clone(),
                name: drv
                    .name
                    .clone()
                    .unwrap_or_else(|| readable_name(key.as_str())),
                deps,
                env_refs,
            });
        }

        let mut dependents = vec![Vec::new(); nodes.len()];
        for (id, node) in nodes.iter().enumerate() {
            for &dep in &node.deps {
                dependents[dep].push(id);
            }
        }

        Self {
            nodes,
            dependents,
            env_keys,
            unresolved_outputs,
        }
    }
}

/// Strip `/nix/store/<hash>-` and any `.drv` suffix, leaving a readable name.
pub fn readable_name(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    let base = base.strip_suffix(".drv").unwrap_or(base);
    if is_hash_prefixed(base) {
        base[33..].to_owned()
    } else {
        base.to_owned()
    }
}

fn is_hash_prefixed(base: &str) -> bool {
    let bytes = base.as_bytes();
    bytes.len() > 33
        && bytes[32] == b'-'
        && bytes[..32]
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

const STORE_PREFIX: &str = "/nix/store/";

const fn is_store_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.' | '_' | '?' | '=')
}

/// The last path segment, which for a store path is `<hash>-<name>`.
fn store_basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Every `<hash>-<name>` store root mentioned in `value`, without the subpath,
/// so `.../lib` and `.../bin/foo` both attribute to the same derivation.
fn store_roots(value: &str) -> Vec<&str> {
    let mut roots: Vec<&str> = value
        .match_indices(STORE_PREFIX)
        .filter_map(|(at, _)| {
            let tail = &value[at + STORE_PREFIX.len()..];
            let end = tail
                .find(|ch: char| !is_store_name_char(ch))
                .unwrap_or(tail.len());
            let root = &tail[..end];
            is_hash_prefixed(root).then_some(root)
        })
        .collect();
    roots.sort_unstable();
    roots.dedup();
    roots
}

/// Is this environment value nothing but store paths?
///
/// This is the whole carrier test. A value that is only paths is a pointer: the
/// variable exists to hand the dependent a location. A value with any other
/// token is a script, a flag list, or prose that happens to name a path, which
/// means the derivation actually does something with it. Splitting on `:` as
/// well as whitespace catches search-path variables (`PKG_CONFIG_PATH`), which
/// carry exactly the same invalidation whether or not anything reads them.
fn is_carrier_value(value: &str) -> bool {
    let mut saw_token = false;
    for token in value.split([' ', '\t', '\n', '\r', ':']) {
        if token.is_empty() {
            continue;
        }
        saw_token = true;
        let Some(rest) = token.strip_prefix(STORE_PREFIX) else {
            return false;
        };
        if !is_hash_prefixed(rest) {
            return false;
        }
    }
    saw_token
}

#[cfg(test)]
mod tests {
    use super::{Plan, is_carrier_value, readable_name, store_roots};

    const HASH: &str = "abcdefghijklmnopqrstuvwxyz012345";

    #[test]
    fn readable_name_strips_hash_and_drv_suffix() {
        assert_eq!(
            readable_name(&format!("/nix/store/{HASH}-libghostty-vt-1.3.2-dev.drv")),
            "libghostty-vt-1.3.2-dev"
        );
        assert_eq!(readable_name("plain.drv"), "plain");
    }

    // A search-path variable and a bare path are carriers; a build script that
    // merely mentions a path is not, and neither is an empty value.
    #[test]
    fn carrier_test_separates_pointers_from_scripts() {
        assert!(is_carrier_value(&format!("/nix/store/{HASH}-libghostty-vt/lib")));
        assert!(is_carrier_value(&format!(
            "/nix/store/{HASH}-zlib/lib/pkgconfig:/nix/store/{HASH}-openssl/lib/pkgconfig"
        )));
        assert!(!is_carrier_value(&format!(
            "cc -I/nix/store/{HASH}-zlib/include main.c"
        )));
        assert!(!is_carrier_value("   "));
        assert!(!is_carrier_value("/usr/lib"));
    }

    #[test]
    fn store_roots_drops_subpaths_and_dedupes() {
        let value = format!("/nix/store/{HASH}-zlib/lib:/nix/store/{HASH}-zlib/include");
        assert_eq!(store_roots(&value), vec![format!("{HASH}-zlib")]);
    }

    // An env value naming an input's output path is attributed to that input,
    // and the carrier flag follows the key: `IX_LIB_DIR` points, `buildPhase`
    // uses. Guards the join from env text back to a graph node, which is what
    // the whole carrier metric rests on, and pins the spelling mismatch that
    // makes it delicate: Nix keys derivations and outputs store-RELATIVE while
    // printing environment values ABSOLUTE. Matching those literally finds
    // nothing and reports a clean plan, which is how this first shipped wrong.
    #[test]
    fn env_references_resolve_to_the_producing_derivation() {
        let json = format!(
            r#"{{"version":4,"derivations":{{
              "{HASH}-lib.drv":{{"name":"lib","outputs":{{"out":{{"path":"{HASH}-lib"}}}},"inputs":{{"drvs":{{}}}},"env":{{}}}},
              "{HASH}-user.drv":{{"name":"user","outputs":{{"out":{{"path":"{HASH}-user"}}}},
                "inputs":{{"drvs":{{"{HASH}-lib.drv":["out"]}}}},
                "env":{{"IX_LIB_DIR":"/nix/store/{HASH}-lib/lib","buildPhase":"gcc -L/nix/store/{HASH}-lib/lib"}}}}
            }}}}"#
        );
        let plan = Plan::from_json(&json).expect("well-formed plan parses");
        let user = plan
            .nodes
            .iter()
            .position(|node| node.name == "user")
            .expect("user node");
        let lib = plan
            .nodes
            .iter()
            .position(|node| node.name == "lib")
            .expect("lib node");

        let refs = &plan.nodes[user].env_refs;
        assert_eq!(refs.len(), 2, "one per env key naming the lib");
        assert!(refs.iter().all(|reference| reference.target == lib));
        let by_key: Vec<(&str, bool)> = refs
            .iter()
            .map(|reference| (plan.env_keys[reference.key].as_str(), reference.carrier))
            .collect();
        assert!(by_key.contains(&("IX_LIB_DIR", true)));
        assert!(by_key.contains(&("buildPhase", false)));
    }
}
