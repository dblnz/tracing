//! Span and `Event` key-value data.
//!
//! Spans and events may be annotated with key-value data, known as _fields_.
//! These fields consist of a mapping from a key (corresponding to a `&str` but
//! represented internally as an array index) to a [`Value`].
//!
//! # `Value`s and `Collect`s
//!
//! Collectors consume `Value`s as fields attached to [span]s or [`Event`]s.
//! The set of field keys on a given span or event is defined on its [`Metadata`].
//! When a span is created, it provides [`Attributes`] to the collector's
//! [`new_span`] method, containing any fields whose values were provided when
//! the span was created; and may call the collector's [`record`] method
//! with additional [`Record`]s if values are added for more of its fields.
//! Similarly, the [`Event`] type passed to the collector's [`event`] method
//! will contain any fields attached to each event.
//!
//! `tracing` represents values as either one of a set of Rust primitives
//! (`i64`, `u64`, `f64`, `i128`, `u128`, `bool`, and `&str`) or using a
//! `fmt::Display` or `fmt::Debug` implementation. Collectors are provided
//! these primitive value types as `dyn Value` trait objects.
//!
//! These trait objects can be formatted using `fmt::Debug`, but may also be
//! recorded as typed data by calling the [`Value::record`] method on these
//! trait objects with a _visitor_ implementing the [`Visit`] trait. This trait
//! represents the behavior used to record values of various types. For example,
//! we might record integers by incrementing counters for their field names,
//! rather than printing them.
//!
//! [span]: super::span
//! [`Event`]: super::event::Event
//! [`Metadata`]: super::metadata::Metadata
//! [`Attributes`]:  super::span::Attributes
//! [`Record`]: super::span::Record
//! [`new_span`]: super::collect::Collect::new_span
//! [`record`]: super::collect::Collect::record
//! [`event`]:  super::collect::Collect::event
use crate::callsite;
use core::{
    borrow::Borrow,
    fmt::{self, Write},
    hash::{Hash, Hasher},
    num,
    ops::Range,
};

use self::private::ValidLen;

/// An opaque key allowing _O_(1) access to a field in a `Span`'s key-value
/// data.
///
/// As keys are defined by the _metadata_ of a span, rather than by an
/// individual instance of a span, a key may be used to access the same field
/// across all instances of a given span with the same metadata. Thus, when a
/// collector observes a new span, it need only access a field by name _once_,
/// and use the key for that name for all other accesses.
#[derive(Debug)]
pub struct Field {
    i: usize,
    fields: FieldSet,
}

/// An empty field.
///
/// This can be used to indicate that the value of a field is not currently
/// present but will be recorded later.
///
/// When a field's value is `Empty`. it will not be recorded.
#[derive(Debug, Eq, PartialEq)]
pub struct Empty;

/// Describes the fields present on a span.
///
/// ## Equality
///
/// In well-behaved applications, two `FieldSet`s [initialized] with equal
/// [callsite identifiers] will have identical fields. Consequently, in release
/// builds, [`FieldSet::eq`] *only* checks that its arguments have equal
/// callsites. However, the equality of field names is checked in debug builds.
///
/// [initialized]: Self::new
/// [callsite identifiers]: callsite::Identifier
pub struct FieldSet {
    /// The names of each field on the described span.
    names: &'static [&'static str],
    /// The callsite where the described span originates.
    callsite: callsite::Identifier,
}

/// A set of fields and values for a span.
pub struct ValueSet<'a> {
    values: &'a [(&'a Field, Option<&'a (dyn Value + 'a)>)],
    fields: &'a FieldSet,
}

/// An iterator over a set of fields.
#[derive(Debug)]
pub struct Iter {
    idxs: Range<usize>,
    fields: FieldSet,
}

