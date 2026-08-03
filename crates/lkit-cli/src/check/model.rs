use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Warning,
    Error,
    Unknown,
}

impl PartialOrd for Status {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Status {
    fn cmp(&self, other: &Self) -> Ordering {
        self.severity().cmp(&other.severity())
    }
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Warning => "WARN",
            Self::Error => "ERROR",
            Self::Unknown => "UNKNOWN",
        }
    }

    fn severity(self) -> u8 {
        match self {
            Self::Pass => 0,
            Self::Warning => 1,
            Self::Unknown => 2,
            Self::Error => 3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub id: &'static str,
    pub title: &'static str,
    pub status: Status,
    pub value: String,
    pub reason: String,
    pub suggestion: String,
    pub details: Vec<String>,
}

impl CheckResult {
    pub fn new(id: &'static str, title: &'static str) -> Self {
        Self {
            id,
            title,
            status: Status::Pass,
            value: String::new(),
            reason: String::new(),
            suggestion: String::new(),
            details: Vec::new(),
        }
    }

    pub fn set(
        mut self,
        status: Status,
        value: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        self.status = status;
        self.value = value.into();
        self.reason = reason.into();
        self
    }

    pub fn suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = suggestion.into();
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.details.push(detail.into());
        self
    }
}

#[derive(Debug, Clone)]
pub struct CheckGroup {
    pub title: &'static str,
    pub results: Vec<CheckResult>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct StatusCounts {
    pub pass: usize,
    pub warning: usize,
    pub error: usize,
    pub unknown: usize,
}

pub fn aggregate_status(statuses: impl IntoIterator<Item = Status>) -> Status {
    statuses.into_iter().max().unwrap_or(Status::Pass)
}

#[derive(Debug, Clone)]
pub struct CheckReport {
    pub groups: Vec<CheckGroup>,
    pub summary: Status,
    pub counts: StatusCounts,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_priority_is_error_over_unknown_over_warning_over_pass() {
        assert!(Status::Error > Status::Unknown);
        assert!(Status::Unknown > Status::Warning);
        assert!(Status::Warning > Status::Pass);
    }

    #[test]
    fn aggregate_picks_most_severe_status() {
        assert_eq!(
            aggregate_status([Status::Pass, Status::Warning, Status::Error]),
            Status::Error
        );
        assert_eq!(
            aggregate_status([Status::Pass, Status::Warning, Status::Unknown]),
            Status::Unknown
        );
        assert_eq!(
            aggregate_status([Status::Pass, Status::Warning]),
            Status::Warning
        );
        assert_eq!(aggregate_status([]), Status::Pass);
    }
}
