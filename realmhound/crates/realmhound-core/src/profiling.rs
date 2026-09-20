//! Thin profiling shims around `puffin`.
//!
//! All macros compile to nothing unless the `profiling` feature is enabled, so
//! instrumentation can be left in hot paths at zero cost for normal builds.
//!
//! Enable with `--features profiling` (workspace crate: `RealmHound/profiling`).

/// Profile the enclosing scope. The name should be a short static string.
///
/// ```ignore
/// realmhound_core::prof_scope!("reassemble");
/// ```
#[macro_export]
macro_rules! prof_scope {
    ($name:expr) => {
        #[cfg(feature = "profiling")]
        $crate::__puffin::profile_scope!($name);
    };
    ($name:expr, $data:expr) => {
        #[cfg(feature = "profiling")]
        $crate::__puffin::profile_scope!($name, $data);
    };
}

/// Profile the enclosing function, using its name as the scope label.
#[macro_export]
macro_rules! prof_function {
    () => {
        #[cfg(feature = "profiling")]
        $crate::__puffin::profile_function!();
    };
}