/// Visits typed values.
///
/// An instance of `Visit` ("a visitor") represents the logic necessary to
/// record field values of various types. When an implementor of [`Value`] is
/// [recorded], it calls the appropriate method on the provided visitor to
/// indicate the type that value should be recorded as.
///
/// When a [`Collect`] implementation [records an `Event`] or a
/// [set of `Value`s added to a `Span`], it can pass an `&mut Visit` to the
/// `record` method on the provided [`ValueSet`] or [`Event`]. This visitor
/// will then be used to record all the field-value pairs present on that
/// `Event` or `ValueSet`.
///
/// # Examples
///
/// A simple visitor that writes to a string might be implemented like so:
/// ```
/// # extern crate tracing_core as tracing;
/// use std::fmt::{self, Write};
/// use tracing::field::{Value, Visit, Field};
/// pub struct StringVisitor<'a> {
///     string: &'a mut String,
/// }
///
/// impl<'a> Visit for StringVisitor<'a> {
///     fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
///         write!(self.string, "{} = {:?}; ", field.name(), value).unwrap();
///     }
/// }
/// ```
/// This visitor will format each recorded value using `fmt::Debug`, and
/// append the field name and formatted value to the provided string,
/// regardless of the type of the recorded value. When all the values have
/// been recorded, the `StringVisitor` may be dropped, allowing the string
/// to be printed or stored in some other data structure.
///
/// The `Visit` trait provides default implementations for `record_i64`,
/// `record_u64`, `record_bool`, `record_str`, and `record_error`, which simply
/// forward the recorded value to `record_debug`. Thus, `record_debug` is the
/// only method which a `Visit` implementation *must* implement. However,
/// visitors may override the default implementations of these functions in
/// order to implement type-specific behavior.
///
/// Additionally, when a visitor receives a value of a type it does not care
/// about, it is free to ignore those values completely. For example, a
/// visitor which only records numeric data might look like this:
///
/// ```
/// # extern crate tracing_core as tracing;
/// # use std::fmt::{self, Write};
/// # use tracing::field::{Value, Visit, Field};
/// pub struct SumVisitor {
///     sum: i64,
/// }
///
/// impl Visit for SumVisitor {
///     fn record_i64(&mut self, _field: &Field, value: i64) {
///        self.sum += value;
///     }
///
///     fn record_u64(&mut self, _field: &Field, value: u64) {
///         self.sum += value as i64;
///     }
///
///     fn record_debug(&mut self, _field: &Field, _value: &dyn fmt::Debug) {
///         // Do nothing
///     }
/// }
/// ```
///
/// This visitor (which is probably not particularly useful) keeps a running
/// sum of all the numeric values it records, and ignores all other values. A
/// more practical example of recording typed values is presented in
/// `examples/counters.rs`, which demonstrates a very simple metrics system
/// implemented using `tracing`.
///
/// <div class="example-wrap" style="display:inline-block">
/// <pre class="ignore" style="white-space:normal;font:inherit;">
/// <strong>Note</strong>: The <code>record_error</code> trait method is only
/// available when the Rust standard library is present, as it requires the
/// <code>std::error::Error</code> trait.
/// </pre></div>
///
/// [recorded]: Value::record
/// [`Collect`]: super::collect::Collect
/// [records an `Event`]: super::collect::Collect::event
/// [set of `Value`s added to a `Span`]: super::collect::Collect::record
/// [`Event`]: super::event::Event
pub trait Visit {
    /// Visit a double-precision floating point value.
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.record_debug(field, &value)
    }

    /// Visit a signed 64-bit integer value.
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.record_debug(field, &value)
    }

    /// Visit an unsigned 64-bit integer value.
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record_debug(field, &value)
    }

    /// Visit a signed 128-bit integer value.
    fn record_i128(&mut self, field: &Field, value: i128) {
        self.record_debug(field, &value)
    }

    /// Visit an unsigned 128-bit integer value.
    fn record_u128(&mut self, field: &Field, value: u128) {
        self.record_debug(field, &value)
    }

    /// Visit a boolean value.
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.record_debug(field, &value)
    }

    /// Visit a string value.
    fn record_str(&mut self, field: &Field, value: &str) {
        self.record_debug(field, &value)
    }

    /// Visit a byte slice.
    fn record_bytes(&mut self, field: &Field, value: &[u8]) {
        self.record_debug(field, &HexBytes(value))
    }

    /// Records a type implementing `Error`.
    ///
    /// <div class="example-wrap" style="display:inline-block">
    /// <pre class="ignore" style="white-space:normal;font:inherit;">
    /// <strong>Note</strong>: This is only enabled when the Rust standard library is
    /// present.
    /// </pre>
    /// </div>
    #[cfg(feature = "std")]
    #[cfg_attr(docsrs, doc(cfg(feature = "std")))]
    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.record_debug(field, &DisplayValue(value))
    }

    /// Visit a value implementing `fmt::Debug`.
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug);
}

