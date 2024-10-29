use std::error::Error;
use std::fmt::{Debug, Display};

#[cfg(feature = "verbose_errors")]
pub struct OozError {
    pub message: Option<String>,
    pub context: Option<String>,
    pub source: Option<Box<dyn Error + Send + Sync>>,
    pub location: &'static std::panic::Location<'static>,
}

#[cfg(not(feature = "verbose_errors"))]
#[derive(Debug)]
pub struct OozError;

pub type Res<T> = Result<T, OozError>;

impl Error for OozError {
    #[cfg(feature = "verbose_errors")]
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self.source {
            Some(ref err) => Some(std::ops::Deref::deref(err)),
            None => None,
        }
    }
}

impl Display for OozError {
    #[cfg(feature = "verbose_errors")]
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(cause) = &self.source {
            Display::fmt(cause, f)?
        }
        if let Some(message) = &self.message {
            writeln!(f, "{}", message)?;
        }
        writeln!(f, "at {}", self.location)?;
        if let Some(context) = &self.context {
            writeln!(f, "  ({})", context)?
        }
        Ok(())
    }

    #[cfg(not(feature = "verbose_errors"))]
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OozError")
    }
}

#[cfg(feature = "verbose_errors")]
impl Debug for OozError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, r#"OozError("{}")"#, self)
    }
}

impl From<OozError> for std::io::Error {
    #[cfg(feature = "verbose_errors")]
    fn from(value: OozError) -> Self {
        std::io::Error::new(std::io::ErrorKind::InvalidData, value)
    }

    #[cfg(not(feature = "verbose_errors"))]
    fn from(_: OozError) -> Self {
        std::io::ErrorKind::InvalidData.into()
    }
}

impl From<std::io::Error> for OozError {
    #[cfg(feature = "verbose_errors")]
    #[track_caller]
    fn from(value: std::io::Error) -> Self {
        Self {
            message: Some(format!("IO error: {}", value)),
            location: std::panic::Location::caller(),
            context: None,
            source: None,
        }
    }

    #[cfg(not(feature = "verbose_errors"))]
    fn from(_: std::io::Error) -> Self {
        Self
    }
}

impl From<ErrorBuilder> for OozError {
    #[cfg(feature = "verbose_errors")]
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
            location: std::panic::Location::caller(),
        }
    }

    #[cfg(not(feature = "verbose_errors"))]
    fn from(_: ErrorBuilder) -> Self {
        Self
    }
}

#[derive(Default)]
pub(crate) struct ErrorBuilder {
    #[cfg(feature = "verbose_errors")]
    pub message: Option<String>,
    #[cfg(feature = "verbose_errors")]
    pub context: Option<String>,
    #[cfg(feature = "verbose_errors")]
    pub source: Option<Box<dyn Error + Send + Sync>>,
}

#[cfg(feature = "async")]
impl ErrorBuilder {
    pub fn invert<T, E: Error + 'static + Send + Sync>(
        option: Option<Result<T, E>>,
    ) -> Result<Option<T>, Self> {
        match option {
            Some(Ok(v)) => Ok(Some(v)),
            Some(Err(_err)) => Err(Self {
                #[cfg(feature = "verbose_errors")]
                message: None,
                #[cfg(feature = "verbose_errors")]
                context: None,
                #[cfg(feature = "verbose_errors")]
                source: Some(Box::new(_err)),
            })?,
            None => Ok(None),
        }
    }
}

pub trait ResultBuilder<T>: Sized {
    fn message<F: FnOnce(Option<&str>) -> String>(self, msg: F) -> Result<T, ErrorBuilder>;
    fn msg_of<M: Debug>(self, msg: &M) -> Result<T, ErrorBuilder> {
        self.message(|_| format!("{:?}", msg))
    }
    fn err(self) -> Result<T, ErrorBuilder>;
}

impl<T> ResultBuilder<T> for Result<T, ErrorBuilder> {
    fn message<F: FnOnce(Option<&str>) -> String>(self, _msg: F) -> Self {
        match self {
            Ok(v) => Ok(v),
            Err(e) => Err(ErrorBuilder {
                #[cfg(feature = "verbose_errors")]
                message: Some(_msg(e.message.as_deref())),
                ..e
            }),
        }
    }

