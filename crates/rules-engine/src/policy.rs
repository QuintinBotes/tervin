//! Tervin Rules: policy, approvals, and audit.
//!
//! The engine is provider-neutral. It decides about *actions*, not about agents,
//! so the same policy governs a command the user re-ran from a Block, a command
//! an agent proposed, and a workflow step.
//!
//! Two decisions here are deliberate and load-bearing:
//!
//! **Grants are keyed on the exact action.** Approving `rm -rf build` for a
//! workspace does not approve `rm -rf /`. A looser key — the program name, a
//! prefix — would turn one considered decision into a standing licence for a
//! whole family of commands, which is precisely how approval fatigue becomes a
//! security incident.
//!
//! **Enforceability is tracked separately from risk.** Tervin genuinely gates
//! actions routed through it. For a runtime that acts on its own, Tervin can
//! observe and interrupt but not pre-empt, and every such request is marked
//! unenforceable so the UI can say so rather than implying a gate that is not
//! there.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tervin_core::{RequestId, RiskAssessment, ThreadId, Timestamp};

/// How far an approval extends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum ApprovalScope {
    /// This one execution only.
    Once,
    /// Every identical action in one Thread.
    Task { thread_id: ThreadId },
    /// Every identical action in this workspace, until Tervin restarts.
    Workspace,
}

impl ApprovalScope {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::Task { .. } => "this task",
            Self::Workspace => "this workspace",
        }
    }
}

/// What a rule does when it matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Run without prompting.
    Allow,
    /// Always ask, even if classification says the action is ordinary.
    RequireApproval,
    /// Never run.
    Deny,
}

/// How a rule matches an action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "match", rename_all = "snake_case")]
pub enum Pattern {
    /// The whole command, after whitespace normalisation.
    Exact { command: String },
    /// The command begins with this text.
    Prefix { prefix: String },
    /// The program being run, ignoring its path.
    Program { program: String },
    /// A regular expression over the command line.
    Regex { pattern: String },
}

impl Pattern {
    fn matches(&self, command: &str) -> bool {
        let normalised = normalise(command);
        match self {
            Self::Exact { command: c } => normalise(c) == normalised,
            Self::Prefix { prefix } => normalised.starts_with(&normalise(prefix)),
            Self::Program { program } => crate::classify::split_segments(command)
                .iter()
                .any(|s| &s.program == program),
            // An invalid pattern matches nothing rather than everything: a
            // typo in a rule must never silently widen what is allowed.
            Self::Regex { pattern } => regex::Regex::new(pattern)
                .map(|re| re.is_match(command))
                .unwrap_or(false),
        }
    }
}

/// A user- or policy-defined rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRule {
    pub id: String,
    pub name: String,
    pub pattern: Pattern,
    pub effect: Effect,
    /// Shown verbatim when the rule decides something.
    pub reason: String,
    pub enabled: bool,
}

impl PolicyRule {
    pub fn new(name: impl Into<String>, pattern: Pattern, effect: Effect) -> Self {
        let name = name.into();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            reason: format!("Matched rule “{name}”."),
            name,
            pattern,
            effect,
            enabled: true,
        }
    }
}

/// The context an action runs in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionContext {
    pub cwd: String,
    pub host: String,
    pub thread_id: Option<ThreadId>,
    /// Who is asking: `user`, or a runtime id such as `claude-code`.
    pub actor: String,
    /// False when Tervin can observe the action but not prevent it.
    pub enforceable: bool,
}

impl ActionContext {
    pub fn user(cwd: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            host: "local".to_string(),
            thread_id: None,
            actor: "user".to_string(),
            enforceable: true,
        }
    }

    pub fn agent(
        runtime_id: impl Into<String>,
        cwd: impl Into<String>,
        thread_id: ThreadId,
        enforceable: bool,
    ) -> Self {
        Self {
            cwd: cwd.into(),
            host: "local".to_string(),
            thread_id: Some(thread_id),
            actor: runtime_id.into(),
            enforceable,
        }
    }
}

