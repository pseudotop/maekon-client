//! define_code_enum! — single-source macro for code enum definitions.
//!
//! Generates enum body, `as_str` match, `Display` impl, and `all()` enumerator
//! from one variant list. Prevents drift between match and array per §7.5 of spec
//! `docs/superpowers/specs/2026-04-19-error-code-infrastructure-design.md`.

#[macro_export]
macro_rules! define_code_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident => $wire:literal, )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum $name {
            $( $(#[$vmeta])* $variant, )+
        }

        impl $name {
            /// Wire-format code string. Immutable after first release — see ADR-019 §2.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( Self::$variant => $wire, )+
                }
            }

            /// Compile-time exhaustive enumeration. The `as_str` match above is
            /// the enforcement point — adding a variant without updating it fails
            /// `cargo build`. This method derives from the same list so drift
            /// between match and array is architecturally impossible.
            ///
            /// `pub(crate)` so that (a) the `error_codes/mod.rs` aggregator can
            /// call `{Xyz}Code::all()` and (b) per-enum `#[cfg(test)] mod tests`
            /// blocks inside the enum's own file can enumerate variants.
            #[allow(dead_code)]
            pub(crate) const fn all() -> &'static [Self] {
                &[ $( Self::$variant, )+ ]
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}
