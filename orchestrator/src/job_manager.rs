//! Job state machine and lifecycle management.

use audraflow_ipc::JobState;

/// Advance a job's state through its lifecycle.
/// Returns the new state.
#[allow(dead_code)]
pub fn advance_state(current: &JobState, next: JobStateTransition) -> JobState {
    match (current, next) {
        (JobState::Pending, JobStateTransition::Start) => JobState::Running,
        (JobState::Running, JobStateTransition::Pause) => JobState::Paused,
        (JobState::Paused, JobStateTransition::Resume) => JobState::Running,
        (JobState::Running, JobStateTransition::Complete) => JobState::Completed,
        (_, JobStateTransition::Fail) => JobState::Failed,
        (_, JobStateTransition::Cancel) => JobState::Cancelled,
        _ => current.clone(),
    }
}

/// Valid transitions for a job state.
#[allow(dead_code)]
pub enum JobStateTransition {
    Start,
    Pause,
    Resume,
    Complete,
    Fail,
    Cancel,
}
