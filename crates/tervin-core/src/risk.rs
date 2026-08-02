//! Risk vocabulary shared by the rules engine, the event stream, and the UI.

use serde::{Deserialize, Serialize};

/// How much a user should slow down before allowing something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Read-only, reversible, scoped to the working tree.
    Low,
    /// Writes files or mutates local state that Git can recover.
    Moderate,
    /// Touches history, credentials, networks, or state Git cannot recover.
    High,
    /// Irreversible, or reaches production or shared systems.
    Critical,
}

impl RiskLevel {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Moderate => "Moderate",
            Self::High => "High",
            Self::Critical => "Critical",
        }
    }

    /// Semantic colour role. Low risk deliberately gets no colour at all — the
    /// interface should not light up for ordinary work.
    pub fn tone(&self) -> &'static str {
        match self {
            Self::Low => "muted",
            Self::Moderate => "amber",
            Self::High => "amber",
            Self::Critical => "red",
        }
    }

    /// Whether this level always requires an explicit, contextual decision.
    pub fn always_confirm(&self) -> bool {
        matches!(self, Self::High | Self::Critical)
    }
}

/// What kind of blast radius an action has. An action can carry several.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskCategory {
    /// Deletes or overwrites data in a way that is not recoverable locally.
    Destructive,
    /// Rewrites or discards Git history or working-tree state.
    GitHistory,
    /// Reads, writes, or transmits credentials or key material.
    Credentials,
    /// Reaches the network, including uploads to unknown destinations.
    Network,
    /// Targets a production or shared environment.
    Production,
    /// Escalates privilege.
    Privilege,
    /// Publishes to a registry or public channel.
    Publishing,
    /// Mutates a database outside a migration.
    Database,
    /// Terminates processes outside the current session's scope.
    ProcessControl,
    /// Changes machine or account configuration outside the project.
    SystemConfig,
}

impl RiskCategory {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Destructive => "Destructive",
            Self::GitHistory => "Git history",
            Self::Credentials => "Credentials",
            Self::Network => "Network",
            Self::Production => "Production",
            Self::Privilege => "Privilege escalation",
            Self::Publishing => "Publishing",
            Self::Database => "Database",
            Self::ProcessControl => "Process control",
            Self::SystemConfig => "System configuration",
        }
    }
}

/// The full, showable result of classifying an action.
///
/// Every field exists because an approval request must show it: the exact
/// action, why it is being asked about, what it will touch, and whether Tervin
/// can actually stop it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskAssessment {
    pub level: RiskLevel,
    pub categories: Vec<RiskCategory>,
    /// Plain-language reasons, one per matched signal. Shown verbatim.
    pub reasons: Vec<String>,
    /// Concrete predicted side effects, where they can be stated honestly.
    pub side_effects: Vec<String>,
    /// The rule that produced this assessment, when a named rule matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_rule: Option<String>,
    /// True when Tervin holds a real gate. False means the action can proceed
    /// regardless of what the user chooses here, and the UI must not pretend
    /// otherwise.
    pub enforceable: bool,
}

impl RiskAssessment {
    /// An unremarkable action: low risk, nothing to warn about.
    pub fn benign() -> Self {
        Self {
            level: RiskLevel::Low,
            categories: Vec::new(),
            reasons: Vec::new(),
            side_effects: Vec::new(),
            matched_rule: None,
            enforceable: true,
        }
    }

    /// Used when classification itself is not possible, e.g. an opaque action
    /// from a Tier 3 runtime. Deliberately not "low".
    pub fn unclassifiable(reason: impl Into<String>) -> Self {
        Self {
            level: RiskLevel::Moderate,
            categories: Vec::new(),
            reasons: vec![reason.into()],
            side_effects: vec!["Unknown — Tervin could not inspect this action.".to_string()],
            matched_rule: None,
            enforceable: false,
        }
    }

    pub fn requires_confirmation(&self) -> bool {
        self.level.always_confirm()
    }
}
