//! CAP-003: generated Bus handler registry.
//! Host calls `register_all` once to wire names → dispatch.

/// All Bus message types exported by this workspace.
pub const HANDLER_NAMES: &[&str] = &[
    "goto",
    "invalidateAll",
    "location_last_segment",
    "location_pathname",
];

/// Register every generated handler name with a host-supplied registrar.
///
/// The host provides the actual dispatch (ports / platform). This module
/// only owns the name list so trampoline code never hardcodes it.
pub fn register_all<F>(mut register: F)
where
    F: FnMut(&'static str),
{
    for name in HANDLER_NAMES {
        register(name);
    }
}

/// Number of handlers in this workspace.
pub fn handler_count() -> usize {
    HANDLER_NAMES.len()
}