/// A field value of an erased type.
///
/// Implementors of `Value` may call the appropriate typed recording methods on
/// the [visitor] passed to their `record` method in order to indicate how
/// their data should be recorded.
///
/// [visitor]: Visit
pub trait Value: crate::sealed::Sealed {
    /// Visits this value with the given `Visitor`.
    fn record(&self, key: &Field, visitor: &mut dyn Visit);
}

/// A `Value` which serializes using `fmt::Display`.
///
/// Uses `record_debug` in the `Value` implementation to
/// avoid an unnecessary evaluation.
#[derive(Clone)]
pub struct DisplayValue<T: fmt::Display>(T);

/// A `Value` which serializes as a string using `fmt::Debug`.
#[derive(Clone)]
pub struct DebugValue<T: fmt::Debug>(T);

/// Wraps a type implementing `fmt::Display` as a `Value` that can be
/// recorded using its `Display` implementation.
pub fn display<T>(t: T) -> DisplayValue<T>
where
    T: fmt::Display,
{
    DisplayValue(t)
}

/// Wraps a type implementing `fmt::Debug` as a `Value` that can be
/// recorded using its `Debug` implementation.
pub fn debug<T>(t: T) -> DebugValue<T>
where
    T: fmt::Debug,
{
    DebugValue(t)
}

struct HexBytes<'a>(&'a [u8]);

impl fmt::Debug for HexBytes<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_char('[')?;

        let mut bytes = self.0.iter();

        if let Some(byte) = bytes.next() {
            f.write_fmt(format_args!("{byte:02x}"))?;
        }

        for byte in bytes {
            f.write_fmt(format_args!(" {byte:02x}"))?;
        }

        f.write_char(']')
    }
}

// ===== impl Visit =====

impl Visit for fmt::DebugStruct<'_, '_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.field(field.name(), value);
    }
}

impl Visit for fmt::DebugMap<'_, '_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.entry(&format_args!("{}", field), value);
    }
}

impl<F> Visit for F
where
    F: FnMut(&Field, &dyn fmt::Debug),
{
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        (self)(field, value)
    }
}

// ===== impl Value =====

macro_rules! impl_values {
    ( $( $record:ident( $( $whatever:tt)+ ) ),+ ) => {
        $(
            impl_value!{ $record( $( $whatever )+ ) }
        )+
    }
}

macro_rules! ty_to_nonzero {
    (u8) => {
        NonZeroU8
    };
    (u16) => {
        NonZeroU16
    };
    (u32) => {
        NonZeroU32
    };
    (u64) => {
        NonZeroU64
    };
    (u128) => {
        NonZeroU128
    };
    (usize) => {
        NonZeroUsize
    };
    (i8) => {
        NonZeroI8
    };
    (i16) => {
        NonZeroI16
    };
    (i32) => {
        NonZeroI32
    };
    (i64) => {
        NonZeroI64
    };
    (i128) => {
        NonZeroI128
    };
    (isize) => {
        NonZeroIsize
    };
}

macro_rules! impl_one_value {
    (f32, $op:expr, $record:ident) => {
        impl_one_value!(normal, f32, $op, $record);
    };
    (f64, $op:expr, $record:ident) => {
        impl_one_value!(normal, f64, $op, $record);
    };
    (bool, $op:expr, $record:ident) => {
        impl_one_value!(normal, bool, $op, $record);
    };
    ($value_ty:tt, $op:expr, $record:ident) => {
        impl_one_value!(normal, $value_ty, $op, $record);
        impl_one_value!(nonzero, $value_ty, $op, $record);
    };
    (normal, $value_ty:tt, $op:expr, $record:ident) => {
        impl $crate::sealed::Sealed for $value_ty {}
        impl $crate::field::Value for $value_ty {
            fn record(&self, key: &$crate::field::Field, visitor: &mut dyn $crate::field::Visit) {
                // `op` is always a function; the closure is used because
                // sometimes there isn't a real function corresponding to that
                // operation. the clippy warning is not that useful here.
                #[allow(clippy::redundant_closure_call)]
                visitor.$record(key, $op(*self))
            }
        }
    };
    (nonzero, $value_ty:tt, $op:expr, $record:ident) => {
        // This `use num::*;` is reported as unused because it gets emitted
        // for every single invocation of this macro, so there are multiple `use`s.
        // All but the first are useless indeed.
        // We need this import because we can't write a path where one part is
        // the `ty_to_nonzero!($value_ty)` invocation.
        #[allow(clippy::useless_attribute, unused)]
        use num::*;
        impl $crate::sealed::Sealed for ty_to_nonzero!($value_ty) {}
        impl $crate::field::Value for ty_to_nonzero!($value_ty) {
            fn record(&self, key: &$crate::field::Field, visitor: &mut dyn $crate::field::Visit) {
                // `op` is always a function; the closure is used because
                // sometimes there isn't a real function corresponding to that
                // operation. the clippy warning is not that useful here.
                #[allow(clippy::redundant_closure_call)]
                visitor.$record(key, $op(self.get()))
            }
        }
    };
}

