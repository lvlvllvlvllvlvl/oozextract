use std::error::Error;
use std::fmt::{Display, Formatter};
use std::panic::Location;

#[derive(Debug)]
pub struct OozError {
    pub message: Option<String>,
    pub context: Option<String>,
    pub source: Option<Box<dyn Error>>,
    pub location: &'static Location<'static>,
}

impl Error for OozError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref()
    }
}

impl Display for OozError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DataError on line {}", self.location)?;
        if let Some(context) = &self.context {
            write!(f, " ({})", context)?
        }
        if let Some(message) = &self.message {
            write!(f, ": {}", message)?
        }
        if let Some(cause) = &self.source {
            write!(f, "\ncaused by {}", cause)?
        }
        Ok(())
    }
}

impl From<ErrorBuilder> for OozError {
    #[track_caller]
    fn from(
        ErrorBuilder {
            message,
            context,
            source,
        }: ErrorBuilder,
    ) -> Self {
        Self {
            message,
            context,
            source,
            location: Location::caller(),
        }
    }
}

#[derive(Default)]
pub(crate) struct ErrorBuilder {
    pub message: Option<String>,
    pub context: Option<String>,
    pub source: Option<Box<dyn Error>>,
}

pub trait ResultBuilder<T> {
    fn message<F: FnOnce(Option<&str>) -> String>(self, msg: F) -> Result<T, ErrorBuilder>;
}

impl<T> ResultBuilder<T> for Result<T, ErrorBuilder> {
    fn message<F: FnOnce(Option<&str>) -> String>(self, msg: F) -> Self {
        match self {
            Ok(v) => Ok(v),
            Err(e) => Err(ErrorBuilder {
                message: Some(msg(e.message.as_ref().map(String::as_str))),
                ..Default::default()
            }),
        }
    }
}

impl<T> ResultBuilder<T> for Option<T> {
    fn message<F: FnOnce(Option<&str>) -> String>(self, msg: F) -> Result<T, ErrorBuilder> {
        match self {
            Some(v) => Ok(v),
            None => Err(ErrorBuilder {
                message: Some(msg(None)),
                ..Default::default()
            }),
        }
    }
}

pub(crate) trait WithContext<T, E: Error, C: ErrorContext> {
    fn at(self, context: &mut C) -> Result<T, ErrorBuilder>;
}

impl<T, E: Error + 'static, C: ErrorContext> WithContext<T, E, C> for Result<T, E> {
    fn at(self, context: &mut C) -> Result<T, ErrorBuilder> {
        self.map_err(|e| ErrorBuilder {
            context: context.describe(),
            source: Some(Box::new(e)),
            ..Default::default()
        })
    }
}

pub(crate) trait ErrorContext {
    fn describe(&mut self) -> Option<String> {
        None
    }

    fn raise<T>(&mut self, msg: String) -> Result<T, ErrorBuilder> {
        Err(ErrorBuilder {
            message: Some(msg),
            context: self.describe(),
            ..Default::default()
        })
    }

    fn slice_mut<'a, T>(
        &mut self,
        slice: &'a mut [T],
        start: usize,
        end: End,
    ) -> Result<&'a mut [T], ErrorBuilder> {
        let len = slice.len();
        match end {
            End::Idx(i) => slice.get_mut(start..i),
            End::Len(l) => slice.get_mut(start..start + l),
            //End::Open => slice.get_mut(start..),
        }
        .ok_or_else(|| ErrorBuilder {
            message: Some(format!(
                "Error getting {}..{:?} from slice with length {}",
                start, end, len
            )),
            context: self.describe(),
            ..Default::default()
        })
    }

    fn assert(&mut self, v: bool, msg: &str) -> Result<(), ErrorBuilder> {
        if v {
            Ok(())
        } else {
            self.raise(msg.into())
        }
    }

    fn assert_le<T: PartialOrd + Display>(&mut self, l: T, r: T) -> Result<(), ErrorBuilder> {
        if l <= r {
            Ok(())
        } else {
            self.raise(format!("Expected {} <= {}", l, r))
        }
    }

    fn assert_lt<T: PartialOrd + Display>(&mut self, l: T, r: T) -> Result<(), ErrorBuilder> {
        if l < r {
            Ok(())
        } else {
            self.raise(format!("Expected {} < {}", l, r))
        }
    }

    fn assert_eq<T: PartialOrd + Display>(&mut self, l: T, r: T) -> Result<(), ErrorBuilder> {
        if l == r {
            Ok(())
        } else {
            self.raise(format!("Expected {} == {}", l, r))
        }
    }

    fn assert_ne<T: PartialOrd + Display>(&mut self, l: T, r: T) -> Result<(), ErrorBuilder> {
        if l != r {
            Ok(())
        } else {
            self.raise(format!("Expected {} != {}", l, r))
        }
    }
}

#[derive(Debug)]
pub(crate) enum End {
    Idx(usize),
    Len(usize),
}
