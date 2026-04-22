use mli_types::{ThreadStatus, TurnStatus};

pub fn can_transition_thread(from: &ThreadStatus, to: &ThreadStatus) -> bool {
    use ThreadStatus::*;
    matches!(
        (from, to),
        (NotLoaded, Starting)
            | (Starting, Idle)
            | (Idle, Running)
            | (Running, WaitingApproval)
            | (WaitingApproval, Running)
            | (Running, Idle)
            | (Running, Interrupted)
            | (Interrupted, Idle)
            | (_, Error)
    ) || from == to
}

pub fn can_transition_turn(from: &TurnStatus, to: &TurnStatus) -> bool {
    use TurnStatus::*;
    matches!(
        (from, to),
        (Pending, Starting)
            | (Starting, Streaming)
            | (Streaming, WaitingApproval)
            | (WaitingApproval, Streaming)
            | (Streaming, Completed)
            | (Streaming, Interrupted)
            | (Starting, Failed)
            | (Streaming, Failed)
    ) || from == to
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_state_machine_accepts_documented_transitions() {
        assert!(can_transition_thread(
            &ThreadStatus::NotLoaded,
            &ThreadStatus::Starting
        ));
        assert!(can_transition_thread(
            &ThreadStatus::Starting,
            &ThreadStatus::Idle
        ));
        assert!(can_transition_thread(
            &ThreadStatus::Running,
            &ThreadStatus::WaitingApproval
        ));
        assert!(can_transition_thread(
            &ThreadStatus::Interrupted,
            &ThreadStatus::Idle
        ));
    }

    #[test]
    fn thread_state_machine_rejects_invalid_jump() {
        assert!(!can_transition_thread(
            &ThreadStatus::Idle,
            &ThreadStatus::Starting
        ));
        assert!(!can_transition_thread(
            &ThreadStatus::WaitingApproval,
            &ThreadStatus::Idle
        ));
    }

    #[test]
    fn turn_state_machine_accepts_documented_transitions() {
        assert!(can_transition_turn(
            &TurnStatus::Pending,
            &TurnStatus::Starting
        ));
        assert!(can_transition_turn(
            &TurnStatus::Starting,
            &TurnStatus::Streaming
        ));
        assert!(can_transition_turn(
            &TurnStatus::Streaming,
            &TurnStatus::Completed
        ));
        assert!(can_transition_turn(
            &TurnStatus::Streaming,
            &TurnStatus::Interrupted
        ));
    }

    #[test]
    fn turn_state_machine_rejects_invalid_jump() {
        assert!(!can_transition_turn(
            &TurnStatus::Pending,
            &TurnStatus::Completed
        ));
        assert!(!can_transition_turn(
            &TurnStatus::Completed,
            &TurnStatus::Streaming
        ));
    }
}
