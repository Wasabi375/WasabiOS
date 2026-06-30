use std::{any::Any, ffi::OsStr, marker::PhantomData, sync::Arc};

use congen::{
    Configuration,
    internal::{
        ChangeVerb, CongenChange, CongenInternal, Description, NotSupported, ParseError, VerbError,
    },
};
use serde::{Deserialize, Serialize};

pub trait IdType {
    fn type_name() -> &'static str;
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize, Hash)]
pub struct Id<T: IdType> {
    inner: Arc<str>,
    #[serde(skip)]
    typ: PhantomData<T>,
}

impl<T: IdType> Id<T> {
    pub fn new<S: AsRef<str>>(id: S) -> Self {
        Self {
            inner: id.as_ref().into(),
            typ: PhantomData,
        }
    }
    pub fn as_str(&self) -> &str {
        self.inner.as_ref()
    }
}

impl<T: IdType> std::fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Id")
            .field("inner", &self.inner)
            .field("typ", &self.typ)
            .finish()
    }
}

impl<T: IdType> std::fmt::Display for Id<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{}({})", T::type_name(), self.inner))
    }
}

impl<T: IdType> From<Arc<str>> for Id<T> {
    fn from(value: Arc<str>) -> Self {
        Self {
            inner: value,
            typ: PhantomData,
        }
    }
}

impl<T: IdType> From<String> for Id<T> {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl<T: IdType> From<&str> for Id<T> {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl<T: IdType> AsRef<str> for Id<T> {
    fn as_ref(&self) -> &str {
        self.inner.as_ref()
    }
}

impl<T: IdType> Id<T> {
    pub fn into_inner(self) -> Arc<str> {
        self.inner
    }
}

#[derive(Debug, Default, Clone)]
pub enum IdChange<T> {
    Apply(T),
    #[default]
    NoChange,
}

impl<T> IdChange<T> {
    pub fn unwrap(self) -> T {
        match self {
            IdChange::Apply(v) => v,
            IdChange::NoChange => panic!("called unwrap on WrapperChange::NoChange"),
        }
    }
}

impl<T> From<Option<T>> for IdChange<T> {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => Self::Apply(value),
            None => Self::NoChange,
        }
    }
}

impl<T: IdType + 'static> Configuration for Id<T> {}
impl<T: IdType + 'static> CongenInternal for Id<T> {
    type CongenChange = IdChange<Id<T>>;

    fn apply_change_with_inner_default(
        &mut self,
        change: Self::CongenChange,
        _inner_default: Option<fn() -> Box<dyn Any>>,
    ) {
        if let IdChange::Apply(value) = change {
            *self = value;
        }
    }

    fn description(field_name: &'static str) -> Description {
        <String as CongenInternal>::description(field_name)
    }
}

impl<T: IdType + 'static> CongenChange for IdChange<Id<T>> {
    type Configuration = Id<T>;

    fn empty() -> Self {
        IdChange::NoChange
    }

    fn parse(input: &OsStr) -> Result<Result<Self, ParseError>, NotSupported> {
        let inner = match <Option<String> as CongenChange>::parse(input)? {
            Ok(inner) => inner,
            Err(parse_err) => return Ok(Err(parse_err)),
        };
        Ok(Ok(inner.map(|inner| <Id<T>>::from(inner)).into()))
    }

    fn apply_change(&mut self, change: Self) {
        if let IdChange::Apply(new_change) = change {
            *self = IdChange::Apply(new_change)
        }
    }

    fn from_path_and_verb<'a, P>(mut path: P, verb: ChangeVerb) -> Result<Self, VerbError>
    where
        P: Iterator<Item = &'a str>,
    {
        assert!(
            path.next().is_none(),
            "OptionChange<Option<T>> implies this is a field"
        );
        match verb {
            ChangeVerb::Set(unparesd) => Ok(Self::parse(&unparesd)??),
            ChangeVerb::SetAny(value) => Ok(IdChange::Apply(
                *value.downcast().map_err(|_| VerbError::DowncastFailed)?,
            )),
            ChangeVerb::UseDefault
            | ChangeVerb::SetFlag
            | ChangeVerb::Unset
            | ChangeVerb::List(_) => Err(VerbError::UnsupportedVerb(verb)),
        }
    }

    fn unwrap_field(self) -> Result<Self::Configuration, Self> {
        Ok(self.unwrap())
    }
}

#[macro_export]
macro_rules! config_id_type {
    ($typ:ident, $marker:ident) => {
        #[derive(Clone, Copy, Hash, Eq, PartialEq, Ord, PartialOrd)]
        #[doc(hidden)]
        pub enum $marker {}
        impl std::default::Default for $marker {
            fn default() -> Self {
                unreachable!()
            }
        }
        impl crate::config::id::IdType for $marker {
            fn type_name() -> &'static str {
                stringify!($typ)
            }
        }
        pub type $typ = crate::config::id::Id<$marker>;
    };
}