/// The engine's verdict on an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    /// Proceed without asking.
    Allow {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        matched_rule: Option<String>,
    },
    /// Ask the user, showing this request.
    ///
    /// Boxed because an `ApprovalRequest` carries everything a prompt must show and is
    /// an order of magnitude larger than the other variants. Every evaluation returns
    /// a `Decision`, and the overwhelming majority are `Allow` — so inlining the rare
    /// large case would make the common path pay for it.
    RequireApproval { request: Box<ApprovalRequest> },
    /// Refuse. Tervin will not run this.
    Deny {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        matched_rule: Option<String>,
    },
}

/// Everything an approval prompt must show.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: RequestId,
    /// The exact command, file operation, or tool action.
    pub action: String,
    pub kind: ActionKind,
    pub cwd: String,
    pub host: String,
    pub thread_id: Option<ThreadId>,
    pub actor: String,
    pub risk: RiskAssessment,
    /// Why this is being asked about at all.
    pub reason: String,
    /// False when a decision here cannot actually stop the action.
    pub interceptable: bool,
    pub created_at: Timestamp,
    /// Scopes offered for this request. `Task` is absent outside a Thread.
    pub available_scopes: Vec<ApprovalScope>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Command,
    FileWrite,
    FileDelete,
    ToolCall,
    NetworkRequest,
}

/// What the user chose.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ApprovalOutcome {
    Approve {
        scope: ApprovalScope,
    },
    Deny {
        reason: Option<String>,
    },
    /// Approve, but run this text instead.
    EditAndRun {
        command: String,
    },
    /// Approve once and remember a standing rule.
    AddRule {
        rule: PolicyRule,
        run_now: bool,
    },
}

/// A standing approval, recorded after a decision.
#[derive(Debug, Clone)]
struct Grant {
    /// Normalised action text. Exact by design — see the module docs.
    key: String,
    scope: ApprovalScope,
}

/// Policy, approvals, and the in-flight request set.
pub struct RulesEngine {
    rules: RwLock<Vec<PolicyRule>>,
    grants: RwLock<Vec<Grant>>,
    pending: RwLock<HashMap<RequestId, ApprovalRequest>>,
}

impl Default for RulesEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl RulesEngine {
    pub fn new() -> Self {
        Self {
            rules: RwLock::new(Vec::new()),
            grants: RwLock::new(Vec::new()),
            pending: RwLock::new(HashMap::new()),
        }
    }

    pub fn rules(&self) -> Vec<PolicyRule> {
        self.rules.read().clone()
    }

    pub fn add_rule(&self, rule: PolicyRule) {
        self.rules.write().push(rule);
    }

    pub fn remove_rule(&self, id: &str) -> bool {
        let mut rules = self.rules.write();
        let before = rules.len();
        rules.retain(|r| r.id != id);
        rules.len() != before
    }

    pub fn set_rule_enabled(&self, id: &str, enabled: bool) -> bool {
        let mut rules = self.rules.write();
        match rules.iter_mut().find(|r| r.id == id) {
            Some(rule) => {
                rule.enabled = enabled;
                true
            }
            None => false,
        }
    }

