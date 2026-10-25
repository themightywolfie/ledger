//! Who is asking: the [`Caller`] every engine operation takes.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::claim::{Scope, ScopeLevel};
use crate::error::{Error, Result};

/// Everything the engine needs to know about who is asking. Surfaces build it from an API key or
/// from launch arguments. The model never supplies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// The one tenant this caller may touch.
    pub tenant: String,
    /// Who is acting, such as `agent:claude-code`.
    pub actor: String,
    /// The path the caller's writes arrive through.
    pub channel: Channel,
    pub permissions: BTreeSet<Permission>,
    /// The widest scopes the caller may read and write.
    pub bounds: ScopeBounds,
}

/// The path a write arrived through (ADR-0009).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Agent,
    Integration,
    Visualizer,
    Cli,
    Elicitation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Read,
    Propose,
    Attest,
    Resolve,
    Forget,
    RelayUser,
    DeclareSource,
    Operator,
    Admin,
}

impl Permission {
    fn as_str(self) -> &'static str {
        match self {
            Permission::Read => "read",
            Permission::Propose => "propose",
            Permission::Attest => "attest",
            Permission::Resolve => "resolve",
            Permission::Forget => "forget",
            Permission::RelayUser => "relay_user",
            Permission::DeclareSource => "declare_source",
            Permission::Operator => "operator",
            Permission::Admin => "admin",
        }
    }
}

/// The user, project and agent scopes a caller is bound to. Org scope is always within bounds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeBounds {
    pub user: Option<String>,
    pub project: Option<String>,
    pub agent: Option<String>,
}

impl Caller {
    /// An agent acting on the agent channel, bound to its own agent scope and no permissions yet.
    pub fn agent(tenant: &str, agent: &str) -> Self {
        Caller {
            tenant: tenant.to_owned(),
            actor: format!("agent:{agent}"),
            channel: Channel::Agent,
            permissions: BTreeSet::new(),
            bounds: ScopeBounds {
                agent: Some(agent.to_owned()),
                ..ScopeBounds::default()
            },
        }
    }

    pub fn with_user(mut self, user: &str) -> Self {
        self.bounds.user = Some(user.to_owned());
        self
    }

    pub fn with_project(mut self, project: &str) -> Self {
        self.bounds.project = Some(project.to_owned());
        self
    }

    pub fn with_permissions(mut self, permissions: impl IntoIterator<Item = Permission>) -> Self {
        self.permissions = permissions.into_iter().collect();
        self
    }

    /// Fails unless the caller holds `permission`.
    pub(crate) fn require(&self, permission: Permission) -> Result<()> {
        if self.permissions.contains(&permission) {
            return Ok(());
        }
        Err(Error::PermissionDenied(format!(
            "{} lacks the {} permission",
            self.actor,
            permission.as_str()
        )))
    }

    /// Fails unless `scope` is one the caller may read.
    pub(crate) fn require_visible(&self, scope: &Scope) -> Result<()> {
        if self.visible_scopes().contains(scope) {
            return Ok(());
        }
        Err(Error::ScopeDenied(format!(
            "{scope} is outside the scope bounds of {}",
            self.actor
        )))
    }

    /// Where a write lands when the caller names no scope: the project if bound, else the user.
    pub(crate) fn default_write_scope(&self) -> Scope {
        match (&self.bounds.project, &self.bounds.user) {
            (Some(project), _) => Scope::new(ScopeLevel::Project, project),
            (None, Some(user)) => Scope::new(ScopeLevel::User, user),
            (None, None) => Scope::org(),
        }
    }

    /// Every scope the caller may read, widest first.
    pub(crate) fn visible_scopes(&self) -> Vec<Scope> {
        let bounded = [
            (ScopeLevel::User, &self.bounds.user),
            (ScopeLevel::Project, &self.bounds.project),
            (ScopeLevel::Agent, &self.bounds.agent),
        ];
        std::iter::once(Scope::org())
            .chain(
                bounded
                    .into_iter()
                    .filter_map(|(level, id)| id.as_ref().map(|id| Scope::new(level, id))),
            )
            .collect()
    }
}
