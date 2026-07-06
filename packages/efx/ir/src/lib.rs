//! Content-addressed effect IR.
//!
//! A [`Plan`] is a set of [`Effect`] nodes wired by output references. Every
//! effect gets an [`EffectId`]: the SHA-256 of a canonical JSON document over
//! `(kind, executor, resolved input hashes)`. A literal input contributes the
//! hash of its value; a reference input contributes the *id* of the effect it
//! points at, so an upstream change re-identifies every dependent — cache
//! invalidation falls out of identity rather than being tracked separately.
//!
//! The IR is the contract: any frontend (the `.efx` language, or a
//! general-purpose program) that emits a `Plan` gets memoization, diffing,
//! and scheduling from the engine for free.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use snafu::Snafu;

/// Every way a plan can fail validation.
#[derive(Debug, Snafu)]
pub enum PlanError {
    #[snafu(display("duplicate effect name `{name}`"))]
    DuplicateName { name: String },

    #[snafu(display("effect `{from}` references unknown effect `{to}`"))]
    UnknownReference { from: String, to: String },

    #[snafu(display("dependency cycle involving effect `{name}`"))]
    Cycle { name: String },
}

/// A literal input or output value. Deliberately float-free so canonical
/// serialization never depends on float formatting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Literal {
    Str(String),
    Int(i64),
    Bool(bool),
}

impl Literal {
    /// Hex SHA-256 of the value, tagged by type so `"1"` and `1` differ.
    #[must_use]
    pub fn content_hash(&self) -> String {
        let mut hasher = Sha256::new();
        match self {
            Self::Str(s) => {
                hasher.update(b"str:");
                hasher.update(s.as_bytes());
            }
            Self::Int(n) => {
                hasher.update(b"int:");
                hasher.update(n.to_string().as_bytes());
            }
            Self::Bool(b) => {
                hasher.update(b"bool:");
                hasher.update(b.to_string().as_bytes());
            }
        }
        hex::encode(hasher.finalize())
    }

    /// The value rendered for human-facing output (no quoting).
    #[must_use]
    pub fn display_string(&self) -> String {
        match self {
            Self::Str(s) => s.clone(),
            Self::Int(n) => n.to_string(),
            Self::Bool(b) => b.to_string(),
        }
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_string())
    }
}

/// A reference to one named output field of another effect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputRef {
    pub effect: String,
    pub field: String,
}

/// An input value: either a literal or a wire from another effect's output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Value {
    Literal(Literal),
    Ref(OutputRef),
}

/// Execution metadata carried alongside an effect, not part of its identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectMeta {
    pub idempotent: bool,
    pub rollback_hint: Option<String>,
}

impl Default for EffectMeta {
    fn default() -> Self {
        Self {
            idempotent: true,
            rollback_hint: None,
        }
    }
}

/// One node of a plan.
///
/// `inputs` and `meta` default when absent so hand-authored or generated IR
/// documents (`efx plan --ir`) only spell what they mean.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effect {
    /// Human name, unique within a plan. Not part of the identity.
    pub name: String,
    /// What the effect does, e.g. `file.write`.
    pub kind: String,
    /// Which executor runs it. Usually equal to `kind`; kept separate so a
    /// kind can be re-bound to a different implementation.
    pub executor: String,
    #[serde(default)]
    pub inputs: BTreeMap<String, Value>,
    #[serde(default)]
    pub meta: EffectMeta,
}

/// Content-addressed identity of an effect: 32 bytes of SHA-256.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EffectId([u8; 32]);

impl EffectId {
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Short prefix for human-facing output.
    #[must_use]
    pub fn short(&self) -> String {
        hex::encode(&self.0[..6])
    }
}

impl fmt::Display for EffectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for EffectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EffectId({})", self.to_hex())
    }
}

impl Serialize for EffectId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for EffectId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = hex::decode(&text).map_err(serde::de::Error::custom)?;
        let array: [u8; 32] = bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("effect id must be 32 bytes of hex"))?;
        Ok(Self(array))
    }
}

/// One resolved input as it enters the identity hash. Serialized inside a
/// `BTreeMap`, and the struct fields are declared alphabetically, so the
/// canonical JSON has fully sorted keys.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum IdInput {
    /// Content hash of a literal value.
    Lit(String),
    /// Identity of the referenced effect plus the field read from it.
    Ref { effect: String, field: String },
}

#[derive(Serialize)]
struct IdDocument<'a> {
    executor: &'a str,
    inputs: BTreeMap<&'a str, IdInput>,
    kind: &'a str,
}

/// A dataflow edge, derived from reference inputs: `from` must run before `to`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
}

/// An ordered set of effects with unique names.
///
/// Deserialization goes through [`Plan::from_effects`], so a `Plan` parsed
/// from an IR document carries the same name-uniqueness invariant as one
/// built through [`Plan::add`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Plan {
    effects: Vec<Effect>,
}

impl<'de> Deserialize<'de> for Plan {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Document {
            effects: Vec<Effect>,
        }
        let document = Document::deserialize(deserializer)?;
        Self::from_effects(document.effects).map_err(serde::de::Error::custom)
    }
}

