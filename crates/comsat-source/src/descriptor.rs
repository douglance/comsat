use comsat_types::SourceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OperationKind {
    Search,
    Fetch,
    Follow,
}

impl OperationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Fetch => "fetch",
            Self::Follow => "follow",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProfile {
    pub search: bool,
    pub fetch: bool,
    pub follow: bool,
}

impl SourceProfile {
    pub const fn search_only() -> Self {
        Self {
            search: true,
            fetch: false,
            follow: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDescriptor {
    pub id: SourceId,
    pub display_name: String,
    pub profile: SourceProfile,
    pub commands: SourceCommandNames,
}

impl SourceDescriptor {
    pub fn new(id: SourceId, display_name: impl Into<String>, profile: SourceProfile) -> Self {
        let commands = SourceCommandNames::for_source(&id);
        Self {
            id,
            display_name: display_name.into(),
            profile,
            commands,
        }
    }

    pub const fn supports(&self, operation: OperationKind) -> bool {
        match operation {
            OperationKind::Search => self.profile.search,
            OperationKind::Fetch => self.profile.fetch,
            OperationKind::Follow => self.profile.follow,
        }
    }

    pub fn tool_name(&self, operation: OperationKind) -> Option<&str> {
        if !self.supports(operation) {
            return None;
        }
        Some(match operation {
            OperationKind::Search => &self.commands.search,
            OperationKind::Fetch => &self.commands.fetch,
            OperationKind::Follow => &self.commands.follow,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCommandNames {
    pub search: String,
    pub fetch: String,
    pub follow: String,
}

impl SourceCommandNames {
    pub fn for_source(source: &SourceId) -> Self {
        let source = source.as_str().replace('-', "_");
        Self {
            search: format!("comsat_{source}_search"),
            fetch: format!("comsat_{source}_fetch"),
            follow: format!("comsat_{source}_follow"),
        }
    }
}