    fn err(self) -> Result<T, ErrorBuilder> {
        self
    }
}

impl<T> ResultBuilder<T> for Option<T> {
    fn message<F: FnOnce(Option<&str>) -> String>(self, _msg: F) -> Result<T, ErrorBuilder> {
        match self {
            Some(v) => Ok(v),
            None => Err(ErrorBuilder {
                #[cfg(feature = "verbose_errors")]
                message: Some(_msg(None)),
                ..Default::default()
            }),
        }
    }

    fn msg_of<M: Debug>(self, _msg: &M) -> Result<T, ErrorBuilder> {
        match self {
            Some(v) => Ok(v),
            None => Err(ErrorBuilder {
                #[cfg(feature = "verbose_errors")]
                message: Some(format!("{:?}", _msg)),
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

pub(crate) trait WithContext<T, E, C: ErrorContext> {
    fn at(self, context: &C) -> Result<T, ErrorBuilder>;
}

impl<T, E: Error + 'static + Send + Sync, C: ErrorContext> WithContext<T, E, C> for Result<T, E> {
    fn at(self, _context: &C) -> Result<T, ErrorBuilder> {
        self.map_err(|_e| ErrorBuilder {
            #[cfg(feature = "verbose_errors")]
            context: _context.describe(),
            #[cfg(feature = "verbose_errors")]
            source: Some(Box::new(_e)),
            ..Default::default()
        })
    }
}

pub(crate) trait ErrorContext {
    #[allow(dead_code)]
    fn describe(&self) -> Option<String> {
        None
    }

    fn raise<T>(&self, _msg: String) -> Result<T, ErrorBuilder> {
        Err(ErrorBuilder {
            #[cfg(feature = "verbose_errors")]
            message: Some(_msg),
            #[cfg(feature = "verbose_errors")]
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
        let _len = slice.len();
        match end {
            End::Idx(i) => slice.get_mut(start..i),
            End::Len(l) => slice.get_mut(start..start + l),
            //End::Open => slice.get_mut(start..),
        }
        .ok_or_else(|| ErrorBuilder {
            #[cfg(feature = "verbose_errors")]
            message: Some(format!(
                "Error getting {}..{:?} from slice with length {}",
                start, end, _len
            )),
            #[cfg(feature = "verbose_errors")]
            context: self.describe(),
            ..Default::default()
        })
    }

    fn assert(&self, v: bool, msg: &str) -> Result<(), ErrorBuilder> {
        if v {
            Ok(())
        } else {
            self.raise(msg.into())
        }
    }

    fn assert_le<T: PartialOrd + Display>(&self, l: T, r: T) -> Result<(), ErrorBuilder> {
        if l <= r {
            Ok(())
        } else {
            self.raise(format!("Expected {} <= {}", l, r))
        }
    }

    fn assert_lt<T: PartialOrd + Display>(&self, l: T, r: T) -> Result<(), ErrorBuilder> {
        if l < r {
            Ok(())
        } else {
            self.raise(format!("Expected {} < {}", l, r))
        }
    }

    fn assert_eq<T: PartialOrd + Display>(&self, l: T, r: T) -> Result<(), ErrorBuilder> {
        if l == r {
            Ok(())
        } else {
            self.raise(format!("Expected {} == {}", l, r))
        }
    }

    fn assert_ne<T: PartialOrd + Display>(&self, l: T, r: T) -> Result<(), ErrorBuilder> {
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
            #[cfg(feature = "verbose_errors")]
            message: Some(format!(
                "Error getting {} from slice with length {}",
                i,
                self.len()
            )),
            ..Default::default()
        })
    }
    fn slice_mut(&mut self, start: usize, end: End) -> Result<&mut Self, ErrorBuilder> {
        let _len = self.len();
        match end {
            End::Idx(i) => self.get_mut(start..i),
            End::Len(l) => self.get_mut(start..start + l),
            //End::Open => slice.get_mut(start..),
        }
        .ok_or_else(|| ErrorBuilder {
            #[cfg(feature = "verbose_errors")]
            message: Some(format!(
                "Error getting {}..{:?} from slice with length {}",
                start, end, _len
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