impl Plan {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            effects: Vec::new(),
        }
    }

    /// Builds a plan from effects in order, enforcing name uniqueness.
    ///
    /// # Errors
    ///
    /// Returns [`PlanError::DuplicateName`] when two effects share a name.
    pub fn from_effects(effects: impl IntoIterator<Item = Effect>) -> Result<Self, PlanError> {
        let mut plan = Self::new();
        for effect in effects {
            plan.add(effect)?;
        }
        Ok(plan)
    }

    /// Adds an effect, keeping declaration order.
    ///
    /// # Errors
    ///
    /// Returns [`PlanError::DuplicateName`] when the name is already taken.
    pub fn add(&mut self, effect: Effect) -> Result<(), PlanError> {
        if self.effects.iter().any(|e| e.name == effect.name) {
            return Err(PlanError::DuplicateName { name: effect.name });
        }
        self.effects.push(effect);
        Ok(())
    }

    #[must_use]
    pub fn effects(&self) -> &[Effect] {
        &self.effects
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Effect> {
        self.effects.iter().find(|e| e.name == name)
    }

    /// Dataflow edges derived from reference inputs, deduplicated.
    #[must_use]
    pub fn edges(&self) -> Vec<Edge> {
        let mut seen = BTreeSet::new();
        let mut edges = Vec::new();
        for effect in &self.effects {
            for value in effect.inputs.values() {
                if let Value::Ref(r) = value
                    && seen.insert((r.effect.clone(), effect.name.clone()))
                {
                    edges.push(Edge {
                        from: r.effect.clone(),
                        to: effect.name.clone(),
                    });
                }
            }
        }
        edges
    }

    /// Effects in a deterministic dependency order (declaration order among
    /// ready nodes).
    ///
    /// # Errors
    ///
    /// Returns [`PlanError::UnknownReference`] for a reference to an absent
    /// effect and [`PlanError::Cycle`] when the dataflow graph has a cycle.
    pub fn topo_order(&self) -> Result<Vec<&Effect>, PlanError> {
        for effect in &self.effects {
            for value in effect.inputs.values() {
                if let Value::Ref(r) = value
                    && self.get(&r.effect).is_none()
                {
                    return Err(PlanError::UnknownReference {
                        from: effect.name.clone(),
                        to: r.effect.clone(),
                    });
                }
            }
        }
        let mut done: BTreeSet<&str> = BTreeSet::new();
        let mut order = Vec::with_capacity(self.effects.len());
        while order.len() < self.effects.len() {
            let mut advanced = false;
            for effect in &self.effects {
                if done.contains(effect.name.as_str()) {
                    continue;
                }
                let ready = effect.inputs.values().all(|value| match value {
                    Value::Ref(r) => done.contains(r.effect.as_str()),
                    Value::Literal(_) => true,
                });
                if ready {
                    done.insert(effect.name.as_str());
                    order.push(effect);
                    advanced = true;
                }
            }
            if !advanced {
                let stuck = self
                    .effects
                    .iter()
                    .find(|e| !done.contains(e.name.as_str()))
                    .map_or_else(String::new, |e| e.name.clone());
                return Err(PlanError::Cycle { name: stuck });
            }
        }
        Ok(order)
    }

    /// Computes the content-addressed identity of every effect.
    ///
    /// # Errors
    ///
    /// Propagates the graph errors of [`Plan::topo_order`].
    pub fn effect_ids(&self) -> Result<BTreeMap<String, EffectId>, PlanError> {
        let mut ids: BTreeMap<String, EffectId> = BTreeMap::new();
        for effect in self.topo_order()? {
            let mut inputs = BTreeMap::new();
            for (key, value) in &effect.inputs {
                let entry = match value {
                    Value::Literal(lit) => IdInput::Lit(lit.content_hash()),
                    // Upstream ids exist: topo order visits dependencies first.
                    Value::Ref(r) => IdInput::Ref {
                        effect: ids[&r.effect].to_hex(),
                        field: r.field.clone(),
                    },
                };
                inputs.insert(key.as_str(), entry);
            }
            let doc = IdDocument {
                executor: &effect.executor,
                inputs,
                kind: &effect.kind,
            };
            let canonical =
                serde_json::to_vec(&doc).unwrap_or_else(|_| unreachable!("plain data serializes"));
            let digest: [u8; 32] = Sha256::digest(&canonical).into();
            ids.insert(effect.name.clone(), EffectId(digest));
        }
        Ok(ids)
    }

    /// Per-input signatures of one effect, given the plan's ids: literal
    /// inputs sign their content hash, reference inputs sign the upstream id
    /// plus field. Two runs disagree on a signature exactly when that input
    /// changed — the engine uses this to explain invalidations.
    #[must_use]
    pub fn input_signatures(
        effect: &Effect,
        ids: &BTreeMap<String, EffectId>,
    ) -> BTreeMap<String, String> {
        effect
            .inputs
            .iter()
            .map(|(key, value)| {
                let signature = match value {
                    Value::Literal(lit) => format!("lit:{}", lit.content_hash()),
                    Value::Ref(r) => {
                        let id = ids
                            .get(&r.effect)
                            .map_or_else(String::new, EffectId::to_hex);
                        format!("ref:{id}:{}", r.field)
                    }
                };
                (key.clone(), signature)
            })
            .collect()
    }
}
