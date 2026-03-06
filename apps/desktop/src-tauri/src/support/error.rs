use thiserror::Error;

use crate::domain::models::CommandError;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("path does not exist: {0}")]
    PathNotFound(String),
    #[error("not a git repository: {0}")]
    NotGitRepository(String),
    #[error("git command failed: {0}")]
    GitCommand(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::PathNotFound(_) => "path_not_found",
            Self::NotGitRepository(_) => "not_git_repository",
            Self::GitCommand(_) => "git_command_failed",
            Self::InvalidArgument(_) => "invalid_argument",
            Self::Parse(_) => "parse_error",
            Self::Io(_) => "io_error",
            Self::Db(_) => "database_error",
        }
    }

    pub fn into_command_error(self) -> CommandError {
        CommandError {
            code: self.code().to_string(),
            message: self.to_string(),
        }
    }
}