macro_rules! impl_value {
    ( $record:ident( $( $value_ty:tt ),+ ) ) => {
        $(
            impl_one_value!($value_ty, |this: $value_ty| this, $record);
        )+
    };
    ( $record:ident( $( $value_ty:tt ),+ as $as_ty:ty) ) => {
        $(
            impl_one_value!($value_ty, |this: $value_ty| this as $as_ty, $record);
        )+
    };
}

// ===== impl Value =====

impl_values! {
    record_u64(u64),
    record_u64(usize, u32, u16, u8 as u64),
    record_i64(i64),
    record_i64(isize, i32, i16, i8 as i64),
    record_u128(u128),
    record_i128(i128),
    record_bool(bool),
    record_f64(f64, f32 as f64)
}

impl<T: crate::sealed::Sealed> crate::sealed::Sealed for Wrapping<T> {}
impl<T: crate::field::Value> crate::field::Value for Wrapping<T> {
    fn record(&self, key: &crate::field::Field, visitor: &mut dyn crate::field::Visit) {
        self.0.record(key, visitor)
    }
}

impl crate::sealed::Sealed for str {}

impl Value for str {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_str(key, self)
    }
}

impl crate::sealed::Sealed for [u8] {}

impl Value for [u8] {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_bytes(key, self)
    }
}

#[cfg(feature = "std")]
impl crate::sealed::Sealed for dyn std::error::Error + 'static {}

#[cfg(feature = "std")]
#[cfg_attr(docsrs, doc(cfg(feature = "std")))]
impl Value for dyn std::error::Error + 'static {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_error(key, self)
    }
}

#[cfg(feature = "std")]
impl crate::sealed::Sealed for dyn std::error::Error + Send + 'static {}

#[cfg(feature = "std")]
#[cfg_attr(docsrs, doc(cfg(feature = "std")))]
impl Value for dyn std::error::Error + Send + 'static {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        (self as &dyn std::error::Error).record(key, visitor)
    }
}

#[cfg(feature = "std")]
impl crate::sealed::Sealed for dyn std::error::Error + Sync + 'static {}

#[cfg(feature = "std")]
#[cfg_attr(docsrs, doc(cfg(feature = "std")))]
impl Value for dyn std::error::Error + Sync + 'static {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        (self as &dyn std::error::Error).record(key, visitor)
    }
}

#[cfg(feature = "std")]
impl crate::sealed::Sealed for dyn std::error::Error + Send + Sync + 'static {}

#[cfg(feature = "std")]
#[cfg_attr(docsrs, doc(cfg(feature = "std")))]
impl Value for dyn std::error::Error + Send + Sync + 'static {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        (self as &dyn std::error::Error).record(key, visitor)
    }
}

impl<'a, T: ?Sized> crate::sealed::Sealed for &'a T where T: Value + crate::sealed::Sealed + 'a {}

impl<'a, T: ?Sized> Value for &'a T
where
    T: Value + 'a,
{
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        (*self).record(key, visitor)
    }
}

impl<'a, T: ?Sized> crate::sealed::Sealed for &'a mut T where T: Value + crate::sealed::Sealed + 'a {}

impl<'a, T: ?Sized> Value for &'a mut T
where
    T: Value + 'a,
{
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        // Don't use `(*self).record(key, visitor)`, otherwise would
        // cause stack overflow due to `unconditional_recursion`.
        T::record(self, key, visitor)
    }
}

impl crate::sealed::Sealed for fmt::Arguments<'_> {}

impl Value for fmt::Arguments<'_> {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_debug(key, self)
    }
}

#[cfg(feature = "alloc")]
impl<T: ?Sized> crate::sealed::Sealed for alloc::boxed::Box<T> where T: Value {}

#[cfg(feature = "alloc")]
#[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
impl<T: ?Sized> Value for alloc::boxed::Box<T>
where
    T: Value,
{
    #[inline]
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        self.as_ref().record(key, visitor)
    }
}

