//! utilites for the primitive enum proc-macro

/// Error type used to denote that a given value is invalid for the operation
pub struct InvalidValue<T> {
    /// the invalid value
    pub value: T,
}
