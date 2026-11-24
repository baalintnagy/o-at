//! The CLAP and VST3 entry points shared by every O-AT plugin.

/// Exports `$plugin` as a CLAP and a VST3 plugin.
///
/// Invoke it once, at the plugin crate's root, with a type that implements `ClapPlugin` and
/// `Vst3Plugin`. The calling crate must depend on `nih_plug`, whose export macros this
/// expands to; macro expansions need the absolute `::nih_plug` paths.
#[macro_export]
macro_rules! export_plugin {
    ($plugin:ident) => {
        ::nih_plug::nih_export_clap!($plugin);

        // The VST3 bindings' COM macros expand to a trailing semicolon in expression
        // position, which only NIH-plug can fix; the allow is limited to this module.
        #[allow(semicolon_in_expressions_from_non_local_macros)]
        mod vst3_export {
            use super::$plugin;

            ::nih_plug::nih_export_vst3!($plugin);
        }
    };
}
