use std::panic::Location;

pub struct DataError {
    pub message: String,
    pub location: String,
}

#[track_caller]
pub fn data_error<T>(message: String) -> Result<T, DataError> {
    log::warn!("{}: {}", Location::caller(), message);
    Err(DataError {
        message,
        location: Location::caller().to_string(),
    })
}