#[cfg(feature = "alloc")]
#[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
impl crate::sealed::Sealed for alloc::string::String {}

#[cfg(feature = "alloc")]
#[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
impl Value for alloc::string::String {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_str(key, self.as_str())
    }
}

impl fmt::Debug for dyn Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // We are only going to be recording the field value, so we don't
        // actually care about the field name here.
        struct NullCallsite;
        static NULL_CALLSITE: NullCallsite = NullCallsite;
        impl crate::callsite::Callsite for NullCallsite {
            fn set_interest(&self, _: crate::collect::Interest) {
                unreachable!("you somehow managed to register the null callsite?")
            }

            fn metadata(&self) -> &crate::Metadata<'_> {
                unreachable!("you somehow managed to access the null callsite?")
            }
        }

        static FIELD: Field = Field {
            i: 0,
            fields: FieldSet::new(&[], crate::identify_callsite!(&NULL_CALLSITE)),
        };

        let mut res = Ok(());
        self.record(&FIELD, &mut |_: &Field, val: &dyn fmt::Debug| {
            res = write!(f, "{:?}", val);
        });
        res
    }
}

impl fmt::Display for dyn Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

// ===== impl DisplayValue =====

impl<T: fmt::Display> crate::sealed::Sealed for DisplayValue<T> {}

impl<T> Value for DisplayValue<T>
where
    T: fmt::Display,
{
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_debug(key, self)
    }
}

impl<T: fmt::Display> fmt::Debug for DisplayValue<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl<T: fmt::Display> fmt::Display for DisplayValue<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

// ===== impl DebugValue =====

impl<T: fmt::Debug> crate::sealed::Sealed for DebugValue<T> {}

impl<T: fmt::Debug> Value for DebugValue<T>
where
    T: fmt::Debug,
{
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_debug(key, &self.0)
    }
}

impl<T: fmt::Debug> fmt::Debug for DebugValue<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl crate::sealed::Sealed for Empty {}
impl Value for Empty {
    #[inline]
    fn record(&self, _: &Field, _: &mut dyn Visit) {}
}

// ===== impl Field =====

impl Field {
    /// Returns an [`Identifier`] that uniquely identifies the [`Callsite`]
    /// which defines this field.
    ///
    /// [`Identifier`]: super::callsite::Identifier
    /// [`Callsite`]: super::callsite::Callsite
    #[inline]
    pub fn callsite(&self) -> callsite::Identifier {
        self.fields.callsite()
    }

    /// Returns a string representing the name of the field.
    pub fn name(&self) -> &'static str {
        self.fields.names[self.i]
    }

    /// Returns the index of this field in its [`FieldSet`].
    pub fn index(&self) -> usize {
        self.i
    }
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.name())
    }
}

impl AsRef<str> for Field {
    fn as_ref(&self) -> &str {
        self.name()
    }
}

impl PartialEq for Field {
    fn eq(&self, other: &Self) -> bool {
        self.callsite() == other.callsite() && self.i == other.i
    }
}

impl Eq for Field {}

impl Hash for Field {
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        self.callsite().hash(state);
        self.i.hash(state);
    }
}

impl Clone for Field {
    fn clone(&self) -> Self {
        Field {
            i: self.i,
            fields: FieldSet {
                names: self.fields.names,
                callsite: self.fields.callsite(),
            },
        }
    }
}

// ===== impl FieldSet =====

