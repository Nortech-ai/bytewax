//! Data model representing state in the dataflow and the recovery
//! system.
//!
//! A state store is a K-V mapping from [`StoreKey`] to
//! [`StateBytes`].

use super::change::*;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde::Serialize;
use std::any::type_name;
use std::cell::RefCell;
use std::collections::hash_map;
use std::collections::hash_map::Keys;
use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;

/// Unique ID for a step in a dataflow.
///
/// Recovery data is keyed off of this to ensure state is not mixed
/// between operators.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StepId(pub(crate) String);

/// Key to route state within a dataflow step.
///
/// This is the user-facing "state key" since users only work with
/// state within a step.
///
/// We place restraints on this, rather than allowing any Python type
/// to be routeable because the routing key interfaces with a lot of
/// Bytewax and Timely code which puts requirements on it: it has to
/// be hashable, have equality, debug printable, and is serde-able and
/// we can't guarantee those things are correct on any arbitrary
/// Python type.
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct StateKey(pub(crate) String);

/// Key to route state within a whole dataflow.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FlowKey(pub(crate) StepId, pub(crate) StateKey);

/// This state snapshot happened at the end of this epoch.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct SnapshotEpoch(pub(crate) u64);

/// Key to route state within the state store.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoreKey(pub(crate) SnapshotEpoch, pub(crate) FlowKey);

/// A snapshot of state for a specific key within a step.
///
/// The recovery system only deals in bytes so each operator can store
/// custom types without going through Rust generic gymnastics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StateBytes(pub(crate) Vec<u8>);

impl StateBytes {
    /// Serialize this state object from an operator into bytes the
    /// recovery system can store.
    pub(crate) fn ser<T: Serialize>(obj: &T) -> Self {
        // TODO: Figure out if there's a more robust-to-evolution way
        // to serialize this key. If the serialization changes between
        // versions, then recovery doesn't work. Or if we use an
        // encoding that isn't deterministic.
        let t_name = type_name::<T>();
        Self(
            bincode::serialize(obj)
                .unwrap_or_else(|_| panic!("Error serializing recovery state type {t_name})")),
        )
    }

    /// Deserialize these bytes from the recovery system into a state
    /// object that an operator can use.
    pub(crate) fn de<T: DeserializeOwned>(self) -> T {
        let t_name = type_name::<T>();
        bincode::deserialize(&self.0)
            .unwrap_or_else(|_| panic!("Error deserializing recovery state type {t_name})"))
    }
}

/// A change to state within the dataflow.
pub(crate) type FlowChange = KChange<FlowKey, StateBytes>;

/// A change to the state store.
pub(crate) type StoreChange = KChange<StoreKey, Change<StateBytes>>;

/// All state stores have to implement this writer.
pub(crate) trait StateWriter: KWriter<StoreKey, Change<StateBytes>> {}

impl<P> StateWriter for Box<P> where P: StateWriter + ?Sized {}
impl<P> StateWriter for Rc<RefCell<P>> where P: StateWriter + ?Sized {}

/// All state stores have to implement this reader.
pub(crate) trait StateReader: KReader<StoreKey, Change<StateBytes>> {}

impl<P> StateReader for Box<P> where P: StateReader + ?Sized {}

/// A change to the state store, but elide the actual state change
/// within the dataflow and only keep the "type" of change.
///
/// This is used to allow the GC component to not need to store full
/// state snapshots.
pub(crate) type StoreChangeSummary = KChange<StoreKey, ChangeType>;

/// A snapshot of all state within a step.
///
/// Built up during the resume process.
#[derive(Debug, Default)]
pub(crate) struct StepStateBytes(HashMap<StateKey, StateBytes>);

impl StepStateBytes {
    pub(crate) fn remove(&mut self, key: &StateKey) -> Option<StateBytes> {
        self.0.remove(key)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn keys(&self) -> Keys<'_, StateKey, StateBytes> {
        self.0.keys()
    }
}

impl IntoIterator for StepStateBytes {
    type Item = (StateKey, StateBytes);

    type IntoIter = hash_map::IntoIter<StateKey, StateBytes>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl KWriter<StateKey, StateBytes> for StepStateBytes {
    fn write(&mut self, kchange: KChange<StateKey, StateBytes>) {
        self.0.write(kchange)
    }
}

/// A snapshot of all state within a dataflow.
///
/// Built up during the resume process.
#[derive(Debug)]
pub(crate) struct FlowStateBytes(HashMap<StepId, StepStateBytes>);

impl FlowStateBytes {
    pub(crate) fn new() -> Self {
        Self(HashMap::new())
    }

    pub(crate) fn remove(&mut self, step_id: &StepId) -> StepStateBytes {
        if let Some(op_state) = self.0.remove(step_id) {
            op_state
        } else {
            if !self.0.is_empty() {
                tracing::warn!(
                    "No resume state for {step_id:?}, \
                    but other steps have state; did you add or rename a \
                    step and forget to init or migrate state data?"
                );
            }
            Default::default()
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn keys(&self) -> Keys<'_, StepId, StepStateBytes> {
        self.0.keys()
    }
}

impl KWriter<FlowKey, StateBytes> for FlowStateBytes {
    fn write(&mut self, kchange: KChange<FlowKey, StateBytes>) {
        let KChange(FlowKey(step_id, state_key), change) = kchange;
        self.0
            .entry(step_id)
            .or_default()
            .write(KChange(state_key, change));
    }
}