    /// Decide what should happen to an action.
    pub fn evaluate(&self, action: &str, kind: ActionKind, ctx: &ActionContext) -> Decision {
        // 1. Denials win outright, before anything can grant an exception.
        let rules = self.rules.read();
        if let Some(rule) = rules
            .iter()
            .filter(|r| r.enabled && r.effect == Effect::Deny)
            .find(|r| r.pattern.matches(action))
        {
            return Decision::Deny {
                reason: rule.reason.clone(),
                matched_rule: Some(rule.name.clone()),
            };
        }

        // 2. A rule that always asks overrides both grants and classification.
        let forced_prompt = rules
            .iter()
            .filter(|r| r.enabled && r.effect == Effect::RequireApproval)
            .find(|r| r.pattern.matches(action))
            .cloned();
        drop(rules);

        if forced_prompt.is_none() {
            // 3. An existing grant covers this exact action in scope.
            if let Some(scope) = self.find_grant(action, ctx) {
                return Decision::Allow {
                    reason: format!("Approved for {} earlier.", scope.label()),
                    matched_rule: None,
                };
            }

            // 4. An allow rule.
            let rules = self.rules.read();
            if let Some(rule) = rules
                .iter()
                .filter(|r| r.enabled && r.effect == Effect::Allow)
                .find(|r| r.pattern.matches(action))
            {
                return Decision::Allow {
                    reason: rule.reason.clone(),
                    matched_rule: Some(rule.name.clone()),
                };
            }
        }

        // 5. Classify.
        let mut risk = crate::classify::classify(action, &ctx.cwd);
        if let Some(extra) = crate::classify::detect_piped_remote_execution(action) {
            if extra.level > risk.level {
                risk.level = extra.level;
                risk.categories.push(tervin_core::RiskCategory::Network);
                risk.reasons.push(extra.reason);
            }
        }
        // Tervin's gate is only real when Tervin is the one executing.
        risk.enforceable = ctx.enforceable;

        let must_ask = forced_prompt.is_some() || risk.requires_confirmation();
        if !must_ask {
            return Decision::Allow {
                reason: "No policy rule matched and the action is low risk.".to_string(),
                matched_rule: None,
            };
        }

        let mut available_scopes = vec![ApprovalScope::Once];
        if let Some(thread_id) = &ctx.thread_id {
            available_scopes.push(ApprovalScope::Task {
                thread_id: thread_id.clone(),
            });
        }
        available_scopes.push(ApprovalScope::Workspace);

        let reason = match &forced_prompt {
            Some(rule) => rule.reason.clone(),
            None => risk
                .reasons
                .first()
                .cloned()
                .unwrap_or_else(|| "This action needs review.".to_string()),
        };

        let request = ApprovalRequest {
            id: RequestId::new(),
            action: action.to_string(),
            kind,
            cwd: ctx.cwd.clone(),
            host: ctx.host.clone(),
            thread_id: ctx.thread_id.clone(),
            actor: ctx.actor.clone(),
            risk,
            reason,
            interceptable: ctx.enforceable,
            created_at: tervin_core::now(),
            available_scopes,
        };

        self.pending
            .write()
            .insert(request.id.clone(), request.clone());
        Decision::RequireApproval {
            request: Box::new(request),
        }
    }

    /// Record a decision and report what should now happen.
    ///
    /// Returns the command to run, if any. `EditAndRun` deliberately re-evaluates
    /// the edited text: approving `ls` and then editing it to `rm -rf /` must not
    /// inherit the original approval.
    pub fn resolve(&self, request_id: &RequestId, outcome: ApprovalOutcome) -> ResolveResult {
        let request = self.pending.write().remove(request_id);
        let Some(request) = request else {
            return ResolveResult::Unknown;
        };

        match outcome {
            ApprovalOutcome::Deny { reason } => ResolveResult::Denied {
                request,
                reason: reason.unwrap_or_else(|| "Denied by the user.".to_string()),
            },

            ApprovalOutcome::Approve { scope } => {
                if scope != ApprovalScope::Once {
                    self.grants.write().push(Grant {
                        key: normalise(&request.action),
                        scope: scope.clone(),
                    });
                }
                let command = request.action.clone();
                ResolveResult::Approved {
                    request,
                    command,
                    scope,
                }
            }

            ApprovalOutcome::EditAndRun { command } => {
                ResolveResult::ReEvaluate { request, command }
            }

            ApprovalOutcome::AddRule { rule, run_now } => {
                self.add_rule(rule.clone());
                if run_now {
                    let command = request.action.clone();
                    ResolveResult::Approved {
                        request,
                        command,
                        scope: ApprovalScope::Once,
                    }
                } else {
                    ResolveResult::Denied {
                        request,
                        reason: format!("Rule “{}” added; action not run.", rule.name),
                    }
                }
            }
        }
    }

    pub fn pending_requests(&self) -> Vec<ApprovalRequest> {
        let mut out: Vec<ApprovalRequest> = self.pending.read().values().cloned().collect();
        out.sort_by_key(|r| r.created_at);
        out
    }

    pub fn pending_count(&self) -> usize {
        self.pending.read().len()
    }

    /// Drop grants tied to a finished Thread.
    pub fn clear_task_grants(&self, thread_id: &ThreadId) {
        self.grants.write().retain(
            |g| !matches!(&g.scope, ApprovalScope::Task { thread_id: t } if t == thread_id),
        );
    }

    /// Drop every standing grant — the "forget approvals" action.
    pub fn clear_all_grants(&self) {
        self.grants.write().clear();
    }

