//! HTTP byte source for a user-started model download.
//!
//! `cargo test` does not construct this type. Tests inject [`crate::services::ports::ModelSource`].

use std::io::Read;

use crate::common::error::AppError;
use crate::services::ports::ModelSource;

pub(crate) struct UreqModelSource;

impl ModelSource for UreqModelSource {
    fn open(&mut self, url: &str) -> Result<Box<dyn Read + Send>, AppError> {
        let response = ureq::get(url)
            .call()
            .map_err(|error| AppError::message("download model", error.to_string()))?;
        Ok(response.into_reader())
    }
}
