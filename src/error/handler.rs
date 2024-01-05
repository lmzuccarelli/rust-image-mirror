use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub struct MirrorError {
    details: String,
}

impl MirrorError {
    pub fn new(msg: &str) -> MirrorError {
        MirrorError {
            details: msg.to_string(),
        }
    }
}

impl fmt::Display for MirrorError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.details)
    }
}

impl Error for MirrorError {
    fn description(&self) -> &str {
        &self.details
    }
}
