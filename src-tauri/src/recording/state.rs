//! The recording state machine. Only the transitions listed here are legal; anything else is a bug and is refused.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordingState {
    /// Nothing is recording.
    Idle,
    /// Devices are being opened, files created.
    Starting,
    Recording,
    Paused,
    /// Capture has ended; encoders are being closed.
    Stopping,
    /// Tracks are being mixed into the final file.
    Finalizing,
    Complete,
    Failed,
    /// The user discarded the recording.
    Cancelled,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("a recording cannot go from {from:?} to {to:?}")]
pub struct InvalidTransition {
    pub from: RecordingState,
    pub to: RecordingState,
}

impl RecordingState {
    pub fn can_become(self, to: RecordingState) -> bool {
        use RecordingState::*;
        matches!(
            (self, to),
            (Idle, Starting)
                | (Starting, Recording | Failed | Cancelled)
                | (Recording, Paused | Stopping | Failed | Cancelled)
                | (Paused, Recording | Stopping | Failed | Cancelled)
                | (Stopping, Finalizing | Failed)
                | (Finalizing, Complete | Failed)
        )
    }

    /// True while devices may be running or files are being written.
    pub fn is_active(self) -> bool {
        use RecordingState::*;
        matches!(self, Starting | Recording | Paused | Stopping | Finalizing)
    }

    pub fn is_terminal(self) -> bool {
        use RecordingState::*;
        matches!(self, Complete | Failed | Cancelled)
    }

    /// The value stored in `meetings.status`.
    pub fn db_status(self) -> &'static str {
        use RecordingState::*;
        match self {
            Idle | Starting | Recording | Paused => "recording",
            Stopping | Finalizing => "finalizing",
            Complete => "complete",
            Failed => "failed",
            Cancelled => "cancelled",
        }
    }
}

#[derive(Debug)]
pub struct StateMachine {
    state: RecordingState,
}

impl StateMachine {
    pub fn new() -> Self {
        Self {
            state: RecordingState::Idle,
        }
    }

    pub fn state(&self) -> RecordingState {
        self.state
    }

    pub fn transition(&mut self, to: RecordingState) -> Result<RecordingState, InvalidTransition> {
        if !self.state.can_become(to) {
            return Err(InvalidTransition {
                from: self.state,
                to,
            });
        }
        self.state = to;
        Ok(to)
    }
}

impl Default for StateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::RecordingState::*;
    use super::*;

    const ALL: [RecordingState; 9] = [
        Idle, Starting, Recording, Paused, Stopping, Finalizing, Complete, Failed, Cancelled,
    ];

    #[test]
    fn a_normal_recording_walks_through_the_expected_states() {
        let mut machine = StateMachine::new();

        for next in [
            Starting, Recording, Paused, Recording, Stopping, Finalizing, Complete,
        ] {
            machine.transition(next).unwrap();
        }

        assert_eq!(machine.state(), Complete);
    }

    #[test]
    fn exactly_the_listed_transitions_are_legal() {
        let legal: Vec<(RecordingState, RecordingState)> = vec![
            (Idle, Starting),
            (Starting, Recording),
            (Starting, Failed),
            (Starting, Cancelled),
            (Recording, Paused),
            (Recording, Stopping),
            (Recording, Failed),
            (Recording, Cancelled),
            (Paused, Recording),
            (Paused, Stopping),
            (Paused, Failed),
            (Paused, Cancelled),
            (Stopping, Finalizing),
            (Stopping, Failed),
            (Finalizing, Complete),
            (Finalizing, Failed),
        ];

        for from in ALL {
            for to in ALL {
                assert_eq!(
                    from.can_become(to),
                    legal.contains(&(from, to)),
                    "{from:?} -> {to:?}"
                );
            }
        }
    }

    #[test]
    fn terminal_states_never_change() {
        for terminal in [Complete, Failed, Cancelled] {
            assert!(terminal.is_terminal());
            for to in ALL {
                assert!(!terminal.can_become(to), "{terminal:?} -> {to:?}");
            }
        }
    }

    #[test]
    fn an_illegal_transition_is_refused_and_changes_nothing() {
        let mut machine = StateMachine::new();

        let error = machine.transition(Recording).unwrap_err();

        assert_eq!(
            error,
            InvalidTransition {
                from: Idle,
                to: Recording
            }
        );
        assert_eq!(machine.state(), Idle);
    }

    #[test]
    fn activity_and_db_status_match_the_states() {
        assert!(!Idle.is_active());
        for state in [Starting, Recording, Paused, Stopping, Finalizing] {
            assert!(state.is_active(), "{state:?}");
        }
        for state in [Complete, Failed, Cancelled] {
            assert!(!state.is_active(), "{state:?}");
        }
        assert_eq!(Paused.db_status(), "recording");
        assert_eq!(Stopping.db_status(), "finalizing");
        assert_eq!(Complete.db_status(), "complete");
    }
}
