use poise::serenity_prelude::{self as serenity, HttpError};

use crate::util::truncate_bytes;

const UNKNOWN_CHANNEL_CODE: isize = 10003;
const BODY_TRUNCATE_BYTES: usize = 512;

pub fn is_channel_gone(err: &serenity::Error) -> bool {
    matches!(
        err,
        serenity::Error::Http(HttpError::UnsuccessfulRequest(resp))
            if resp.error.code == UNKNOWN_CHANNEL_CODE
    )
}

pub struct ErrorParts {
    pub status: u16,
    pub code: isize,
    pub message: String,
    body: String,
}

impl ErrorParts {
    pub fn body_truncated(&self) -> String {
        truncate_bytes(&self.body, BODY_TRUNCATE_BYTES)
    }
}

pub fn error_parts(err: &serenity::Error) -> Option<ErrorParts> {
    if let serenity::Error::Http(HttpError::UnsuccessfulRequest(resp)) = err {
        Some(ErrorParts {
            status: resp.status_code.as_u16(),
            code: resp.error.code,
            message: resp.error.message.clone(),
            body: format!("{:?}", resp.error),
        })
    } else {
        None
    }
}