impl FieldSet {
    /// Constructs a new `FieldSet` with the given array of field names and callsite.
    pub const fn new(names: &'static [&'static str], callsite: callsite::Identifier) -> Self {
        Self { names, callsite }
    }

    /// Returns an [`Identifier`] that uniquely identifies the [`Callsite`]
    /// which defines this set of fields..
    ///
    /// [`Identifier`]: super::callsite::Identifier
    /// [`Callsite`]: super::callsite::Callsite
    #[inline]
    pub(crate) fn callsite(&self) -> callsite::Identifier {
        callsite::Identifier(self.callsite.0)
    }

    /// Returns the [`Field`] named `name`, or `None` if no such field exists.
    ///
    /// [`Field`]: super::Field
    pub fn field<Q>(&self, name: &Q) -> Option<Field>
    where
        Q: Borrow<str> + ?Sized,
    {
        let name = &name.borrow();
        self.names.iter().position(|f| f == name).map(|i| Field {
            i,
            fields: FieldSet {
                names: self.names,
                callsite: self.callsite(),
            },
        })
    }

    /// Returns `true` if `self` contains the given `field`.
    ///
    /// <div class="example-wrap" style="display:inline-block">
    /// <pre class="ignore" style="white-space:normal;font:inherit;">
    /// <strong>Note</strong>: If <code>field</code> shares a name with a field
    /// in this <code>FieldSet</code>, but was created by a <code>FieldSet</code>
    /// with a different callsite, this <code>FieldSet</code> does <em>not</em>
    /// contain it. This is so that if two separate span callsites define a field
    /// named "foo", the <code>Field</code> corresponding to "foo" for each
    /// of those callsites are not equivalent.
    /// </pre></div>
    pub fn contains(&self, field: &Field) -> bool {
        field.callsite() == self.callsite() && field.i <= self.len()
    }

    /// Returns an iterator over the `Field`s in this `FieldSet`.
    #[inline]
    pub fn iter(&self) -> Iter {
        let idxs = 0..self.len();
        Iter {
            idxs,
            fields: FieldSet {
                names: self.names,
                callsite: self.callsite(),
            },
        }
    }

    /// Returns a new `ValueSet` with entries for this `FieldSet`'s values.
    #[doc(hidden)]
    pub fn value_set<'v, V>(&'v self, values: &'v V) -> ValueSet<'v>
    where
        V: ValidLen<'v>,
    {
        ValueSet {
            fields: self,
            values: values.borrow(),
        }
    }

    /// Returns the number of fields in this `FieldSet`.
    #[inline]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Returns whether or not this `FieldSet` has fields.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

impl IntoIterator for &FieldSet {
    type IntoIter = Iter;
    type Item = Field;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl fmt::Debug for FieldSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FieldSet")
            .field("names", &self.names)
            .field("callsite", &self.callsite)
            .finish()
    }
}

impl fmt::Display for FieldSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set()
            .entries(self.names.iter().map(display))
            .finish()
    }
}

impl Eq for FieldSet {}

impl PartialEq for FieldSet {
    fn eq(&self, other: &Self) -> bool {
        if core::ptr::eq(&self, &other) {
            true
        } else if cfg!(not(debug_assertions)) {
            // In a well-behaving application, two `FieldSet`s can be assumed to
            // be totally equal so long as they share the same callsite.
            self.callsite == other.callsite
        } else {
            // However, when debug-assertions are enabled, do NOT assume that
            // the application is well-behaving; check every the field names of
            // each `FieldSet` for equality.

            // `FieldSet` is destructured here to ensure a compile-error if the
            // fields of `FieldSet` change.
            let Self {
                names: lhs_names,
                callsite: lhs_callsite,
            } = self;

            let Self {
                names: rhs_names,
                callsite: rhs_callsite,
            } = &other;

            // Check callsite equality first, as it is probably cheaper to do
            // than str equality.
            lhs_callsite == rhs_callsite && lhs_names == rhs_names
        }
    }
}

// ===== impl Iter =====

impl Iterator for Iter {
    type Item = Field;
    #[inline]
    fn next(&mut self) -> Option<Field> {
        let i = self.idxs.next()?;
        Some(Field {
            i,
            fields: FieldSet {
                names: self.fields.names,
                callsite: self.fields.callsite(),
            },
        })
    }
}

// ===== impl ValueSet =====

impl ValueSet<'_> {
    /// Returns an [`Identifier`] that uniquely identifies the [`Callsite`]
    /// defining the fields this `ValueSet` refers to.
    ///
    /// [`Identifier`]: super::callsite::Identifier
    /// [`Callsite`]: super::callsite::Callsite
    #[inline]
    pub fn callsite(&self) -> callsite::Identifier {
        self.fields.callsite()
    }

    /// Visits all the fields in this `ValueSet` with the provided [visitor].
    ///
    /// [visitor]: Visit
    pub fn record(&self, visitor: &mut dyn Visit) {
        let my_callsite = self.callsite();
        for (field, value) in self.values {
            if field.callsite() != my_callsite {
                continue;
            }
            if let Some(value) = value {
                value.record(field, visitor);
            }
        }
    }

    /// Returns the number of fields in this `ValueSet` that would be visited
    /// by a given [visitor] to the [`ValueSet::record()`] method.
    ///
    /// [visitor]: Visit
    /// [`ValueSet::record()`]: ValueSet::record()
    pub fn len(&self) -> usize {
        let my_callsite = self.callsite();
        self.values
            .iter()
            .filter(|(field, _)| field.callsite() == my_callsite)
            .count()
    }

    /// Returns `true` if this `ValueSet` contains a value for the given `Field`.
    pub(crate) fn contains(&self, field: &Field) -> bool {
        field.callsite() == self.callsite()
            && self
                .values
                .iter()
                .any(|(key, val)| *key == field && val.is_some())
    }

    /// Returns true if this `ValueSet` contains _no_ values.
    pub fn is_empty(&self) -> bool {
        let my_callsite = self.callsite();
        self.values
            .iter()
            .all(|(key, val)| val.is_none() || key.callsite() != my_callsite)
    }

    pub(crate) fn field_set(&self) -> &FieldSet {
        self.fields
    }
}

