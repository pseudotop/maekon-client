pub use maekon_core::provider_surface_catalog::*;

#[cfg(test)]
mod tests {
    use maekon_core::provider_surface_catalog as core_catalog;

    #[test]
    fn provider_specs_wrapper_delegates_to_core_catalog_owner() {
        let surface = super::provider_surface_spec("provider_surface.openai.subprocess_cli")
            .unwrap_or_else(|error| panic!("wrapper should expose core-owned catalog: {error}"));
        let core_surface =
            core_catalog::provider_surface_spec("provider_surface.openai.subprocess_cli")
                .unwrap_or_else(|error| {
                    panic!("core catalog should expose the same surface: {error}");
                });

        assert_eq!(surface.surface_id, core_surface.surface_id);
        assert_eq!(surface.execution_kind, core_surface.execution_kind);
    }
}
