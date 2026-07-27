use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};
use synapse_local_service::{
    LocalService, OperationAccepted, OperationKind, OperationResult, OperationState,
    OperationStatus, Problem as ServiceProblem, ServiceError,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::app::{monotonic_operation_timestamp, random_hex};
use crate::security::SecurityPolicy;

pub(crate) const MAX_BLOCKING_OPERATIONS: usize = 8;
pub(crate) const MAX_BLOCKING_OPERATIONS_PER_PROJECT: usize = 2;
pub(crate) const MAX_OPERATION_ENTRIES: usize = 256;
pub(crate) const MAX_ACTIVE_OPERATIONS: usize = 64;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) service: Arc<LocalService>,
    pub(crate) security: SecurityPolicy,
    pub(crate) blocking: BlockingGates,
    pub(crate) uploads: Arc<Semaphore>,
    pub(crate) operations: OperationRegistry,
}

#[derive(Clone)]
pub(crate) struct BlockingGates {
    pub(crate) overall: Arc<Semaphore>,
    pub(crate) projects: Arc<BTreeMap<String, Arc<Semaphore>>>,
}

impl BlockingGates {
    pub(crate) fn new(project_keys: impl IntoIterator<Item = String>) -> Self {
        let projects = project_keys
            .into_iter()
            .map(|key| {
                (
                    key,
                    Arc::new(Semaphore::new(MAX_BLOCKING_OPERATIONS_PER_PROJECT)),
                )
            })
            .collect();
        Self {
            overall: Arc::new(Semaphore::new(MAX_BLOCKING_OPERATIONS)),
            projects: Arc::new(projects),
        }
    }

    pub(crate) async fn acquire(
        &self,
        project_key: Option<&str>,
    ) -> Result<BlockingPermit, BlockingError> {
        // Acquire the narrower gate first so callers queued for one busy
        // project cannot consume all global capacity while they wait.
        let project = match project_key.and_then(|key| self.projects.get(key)) {
            Some(gate) => Some(
                gate.clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| BlockingError::Task)?,
            ),
            None => None,
        };
        let overall = self
            .overall
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| BlockingError::Task)?;
        Ok(BlockingPermit {
            _overall: overall,
            _project: project,
        })
    }
}

pub(crate) struct BlockingPermit {
    _overall: OwnedSemaphorePermit,
    _project: Option<OwnedSemaphorePermit>,
}

#[derive(Debug)]
pub(crate) enum BlockingError {
    Service(ServiceError),
    Task,
}

/// Finite process-local job metadata. The registry is only an observation
/// bridge for synchronous Core operations; it is never recovery authority.
#[derive(Clone, Default)]
pub(crate) struct OperationRegistry {
    state: Arc<Mutex<OperationRegistryState>>,
}

#[derive(Default)]
struct OperationRegistryState {
    entries: BTreeMap<String, OperationStatus>,
    insertion_order: VecDeque<String>,
    last_timestamp: Option<Duration>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OperationRegistryError {
    Capacity,
    Entropy,
    Clock,
}

impl OperationRegistry {
    pub(crate) fn reserve(
        &self,
        kind: OperationKind,
        project_key: String,
    ) -> Result<OperationAccepted, OperationRegistryError> {
        self.reserve_at(kind, project_key, SystemTime::now())
    }

    pub(crate) fn reserve_at(
        &self,
        kind: OperationKind,
        project_key: String,
        observed_at: SystemTime,
    ) -> Result<OperationAccepted, OperationRegistryError> {
        let mut registry = lock_operations(&self.state);
        let active = registry
            .entries
            .values()
            .filter(|status| !operation_is_terminal(status.state))
            .count();
        if active >= MAX_ACTIVE_OPERATIONS {
            return Err(OperationRegistryError::Capacity);
        }

        let expired_position = if registry.entries.len() >= MAX_OPERATION_ENTRIES {
            let expired = registry.insertion_order.iter().position(|operation_id| {
                registry
                    .entries
                    .get(operation_id)
                    .is_some_and(|status| operation_is_terminal(status.state))
            });
            if expired.is_none() {
                return Err(OperationRegistryError::Capacity);
            }
            expired
        } else {
            None
        };

        let mut operation_id = None;
        for _ in 0..4 {
            let candidate = random_hex(32).map_err(|_| OperationRegistryError::Entropy)?;
            if !registry.entries.contains_key(&candidate) {
                operation_id = Some(candidate);
                break;
            }
        }
        let operation_id = operation_id.ok_or(OperationRegistryError::Entropy)?;
        let submitted_at = monotonic_operation_timestamp(&mut registry.last_timestamp, observed_at)
            .ok_or(OperationRegistryError::Clock)?;

        if let Some(expired_position) = expired_position {
            let expired = registry
                .insertion_order
                .remove(expired_position)
                .expect("the selected terminal operation remains in admission order");
            registry.entries.remove(&expired);
        }

        let poll_path = format!("/api/v1/operations/{operation_id}");
        registry.insertion_order.push_back(operation_id.clone());
        registry.entries.insert(
            operation_id.clone(),
            OperationStatus {
                operation_id: operation_id.clone(),
                kind,
                project_key,
                state: OperationState::Queued,
                submitted_at,
                completed_at: None,
                result: None,
                error: None,
            },
        );
        Ok(OperationAccepted {
            operation_id,
            state: OperationState::Queued,
            poll_path,
        })
    }

    pub(crate) fn mark_running(&self, operation_id: &str) {
        let mut registry = lock_operations(&self.state);
        if let Some(status) = registry.entries.get_mut(operation_id)
            && status.state == OperationState::Queued
        {
            status.state = OperationState::Running;
        }
    }

    pub(crate) fn finish(
        &self,
        operation_id: &str,
        state: OperationState,
        result: Option<OperationResult>,
        error: Option<ServiceProblem>,
    ) {
        self.finish_at(operation_id, state, result, error, SystemTime::now());
    }

    pub(crate) fn finish_at(
        &self,
        operation_id: &str,
        state: OperationState,
        result: Option<OperationResult>,
        error: Option<ServiceProblem>,
        observed_at: SystemTime,
    ) {
        debug_assert!(operation_is_terminal(state));
        let mut registry = lock_operations(&self.state);
        let should_finish = registry
            .entries
            .get(operation_id)
            .is_some_and(|status| !operation_is_terminal(status.state));
        if should_finish {
            let completed_at =
                monotonic_operation_timestamp(&mut registry.last_timestamp, observed_at);
            let status = registry
                .entries
                .get_mut(operation_id)
                .expect("the active operation remains registered");
            status.state = state;
            status.completed_at = completed_at;
            status.result = result;
            status.error = error;
        }
    }

    pub(crate) fn get(&self, operation_id: &str) -> Option<OperationStatus> {
        lock_operations(&self.state)
            .entries
            .get(operation_id)
            .cloned()
    }
}

fn lock_operations(
    operations: &Mutex<OperationRegistryState>,
) -> MutexGuard<'_, OperationRegistryState> {
    match operations.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

const fn operation_is_terminal(state: OperationState) -> bool {
    matches!(
        state,
        OperationState::Succeeded | OperationState::Failed | OperationState::OutcomeUnknown
    )
}
