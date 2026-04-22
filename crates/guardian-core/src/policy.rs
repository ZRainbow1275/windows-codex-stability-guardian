#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    C1,
    C2,
    C3,
    C4,
    C5,
    C6,
    D1,
    D2,
    D3,
    D4,
    P1,
    P2,
    P3,
    P4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomationLevel {
    Observe,
    Snapshot,
    SafeRepair,
    GuidedRecovery,
}

impl FailureClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::C1 => "C1",
            Self::C2 => "C2",
            Self::C3 => "C3",
            Self::C4 => "C4",
            Self::C5 => "C5",
            Self::C6 => "C6",
            Self::D1 => "D1",
            Self::D2 => "D2",
            Self::D3 => "D3",
            Self::D4 => "D4",
            Self::P1 => "P1",
            Self::P2 => "P2",
            Self::P3 => "P3",
            Self::P4 => "P4",
        }
    }
}

impl AutomationLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Snapshot => "snapshot",
            Self::SafeRepair => "safe_repair",
            Self::GuidedRecovery => "guided_recovery",
        }
    }
}
