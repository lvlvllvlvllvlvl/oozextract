use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::ops::Deref;
use std::panic::Location;

#[derive(Debug)]
pub struct OozError {
    pub message: Option<String>,
    pub context: Option<String>,
    pub source: Option<Box<dyn Error + Send + Sync>>,
    pub location: &'static Location<'static>,
}

pub type Res<T> = Result<T, OozError>;

impl Error for OozError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self.source {
            Some(ref err) => Some(err.deref()),
            None => None,
        }
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

impl From<OozError> for std::io::Error {
    fn from(value: OozError) -> Self {
        std::io::Error::new(std::io::ErrorKind::InvalidData, value)
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
    pub source: Option<Box<dyn Error + Send + Sync>>,
}

pub trait ResultBuilder<T>: Sized {
    fn message<F: FnOnce(Option<&str>) -> String>(self, msg: F) -> Result<T, ErrorBuilder>;
    fn err(self) -> Result<T, ErrorBuilder>;
    fn msg_of<M: Debug>(self, msg: &M) -> Result<T, ErrorBuilder> {
        self.message(|_| format!("{:?}", msg))
    }
}

impl<T> ResultBuilder<T> for Result<T, ErrorBuilder> {
    fn message<F: FnOnce(Option<&str>) -> String>(self, msg: F) -> Self {
        match self {
            Ok(v) => Ok(v),
            Err(e) => Err(ErrorBuilder {
                message: Some(msg(e.message.as_deref())),
                ..e
            }),
        }
    }

    fn err(self) -> Result<T, ErrorBuilder> {
        self
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

    fn err(self) -> Result<T, ErrorBuilder> {
        match self {
            Some(v) => Ok(v),
            None => Err(Default::default()),
        }
    }
}

pub(crate) trait WithContext<T, E: Error, C: ErrorContext> {
    fn at(self, context: &C) -> Result<T, ErrorBuilder>;
}

impl<T, E: Error + 'static + Send + Sync, C: ErrorContext> WithContext<T, E, C> for Result<T, E> {
    fn at(self, context: &C) -> Result<T, ErrorBuilder> {
        self.map_err(|e| ErrorBuilder {
            context: context.describe(),
            source: Some(Box::new(e)),
            ..Default::default()
        })
    }
}

pub(crate) trait ErrorContext {
    fn describe(&self) -> Option<String> {
        None
    }

    fn raise<T>(&self, msg: String) -> Result<T, ErrorBuilder> {
        Err(ErrorBuilder {
            message: Some(msg),
            context: self.describe(),
            ..Default::default()
        })
    }

    fn slice_mut<'a, T>(
        &self,
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

pub(crate) trait SliceErrors<T> {
    fn get_copy(&self, i: usize) -> Result<T, ErrorBuilder>;
    fn slice_mut(&mut self, start: usize, end: End) -> Result<&mut [T], ErrorBuilder>;
}

impl<T: Copy> SliceErrors<T> for [T] {
    fn get_copy(&self, i: usize) -> Result<T, ErrorBuilder> {
        self.get(i).copied().ok_or_else(|| ErrorBuilder {
            message: Some(format!(
                "Error getting {} from slice with length {}",
                i,
                self.len()
            )),
            ..Default::default()
        })
    }
    fn slice_mut(&mut self, start: usize, end: End) -> Result<&mut Self, ErrorBuilder> {
        let len = self.len();
        match end {
            End::Idx(i) => self.get_mut(start..i),
            End::Len(l) => self.get_mut(start..start + l),
            //End::Open => slice.get_mut(start..),
        }
        .ok_or_else(|| ErrorBuilder {
            message: Some(format!(
                "Error getting {}..{:?} from slice with length {}",
                start, end, len
            )),
            ..Default::default()
        })
    }
}

#[derive(Debug)]
pub(crate) enum End {
    Idx(usize),
    Len(usize),
}