    fn find_grant(&self, action: &str, ctx: &ActionContext) -> Option<ApprovalScope> {
        let key = normalise(action);
        self.grants
            .read()
            .iter()
            .find(|g| {
                g.key == key
                    && match &g.scope {
                        ApprovalScope::Workspace => true,
                        ApprovalScope::Task { thread_id } => {
                            ctx.thread_id.as_ref() == Some(thread_id)
                        }
                        ApprovalScope::Once => false,
                    }
            })
            .map(|g| g.scope.clone())
    }
}

/// What `resolve` concluded.
#[derive(Debug, Clone)]
pub enum ResolveResult {
    Approved {
        request: ApprovalRequest,
        command: String,
        scope: ApprovalScope,
    },
    Denied {
        request: ApprovalRequest,
        reason: String,
    },
    /// The action changed and must go back through `evaluate`.
    ReEvaluate {
        request: ApprovalRequest,
        command: String,
    },
    /// No such pending request — already resolved, or expired.
    Unknown,
}

/// Collapse whitespace so trivially different spellings share a grant key,
/// without loosening what the key actually identifies.
fn normalise(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A standing set of rules matching the always-confirm list.
///
/// These duplicate what the classifier already detects, on purpose: the
/// classifier is heuristic, and these make the policy explicit, visible in the
/// rules list, and impossible to lose to a parsing gap.
pub fn default_rules() -> Vec<PolicyRule> {
    let ask = |name: &str, pattern: Pattern, reason: &str| {
        let mut rule = PolicyRule::new(name, pattern, Effect::RequireApproval);
        rule.reason = reason.to_string();
        rule
    };

    vec![
        ask(
            "Privilege escalation",
            Pattern::Program {
                program: "sudo".to_string(),
            },
            "Runs with elevated privileges and can affect the whole system.",
        ),
        ask(
            "Force push",
            Pattern::Regex {
                pattern: r"git\s+push\b.*(--force\b|--mirror\b|\s-f\b)".to_string(),
            },
            "Overwrites history on the remote.",
        ),
        ask(
            "Hard reset",
            Pattern::Regex {
                pattern: r"git\s+reset\b.*--hard".to_string(),
            },
            "Discards uncommitted work irrecoverably.",
        ),
        ask(
            "Git clean",
            Pattern::Regex {
                pattern: r"git\s+clean\b.*-[a-zA-Z]*[fdx]".to_string(),
            },
            "Deletes untracked files that Git cannot restore.",
        ),
        ask(
            "Package publishing",
            Pattern::Regex {
                pattern: r"\b(npm|pnpm|yarn|cargo)\s+publish\b|\btwine\s+upload\b|\bgem\s+push\b"
                    .to_string(),
            },
            "Publishes a release that usually cannot be withdrawn.",
        ),
        ask(
            "SSH key changes",
            Pattern::Program {
                program: "ssh-keygen".to_string(),
            },
            "Changes how this machine authenticates.",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tervin_core::RiskLevel;

    fn engine() -> RulesEngine {
        RulesEngine::new()
    }

    fn ctx() -> ActionContext {
        ActionContext::user("/Users/dev/proj")
    }

    fn require(d: Decision) -> ApprovalRequest {
        match d {
            Decision::RequireApproval { request } => *request,
            other => panic!("expected an approval request, got {other:?}"),
        }
    }

    #[test]
    fn low_risk_actions_run_without_prompting() {
        let e = engine();
        assert!(matches!(
            e.evaluate("ls -la", ActionKind::Command, &ctx()),
            Decision::Allow { .. }
        ));
        assert_eq!(e.pending_count(), 0);
    }

    #[test]
    fn high_risk_actions_require_approval() {
        let e = engine();
        let req = require(e.evaluate("git push --force origin main", ActionKind::Command, &ctx()));
        assert_eq!(req.risk.level, RiskLevel::Critical);
        assert!(!req.risk.reasons.is_empty());
        assert!(!req.risk.side_effects.is_empty());
        assert_eq!(e.pending_count(), 1);
    }

    #[test]
    fn a_workspace_grant_does_not_cover_a_different_command() {
        // The core safety property: approving one deletion must never license
        // another. A program-level or prefix-level grant key would fail this.
        let e = engine();
        let req = require(e.evaluate("rm -rf build", ActionKind::Command, &ctx()));
        e.resolve(
            &req.id,
            ApprovalOutcome::Approve {
                scope: ApprovalScope::Workspace,
            },
        );

        // The same command is now allowed.
        assert!(matches!(
            e.evaluate("rm -rf build", ActionKind::Command, &ctx()),
            Decision::Allow { .. }
        ));
        // A different one is not.
        assert!(matches!(
            e.evaluate("rm -rf /", ActionKind::Command, &ctx()),
            Decision::RequireApproval { .. }
        ));
    }

    #[test]
    fn a_task_grant_does_not_leak_into_another_task() {
        let e = engine();
        let thread_a = ThreadId::new();
        let thread_b = ThreadId::new();
        let ctx_a = ActionContext::agent("claude-code", "/p", thread_a.clone(), true);
        let ctx_b = ActionContext::agent("claude-code", "/p", thread_b, true);

        let req = require(e.evaluate("rm -rf dist", ActionKind::Command, &ctx_a));
        e.resolve(
            &req.id,
            ApprovalOutcome::Approve {
                scope: ApprovalScope::Task {
                    thread_id: thread_a.clone(),
                },
            },
        );

        assert!(matches!(
            e.evaluate("rm -rf dist", ActionKind::Command, &ctx_a),
            Decision::Allow { .. }
        ));
        assert!(
            matches!(
                e.evaluate("rm -rf dist", ActionKind::Command, &ctx_b),
                Decision::RequireApproval { .. }
            ),
            "a grant for one task must not apply to another"
        );
    }

    #[test]
    fn approving_once_grants_nothing_standing() {
        let e = engine();
        let req = require(e.evaluate("rm -rf build", ActionKind::Command, &ctx()));
        e.resolve(
            &req.id,
            ApprovalOutcome::Approve {
                scope: ApprovalScope::Once,
            },
        );
        assert!(matches!(
            e.evaluate("rm -rf build", ActionKind::Command, &ctx()),
            Decision::RequireApproval { .. }
        ));
    }

    #[test]
    fn editing_a_command_sends_it_back_for_re_evaluation() {
        // Approving `ls` and editing it into something destructive must not
        // inherit the approval.
        let e = engine();
        let req = require(e.evaluate("rm -rf build", ActionKind::Command, &ctx()));
        let result = e.resolve(
            &req.id,
            ApprovalOutcome::EditAndRun {
                command: "rm -rf /".to_string(),
            },
        );
        match result {
            ResolveResult::ReEvaluate { command, .. } => {
                assert_eq!(command, "rm -rf /");
                // Re-evaluating the edit prompts again, at the higher level.
                let again = require(e.evaluate(&command, ActionKind::Command, &ctx()));
                assert_eq!(again.risk.level, RiskLevel::Critical);
            }
            other => panic!("expected re-evaluation, got {other:?}"),
        }
    }

    #[test]
    fn deny_rules_cannot_be_overridden_by_a_grant() {
        let e = engine();
        let mut rule = PolicyRule::new(
            "No force push",
            Pattern::Regex {
                pattern: r"git\s+push.*--force".to_string(),
            },
            Effect::Deny,
        );
        rule.reason = "Force pushing is disabled for this workspace.".to_string();
        e.add_rule(rule);

        // Even after a standing grant, the denial still wins.
        e.grants.write().push(Grant {
            key: normalise("git push --force origin main"),
            scope: ApprovalScope::Workspace,
        });

        match e.evaluate("git push --force origin main", ActionKind::Command, &ctx()) {
            Decision::Deny { reason, .. } => assert!(reason.contains("disabled")),
            other => panic!("expected denial, got {other:?}"),
        }
    }

    #[test]
    fn a_require_approval_rule_overrides_an_existing_grant() {
        let e = engine();
        e.grants.write().push(Grant {
            key: normalise("deploy.sh"),
            scope: ApprovalScope::Workspace,
        });
        e.add_rule(PolicyRule::new(
            "Always confirm deploys",
            Pattern::Prefix {
                prefix: "deploy.sh".to_string(),
            },
            Effect::RequireApproval,
        ));

        assert!(matches!(
            e.evaluate("deploy.sh", ActionKind::Command, &ctx()),
            Decision::RequireApproval { .. }
        ));
    }

    #[test]
    fn allow_rules_suppress_prompts_for_known_safe_actions() {
        let e = engine();
        e.add_rule(PolicyRule::new(
            "Project cleanup is fine",
            Pattern::Exact {
                command: "rm -rf build".to_string(),
            },
            Effect::Allow,
        ));
        assert!(matches!(
            e.evaluate("rm -rf   build", ActionKind::Command, &ctx()),
            Decision::Allow { .. }
        ));
    }

    #[test]
    fn an_invalid_regex_rule_matches_nothing() {
        // A typo in a rule must never widen what is permitted.
        let e = engine();
        e.add_rule(PolicyRule::new(
            "Broken",
            Pattern::Regex {
                pattern: "([unclosed".to_string(),
            },
            Effect::Allow,
        ));
        assert!(matches!(
            e.evaluate("rm -rf build", ActionKind::Command, &ctx()),
            Decision::RequireApproval { .. }
        ));
    }

    #[test]
    fn unenforceable_requests_are_marked_as_such() {
        // For a runtime Tervin cannot pre-empt, the prompt must not imply a gate.
        let e = engine();
        let ctx = ActionContext::agent("some-agent", "/p", ThreadId::new(), false);
        let req = require(e.evaluate("rm -rf build", ActionKind::Command, &ctx));
        assert!(!req.interceptable);
        assert!(!req.risk.enforceable);
    }

    #[test]
    fn task_scope_is_offered_only_inside_a_thread() {
        let e = engine();
        let user_req = require(e.evaluate("sudo ls", ActionKind::Command, &ctx()));
        assert!(!user_req
            .available_scopes
            .iter()
            .any(|s| matches!(s, ApprovalScope::Task { .. })));

        let agent_ctx = ActionContext::agent("claude-code", "/p", ThreadId::new(), true);
        let agent_req = require(e.evaluate("sudo ls", ActionKind::Command, &agent_ctx));
        assert!(agent_req
            .available_scopes
            .iter()
            .any(|s| matches!(s, ApprovalScope::Task { .. })));
    }

    #[test]
    fn clearing_task_grants_leaves_workspace_grants_alone() {
        let e = engine();
        let thread = ThreadId::new();
        e.grants.write().push(Grant {
            key: normalise("a"),
            scope: ApprovalScope::Task {
                thread_id: thread.clone(),
            },
        });
        e.grants.write().push(Grant {
            key: normalise("b"),
            scope: ApprovalScope::Workspace,
        });

        e.clear_task_grants(&thread);
        assert_eq!(e.grants.read().len(), 1);
        assert_eq!(e.grants.read()[0].key, "b");
    }

    #[test]
    fn adding_a_rule_without_running_does_not_execute() {
        let e = engine();
        let req = require(e.evaluate("rm -rf build", ActionKind::Command, &ctx()));
        let rule = PolicyRule::new(
            "Never clean build",
            Pattern::Exact {
                command: "rm -rf build".to_string(),
            },
            Effect::Deny,
        );
        let result = e.resolve(
            &req.id,
            ApprovalOutcome::AddRule {
                rule,
                run_now: false,
            },
        );
        assert!(matches!(result, ResolveResult::Denied { .. }));
        assert_eq!(e.rules().len(), 1);
        assert!(matches!(
            e.evaluate("rm -rf build", ActionKind::Command, &ctx()),
            Decision::Deny { .. }
        ));
    }

    #[test]
    fn resolving_an_unknown_request_is_not_an_approval() {
        let e = engine();
        assert!(matches!(
            e.resolve(
                &RequestId::new(),
                ApprovalOutcome::Approve {
                    scope: ApprovalScope::Workspace
                }
            ),
            ResolveResult::Unknown
        ));
        assert!(e.grants.read().is_empty());
    }

    #[test]
    fn default_rules_cover_the_always_confirm_list() {
        let e = engine();
        for rule in default_rules() {
            e.add_rule(rule);
        }
        for cmd in [
            "sudo rm x",
            "git push --force origin main",
            "git reset --hard HEAD",
            "git clean -fd",
            "npm publish",
            "ssh-keygen -t ed25519",
        ] {
            assert!(
                matches!(
                    e.evaluate(cmd, ActionKind::Command, &ctx()),
                    Decision::RequireApproval { .. }
                ),
                "{cmd} did not require approval"
            );
        }
    }
}