impl fmt::Debug for ValueSet<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.values
            .iter()
            .fold(&mut f.debug_struct("ValueSet"), |dbg, (key, v)| {
                if let Some(val) = v {
                    val.record(key, dbg);
                }
                dbg
            })
            .field("callsite", &self.callsite())
            .finish()
    }
}

impl fmt::Display for ValueSet<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.values
            .iter()
            .fold(&mut f.debug_map(), |dbg, (key, v)| {
                if let Some(val) = v {
                    val.record(key, dbg);
                }
                dbg
            })
            .finish()
    }
}

// ===== impl ValidLen =====

mod private {
    use super::*;

    /// Restrictions on `ValueSet` lengths were removed in #2508 but this type remains for backwards compatibility.
    pub trait ValidLen<'a>: Borrow<[(&'a Field, Option<&'a (dyn Value + 'a)>)]> {}

    impl<'a, const N: usize> ValidLen<'a> for [(&'a Field, Option<&'a (dyn Value + 'a)>); N] {}
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::metadata::{Kind, Level, Metadata};

    // Make sure TEST_CALLSITE_* have non-zero size, so they can't be located at the same address.
    struct TestCallsite1();
    static TEST_CALLSITE_1: TestCallsite1 = TestCallsite1();
    static TEST_META_1: Metadata<'static> = metadata! {
        name: "field_test1",
        target: module_path!(),
        level: Level::INFO,
        fields: &["foo", "bar", "baz"],
        callsite: &TEST_CALLSITE_1,
        kind: Kind::SPAN,
    };

    impl crate::callsite::Callsite for TestCallsite1 {
        fn set_interest(&self, _: crate::collect::Interest) {
            unimplemented!()
        }

        fn metadata(&self) -> &Metadata<'_> {
            &TEST_META_1
        }
    }

    struct TestCallsite2();
    static TEST_CALLSITE_2: TestCallsite2 = TestCallsite2();
    static TEST_META_2: Metadata<'static> = metadata! {
        name: "field_test2",
        target: module_path!(),
        level: Level::INFO,
        fields: &["foo", "bar", "baz"],
        callsite: &TEST_CALLSITE_2,
        kind: Kind::SPAN,
    };

    impl crate::callsite::Callsite for TestCallsite2 {
        fn set_interest(&self, _: crate::collect::Interest) {
            unimplemented!()
        }

        fn metadata(&self) -> &Metadata<'_> {
            &TEST_META_2
        }
    }

    #[test]
    fn value_set_with_no_values_is_empty() {
        let fields = TEST_META_1.fields();
        let values = &[
            (&fields.field("foo").unwrap(), None),
            (&fields.field("bar").unwrap(), None),
            (&fields.field("baz").unwrap(), None),
        ];
        let valueset = fields.value_set(values);
        assert!(valueset.is_empty());
    }

    #[test]
    fn index_of_field_in_fieldset_is_correct() {
        let fields = TEST_META_1.fields();
        let foo = fields.field("foo").unwrap();
        assert_eq!(foo.index(), 0);
        let bar = fields.field("bar").unwrap();
        assert_eq!(bar.index(), 1);
        let baz = fields.field("baz").unwrap();
        assert_eq!(baz.index(), 2);
    }

    #[test]
    fn empty_value_set_is_empty() {
        let fields = TEST_META_1.fields();
        let valueset = fields.value_set(&[]);
        assert!(valueset.is_empty());
    }

    #[test]
    fn value_sets_with_fields_from_other_callsites_are_empty() {
        let fields = TEST_META_1.fields();
        let values = &[
            (&fields.field("foo").unwrap(), Some(&1 as &dyn Value)),
            (&fields.field("bar").unwrap(), Some(&2 as &dyn Value)),
            (&fields.field("baz").unwrap(), Some(&3 as &dyn Value)),
        ];
        let valueset = TEST_META_2.fields().value_set(values);
        assert!(valueset.is_empty())
    }

    #[test]
    fn sparse_value_sets_are_not_empty() {
        let fields = TEST_META_1.fields();
        let values = &[
            (&fields.field("foo").unwrap(), None),
            (&fields.field("bar").unwrap(), Some(&57 as &dyn Value)),
            (&fields.field("baz").unwrap(), None),
        ];
        let valueset = fields.value_set(values);
        assert!(!valueset.is_empty());
    }

    #[test]
    fn fields_from_other_callsets_are_skipped() {
        let fields = TEST_META_1.fields();
        let values = &[
            (&fields.field("foo").unwrap(), None),
            (
                &TEST_META_2.fields().field("bar").unwrap(),
                Some(&57 as &dyn Value),
            ),
            (&fields.field("baz").unwrap(), None),
        ];

        struct MyVisitor;
        impl Visit for MyVisitor {
            fn record_debug(&mut self, field: &Field, _: &dyn (core::fmt::Debug)) {
                assert_eq!(field.callsite(), TEST_META_1.callsite())
            }
        }
        let valueset = fields.value_set(values);
        valueset.record(&mut MyVisitor);
    }

    #[test]
    fn empty_fields_are_skipped() {
        let fields = TEST_META_1.fields();
        let values = &[
            (&fields.field("foo").unwrap(), Some(&Empty as &dyn Value)),
            (&fields.field("bar").unwrap(), Some(&57 as &dyn Value)),
            (&fields.field("baz").unwrap(), Some(&Empty as &dyn Value)),
        ];

        struct MyVisitor;
        impl Visit for MyVisitor {
            fn record_debug(&mut self, field: &Field, _: &dyn (core::fmt::Debug)) {
                assert_eq!(field.name(), "bar")
            }
        }
        let valueset = fields.value_set(values);
        valueset.record(&mut MyVisitor);
    }

    #[test]
    #[cfg(feature = "std")]
    fn record_debug_fn() {
        let fields = TEST_META_1.fields();
        let values = &[
            (&fields.field("foo").unwrap(), Some(&1 as &dyn Value)),
            (&fields.field("bar").unwrap(), Some(&2 as &dyn Value)),
            (&fields.field("baz").unwrap(), Some(&3 as &dyn Value)),
        ];
        let valueset = fields.value_set(values);
        let mut result = String::new();
        valueset.record(&mut |_: &Field, value: &dyn fmt::Debug| {
            use core::fmt::Write;
            write!(&mut result, "{:?}", value).unwrap();
        });
        assert_eq!(result, String::from("123"));
    }

    #[test]
    #[cfg(feature = "std")]
    fn record_error() {
        let fields = TEST_META_1.fields();
        let err: Box<dyn std::error::Error + Send + Sync + 'static> =
            std::io::Error::new(std::io::ErrorKind::Other, "lol").into();
        let values = &[
            (&fields.field("foo").unwrap(), Some(&err as &dyn Value)),
            (&fields.field("bar").unwrap(), Some(&Empty as &dyn Value)),
            (&fields.field("baz").unwrap(), Some(&Empty as &dyn Value)),
        ];
        let valueset = fields.value_set(values);
        let mut result = String::new();
        valueset.record(&mut |_: &Field, value: &dyn fmt::Debug| {
            use core::fmt::Write;
            write!(&mut result, "{:?}", value).unwrap();
        });
        assert_eq!(result, format!("{}", err));
    }

    #[test]
    fn record_bytes() {
        let fields = TEST_META_1.fields();
        let first = &b"abc"[..];
        let second: &[u8] = &[192, 255, 238];
        let values = &[
            (&fields.field("foo").unwrap(), Some(&first as &dyn Value)),
            (&fields.field("bar").unwrap(), Some(&" " as &dyn Value)),
            (&fields.field("baz").unwrap(), Some(&second as &dyn Value)),
        ];
        let valueset = fields.value_set(values);
        let mut result = String::new();
        valueset.record(&mut |_: &Field, value: &dyn fmt::Debug| {
            use core::fmt::Write;
            write!(&mut result, "{:?}", value).unwrap();
        });
        assert_eq!(result, format!("{}", r#"[61 62 63]" "[c0 ff ee]"#));
    }
}
